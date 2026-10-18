//! notify.rs: [post] [why-script] [why-output]
//! 通知交给内嵌 scripts/notify.sh：root 需换 uid 2000，退出码 0 不代表落地，靠脚本回传 strategy/verify 判真落地
//! 仅 ChiRi-FAS 带拦截器：前 120 条静默，成功才记 1h 去重，失败进 60s 冷却

use std::sync::Mutex;

use crate::fluent_args;
use crate::i18n::t_with_args;

/// FAS tag：唯一带拦截器的 tag
const FAS_TAG: &str = "ChiRi-FAS";
/// ChiRi-FAS 前 N 条静默丢弃（防误报）
const FAS_SILENCE_COUNT: u32 = 120;
/// FAS 相同 (标题, 正文) 去重窗口（仅投递成功才进入）
const FAS_DEDUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(3600);
/// 投递失败后的冷却期，期间不再尝试
const FAS_FAIL_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);

/// ChiRi-FAS 拦截器状态：已静默计数 + 上次成功发送的 (标题, 正文, 时刻) + 上次失败时刻
struct FasGuard {
    silenced: u32,
    last_sent: Option<(String, String, std::time::Instant)>,
    last_fail: Option<std::time::Instant>,
}

static FAS_GUARD: Mutex<FasGuard> = Mutex::new(FasGuard {
    silenced: 0,
    last_sent: None,
    last_fail: None,
});

// [post]
/// 三参投递通知，返回脚本退出码 0；同步执行，只用于低频事件路径
/// `true` 只表示脚本报告某条策略被 cmd 接受，不等于用户看得到（见 [why-script]）
pub fn post_notification(title: &str, tag: &str, body: &str) -> bool {
    // [notify-switch] 刻意不接 `meta.notify`：该字段是已删除的常驻状态通知遗留，各机型 meta.yaml 全是 false
    // 本函数是唯一投递点（FAS 开环告警 + 启动完成），接上会让真机默认静默 FAS 告警
    let guarded = tag == FAS_TAG;
    // 放行判定只读状态，成功与否等脚本返回
    if guarded && !fas_allow(title, body) {
        return false;
    }

    // [embed] 脚本由 `scripts::run` 从二进制内嵌正文释放到私有临时目录执行，磁盘副本不参与运行时
    let output = crate::scripts::run(crate::scripts::Script::Notify, &[title, tag, body]);

    match output {
        Some(out) => {
            // cmd 的用法/报错走 Java 侧 out 流（stdout），两路都要收进诊断
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if out.status.success() {
                if guarded {
                    fas_record_sent(title, body);
                }
                // 成功策略号：S1/S2 走 su（可见），末尾直接投递是已知不可见的那条
                log::info!(
                    "{}",
                    t_with_args(
                        "notify-post-ok",
                        &fluent_args! {
                            "tag" => tag,
                            "title" => title,
                            "strategy" => strategy_of(&stdout),
                            "verify" => verdict_of(&stdout),
                        },
                    )
                );
                true
            } else {
                if guarded {
                    fas_record_fail();
                }
                let detail = join_detail(&stdout, &stderr, out.status.code());
                log_post_failure(tag, title, &detail);
                false
            }
        }
        None => {
            if guarded {
                fas_record_fail();
            }
            // 释放或 spawn 失败：脚本没跑起来，与「跑了但投递失败」区分
            log::warn!(
                "{}",
                t_with_args(
                    "notify-script-missing",
                    &fluent_args! {"path" => "scripts/notify.sh（内嵌）"},
                )
            );
            false
        }
    }
}

/// 从脚本 stdout 取成功策略那一行（`OK S<n>`）；取不到给 `-`，避免日志里出现空值
fn strategy_of(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("OK ").map(str::trim))
        .filter(|s| !s.is_empty())
        .unwrap_or("-")
        .to_string()
}

/// 从脚本 stdout 取验证结论（`VERIFY …` / `LIST …`）：`posted` 是 AOSP 回查确认落地，`unverified` 无证据可判
/// 退出码只说明策略被接受，这一位才是用户看不看得到的线索，留痕不能省
fn verdict_of(stdout: &str) -> String {
    for line in stdout.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("VERIFY ") {
            return rest.trim().to_string();
        }
        if let Some(rest) = l.strip_prefix("LIST ") {
            return format!("list {}", rest.trim());
        }
    }
    "unverified".to_string()
}

/// 组装失败诊断：stdout 逐行 + stderr + 退出码，折成单行以免刷屏
fn join_detail(stdout: &str, stderr: &str, code: Option<i32>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !stdout.is_empty() {
        parts.push(stdout.replace('\n', " | "));
    }
    if !stderr.is_empty() {
        parts.push(stderr.replace('\n', " | "));
    }
    match code {
        Some(c) => parts.push(format!("exit {c}")),
        None => parts.push("terminated by signal".to_string()),
    }
    parts.join(" — ")
}

/// 投递失败留痕：i18n key `notify-post-failed`，诊断附在译文之后
fn log_post_failure(tag: &str, title: &str, reason: &str) {
    log::warn!(
        "{} ({})",
        t_with_args(
            "notify-post-failed",
            &fluent_args! {"tag" => tag, "title" => title},
        ),
        reason
    );
}

/// ChiRi-FAS 放行判定（不写 `last_sent`）：前 120 条静默、成功过则 1h 同 (标题, 正文) 去重、失败后 60s 冷却
/// 静默计数在此推进，第 121 条起进入放行判定
fn fas_allow(title: &str, body: &str) -> bool {
    let mut g = FAS_GUARD.lock().unwrap_or_else(|e| e.into_inner());
    if g.silenced < FAS_SILENCE_COUNT {
        g.silenced += 1;
        // [level] 静默窗口用尽只发生一次，留一条 info；窗口内逐条丢弃不打点，否则日志刷满
        if g.silenced == FAS_SILENCE_COUNT {
            log::info!(
                "[notify] FAS 静默窗口用尽（已丢弃 {} 条），之后放行",
                FAS_SILENCE_COUNT
            );
        }
        return false;
    }
    if let Some((lt, lb, at)) = &g.last_sent {
        if lt == title && lb == body && at.elapsed() < FAS_DEDUP_WINDOW {
            // [level] 去重窗口内丢弃静默，只留 debug + log_enabled 门控（INFO 下零 format）
            if log::log_enabled!(log::Level::Debug) {
                log::debug!("[notify] FAS 去重窗口命中，丢弃: {} / {}", title, body);
            }
            return false;
        }
    }
    // 失败冷却：冷却期内不重试，过后放行
    if g.last_fail
        .is_some_and(|at| at.elapsed() < FAS_FAIL_COOLDOWN)
    {
        if log::log_enabled!(log::Level::Debug) {
            log::debug!("[notify] FAS 失败冷却中，丢弃: {} / {}", title, body);
        }
        return false;
    }
    true
}

/// 投递成功才记账：记 (标题, 正文, 时刻) 供去重，并清除失败冷却
fn fas_record_sent(title: &str, body: &str) {
    let mut g = FAS_GUARD.lock().unwrap_or_else(|e| e.into_inner());
    g.last_sent = Some((
        title.to_string(),
        body.to_string(),
        std::time::Instant::now(),
    ));
    g.last_fail = None;
}

/// 投递失败记账：只进冷却，不写 `last_sent`，冷却过后仍可重试
fn fas_record_fail() {
    FAS_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .last_fail = Some(std::time::Instant::now());
}

/// i18n 自检：脚本缺失与成功两条 key 在两种语言里都存在，避免日志里出现裸 key
#[cfg(test)]
mod tests {
    use super::*;

    /// 脚本由 `scripts.rs` 内嵌并由 `run` 释放执行，本模块不拼磁盘路径
    #[test]
    fn notify_runs_the_embedded_script_not_a_disk_path() {
        use crate::scripts::{NOTIFY_SH, Script};
        assert_eq!(Script::Notify.body(), NOTIFY_SH);
        assert!(NOTIFY_SH.contains("notify.sh"), "内嵌正文应是通知脚本");
    }

    #[test]
    fn strategy_of_picks_the_ok_line() {
        let out =
            "S1=su 2000 /system/bin/cmd notification post -v -t <title> <tag> <body>\nS1:ok\nOK S1";
        assert_eq!(strategy_of(out), "S1");
    }

    #[test]
    fn strategy_of_falls_back_to_dash() {
        assert_eq!(strategy_of("S1=fail\nFAIL 所有策略均失败"), "-");
        assert_eq!(strategy_of(""), "-");
    }

    #[test]
    fn join_detail_folds_multiline_and_keeps_exit_code() {
        let d = join_detail("S1:fail:1 error\nFAIL x", "boom", Some(1));
        assert!(d.contains("S1:fail:1 error | FAIL x"));
        assert!(d.contains("boom"));
        assert!(d.contains("exit 1"));
    }

    #[test]
    fn verdict_of_reads_verify_and_list_lines() {
        assert_eq!(verdict_of("OK S1\nVERIFY posted\n"), "posted");
        assert_eq!(
            verdict_of("OK S1\nVERIFY unverified（老 ROM 不认 -v）"),
            "unverified（老 ROM 不认 -v）"
        );
        assert_eq!(
            verdict_of("OK S1\nLIST ok ChiRi-Test"),
            "list ok ChiRi-Test"
        );
        // 两种验证都没给出可判结论：不能默认成功，如实记 unverified
        assert_eq!(verdict_of("OK S1"), "unverified");
    }

    #[test]
    fn notify_keys_exist() {
        // 查编译期嵌入的 ftl 原文而非 t()：单测环境没加载语言包，t() 一律回退 key 名
        let zh = crate::common::embedded_ftl_str("zh");
        let en = crate::common::embedded_ftl_str("en");
        for key in [
            "notify-post-ok",
            "notify-script-missing",
            "notify-post-failed",
        ] {
            // ftl 行形如 `key = 文案`
            let pat = format!("\n{key} = ");
            assert!(zh.contains(&pat), "zh.ftl 缺 key: {key}");
            assert!(en.contains(&pat), "en.ftl 缺 key: {key}");
        }
    }
}
