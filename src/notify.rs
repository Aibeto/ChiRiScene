//! notify.rs: [post]
// 通用通知组件：三参交给外部脚本 `notify.sh`（模块根目录，与 `service.sh`/`action.sh` 同级）投递。
//
// [why-script] 不再由本模块自行 spawn `/system/bin/cmd`。已核实 AOSP `NotificationShellCmd` 源码：
//   - 用法是 `post [flags] <tag> <text>`，第三位置参数是 **tag** 而非「渠道」（同 tag 互相覆盖，
//     `NOTIFICATION_ID` 固定 2020）；`-c` 是 `--content-intent`（**不是**渠道），渠道 id 由 AOSP
//     硬编码为 `shell_cmd`/`Shell command`，**调用方无法指定渠道**——本仓库的「渠道」参数实际进的是 tag 位；
//   - `-o|--ongoing` 在任何版本都不存在；`-u|--user` 在 Android 13~16-release 也没有（2025 才加），
//     故脚本一律不传（传了会打 usage 且**仍然退出 0**）；
//   - **根因**：以 root（uid 0）投递时 `callingPackage` 被设成 `NotificationManagerService.ROOT_PKG`
//     （字符串 `"root"`，非真实安装的包）→ NMS `fixNotification` 里 `getApplicationInfoAsUser("root", …)`
//     抛 `NameNotFoundException` → 记 `Cannot fix notification` → **`return false`，通知被丢弃**。
//     换成 uid 2000（`com.android.shell`，真实包）才落地。故「退出码 0」不能当投递成功，须换身份；
//   - 换身份也可能静默失败：MagiskSU 的 optstring 没有 `l`，`su -lp 2000 -c …` 会停在 root。
//   候选命令行、引号转义与**逐条比对输出**都放脚本里，Rust 侧只传参。
//
// [why-output] 脚本把 `cmd` 的**输出**与「哪条策略真落地」一并回传。两条都必须看：
//   ① `cmd notification post` 出错时诊断走 stdout，而 `onCommand` 丢掉 `doNotify` 的返回值并
//      `return 0`（只有权限门返回 255）——**退出码 0 毫无意义**，只看它必然把失败报成成功；
//   ② 脚本用 AOSP 自带的 `-v`（enqueue 后回查，打 `posted:` 或
//      `warning: couldn't find notification after enqueueing`）判定真落地，只有确认落地才收下。
//   本模块按脚本退出码留痕，并把 `strategy` + `verify` 写进日志——`verify` 才是「用户看不看得到」的答案。
//
// 仅 ChiRi-FAS tag 带拦截器：进程生命周期内前 120 条静默丢弃（防误报），第 121 条起可发，
// **投递成功**才记 (标题, 正文) 并开启 1 小时去重窗口；投递失败不记去重、进 60s 失败冷却，冷却过后可重试
// （旧实现「尝试即记账」把失败当已发，需人介入的 FAS 开环告警会被静默吞掉）；其余 tag 直发不拦截

use std::sync::Mutex;

use crate::fluent_args;
use crate::i18n::t_with_args;

/// FAS 渠道：唯一带拦截器的渠道
const FAS_TAG: &str = "ChiRi-FAS";
/// ChiRi-FAS 渠道前 N 条静默丢弃（防误报）
const FAS_SILENCE_COUNT: u32 = 120;
/// FAS 相同 (标题, 正文) 去重窗口（**只有投递成功才进入该窗口**）
const FAS_DEDUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(3600);
/// 投递失败后的冷却：期间不再尝试（避免通知不可用的机型反复起进程），冷却过后允许重试
const FAS_FAIL_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);

/// ChiRi-FAS 拦截器状态：已静默计数 + 上次**成功**发送的 (标题, 正文, 时刻) + 上次失败时刻
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
/// 三参投递通知，返回是否被接受（脚本退出码 0）。同步执行（调用方为低频事件路径，如 FAS 开环翻转沿、
/// 启动完成一次；不要在调度热循环里调用）。成功/失败都在本函数内留痕，调用方按需再补充自己的上下文。
///
/// 注意 `true` 只表示「脚本报告某条策略被 cmd 接受」，**不等于用户一定看得到**（见文件头 why-script）。
pub fn post_notification(title: &str, tag: &str, body: &str) -> bool {
    // [notify-switch] 刻意**不**接 `meta.notify` 总开关：该字段是已删除的「常驻状态通知」子系统的遗留
    // （meta.yaml 兼容 + WebUI 读写契约，见 `src/chiri/config.rs` 字段注释），5 个机型 meta.yaml 全部
    // 写 `notify: false`。本函数是**唯一**投递点，当前两处调用：① FAS 开环告警（需人介入的功能性
    // 失效，`ChiRi-FAS` 渠道走拦截器）；② 启动完成通知（`main.rs` 启动尾部，`ChiRi-Status` 渠道直发
    // 一次）。接上开关会让真机默认静默①，正是要消灭的静默失效。若将来要恢复总开关，须先移除各机型
    // meta.yaml 的 `notify: false` 并重新定义其用途
    let guarded = tag == FAS_TAG;
    // 放行判定只读状态、不记账——成功与否要等脚本返回才知道
    if guarded && !fas_allow(title, body) {
        return false;
    }

    // [embed] 脚本经 `scripts::run` 从**二进制内嵌正文**释放到私有临时目录再执行：
    // 磁盘上的 `scripts/notify.sh` 不参与运行时（改了不生效，防篡改）；每次调用前覆盖一次
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
                // 成功策略号是这里最有诊断价值的一位：S1/S2 走的是 su（可见），末尾的直接投递是已知不可见的那条
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
            // 释放或 spawn 失败：脚本没能跑起来（与「跑了但投递失败」区分开留痕）
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

/// 从脚本 stdout 里取成功策略那一行（`OK S<n>`，脚本保证成功时必有）；取不到给 `-` 而不是空串，
/// 免得日志里出现「策略=」这种看不出是缺失还是空值的形态
fn strategy_of(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("OK ").map(str::trim))
        .filter(|s| !s.is_empty())
        .unwrap_or("-")
        .to_string()
}

/// 从脚本 stdout 里取验证结论那一行（`VERIFY …` 或 `LIST …`）。这一位是「用户到底看不看得到」的
/// 唯一线索——脚本的退出码只说明「某条策略被 AOSP 接受」：`VERIFY posted` 是 AOSP 自己回查确认
/// 真落地（最可信）；`VERIFY unverified` 说明老 ROM 不认 `-v`、无证据可判；`VERIFY dropped`
/// 不会走到这里（脚本已按失败返回）但保留以兼容直接调用方。留痕时不能省掉这一位
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

/// 组装失败诊断：脚本逐行输出（哪条策略、失败码、cmd 首行报错）+ stderr + 退出码，折成单行以免刷屏
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

/// 投递失败留痕：走 i18n key `notify-post-failed`（占位符 `{ $tag }` / `{ $title }`），诊断附在译文之后
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

/// ChiRi-FAS 渠道拦截——**只做放行判定，不写 `last_sent`**（成功记账见 [`fas_record_sent`]）：
/// ① 前 120 条静默丢弃；② 上次成功发送的同 (标题, 正文) 在 1 小时去重窗口内丢弃；
/// ③ 上次投递失败后 60s 冷却期内不再尝试。
/// 静默计数在此推进：第 120 条打一条 info 并丢弃，第 121 条起进入放行判定
fn fas_allow(title: &str, body: &str) -> bool {
    let mut g = FAS_GUARD.lock().unwrap_or_else(|e| e.into_inner());
    if g.silenced < FAS_SILENCE_COUNT {
        g.silenced += 1;
        // [level] 静默窗口用尽是本进程唯一一次的状态迁移（之后永久放行），留一条 info 即可；
        // 窗口内逐条丢弃刻意**不打点**——那正是防误报的目的，打点反倒把日志刷满
        if g.silenced == FAS_SILENCE_COUNT {
            log::info!(
                "[notify] FAS silence window exhausted after {} dropped, now passing through",
                FAS_SILENCE_COUNT
            );
        }
        return false;
    }
    if let Some((lt, lb, at)) = &g.last_sent {
        if lt == title && lb == body && at.elapsed() < FAS_DEDUP_WINDOW {
            // [level] 去重窗口内丢弃同样静默（与静默窗口同款「丢得无声无息」），只留 debug +
            // log_enabled 门控：本函数是同步调用，门控保证 INFO 下零 format
            if log::log_enabled!(log::Level::Debug) {
                log::debug!("[notify] FAS dedup window hit, drop: {} / {}", title, body);
            }
            return false;
        }
    }
    // 失败冷却：冷却期内不重试（避免通知不可用机型反复起进程）；冷却过后放行重试
    if g.last_fail
        .is_some_and(|at| at.elapsed() < FAS_FAIL_COOLDOWN)
    {
        if log::log_enabled!(log::Level::Debug) {
            log::debug!("[notify] FAS fail cooldown, drop: {} / {}", title, body);
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

/// 投递失败记账：只进冷却，**不写 `last_sent`**——本条未送达，下次（冷却过后）仍可重试
fn fas_record_fail() {
    FAS_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .last_fail = Some(std::time::Instant::now());
}

/// i18n 自检：确认脚本缺失与成功两条新 key 在两种语言里都存在（缺 key 时 `t()` 会回退成 key 名，
/// 单元测试能提前发现，避免日志里出现裸 key）
#[cfg(test)]
mod tests {
    use super::*;

    /// 脚本现在由 `scripts.rs` 内嵌并由 `run` 释放执行；本模块不再自行拼磁盘路径
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
        // 两种验证都没给出可判结论：不能默认成「成功」，如实记 unverified
        assert_eq!(verdict_of("OK S1"), "unverified");
    }

    #[test]
    fn notify_keys_exist() {
        // 查**编译期嵌入的 ftl 原文**而不是 t()：单测环境没加载语言包，t() 一律回退成 key 名，
        // 拿它判存在性恒假。嵌入正文才是运行期真正使用的那份（见 i18n.rs 的 embedded_ftl_str）
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
