//! notify.rs: [post]
// 通用通知组件：三参直发 `cmd notification post -t <标题> <渠道> <正文>`（不走 `sh -c`，参数直接传避免引号问题）
// 仅 ChiRi-FAS 渠道带拦截器：进程生命周期内前 120 条静默丢弃（防误报），第 121 条起可发，
// **投递成功**才记 (标题, 正文) 并开启 1 小时去重窗口；投递失败不记去重、进 60s 失败冷却，冷却过后可重试
// （旧实现「尝试即记账」把失败当已发，需人介入的 FAS 开环告警会被静默吞掉）；其余渠道直发不拦截

use std::process::{Command, Stdio};
use std::sync::Mutex;

use crate::fluent_args;
use crate::i18n::t_with_args;

/// `/system/bin/cmd`（Android 8+ 自带）
const CMD: &str = "/system/bin/cmd";
/// FAS 渠道：唯一带拦截器的渠道
const FAS_CHANNEL: &str = "ChiRi-FAS";
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
/// 三参直发通知。同步执行（调用方为低频事件路径，如 FAS 开环翻转沿、启动完成一次；不要在调度热循环里调用）
pub fn post_notification(title: &str, channel: &str, body: &str) {
    // [notify-switch] 刻意**不**接 `meta.notify` 总开关：该字段是已删除的「常驻状态通知」子系统的遗留
    // （meta.yaml 兼容 + WebUI 读写契约，见 `src/chiri/config.rs` 字段注释），5 个机型 meta.yaml 全部
    // 写 `notify: false`。本函数是**唯一**投递点，当前两处调用：① FAS 开环告警（需人介入的功能性
    // 失效，`ChiRi-FAS` 渠道走拦截器）；② 启动完成通知（`main.rs` 启动尾部，`ChiRi-Status` 渠道直发
    // 一次）。接上开关会让真机默认静默①，正是要消灭的静默失效。若将来要恢复总开关，须先移除各机型
    // meta.yaml 的 `notify: false` 并重新定义其用途
    let guarded = channel == FAS_CHANNEL;
    // 放行判定只读状态、不记账——成功与否要等 `cmd` 返回才知道
    if guarded && !fas_allow(title, body) {
        return;
    }
    let args = ["notification", "post", "-t", title, channel, body];
    match Command::new(CMD)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => {
            // 只有真送达才记去重（成功同时清失败冷却）
            if guarded {
                fas_record_sent(title, body);
            }
        }
        Ok(status) => {
            if guarded {
                fas_record_fail();
            }
            // `status()` 的 io 错误串不再被 `unwrap_or(false)` 吞掉，随 i18n 译文一并留痕
            let reason = match status.code() {
                Some(code) => format!("exit code {code}"),
                None => "terminated by signal".to_string(),
            };
            log_post_failure(channel, title, &reason);
        }
        Err(e) => {
            if guarded {
                fas_record_fail();
            }
            log_post_failure(channel, title, &e.to_string());
        }
    }
}

/// 投递失败留痕：走 i18n key `notify-post-failed`（占位符 `{ $channel }` / `{ $title }`），错误串附在译文之后
fn log_post_failure(channel: &str, title: &str, reason: &str) {
    log::warn!(
        "{} ({})",
        t_with_args(
            "notify-post-failed",
            &fluent_args! {"channel" => channel, "title" => title},
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
