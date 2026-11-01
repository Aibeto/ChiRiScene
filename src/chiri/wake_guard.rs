//! wake_guard.rs: [probe] [hold] [release]
//! 息屏保活锁（2026-11-01）：三份 minidump（10-31 16:40 / 23:40、11-01 08:53）在 sm8750（OnePlus 13 / PJZ110）
//! 上同型撞线——`qcom_wdt` 只给挂起路径约 6.18 s 的窗口（pet→bark 实测 6.35–6.39 s = vWdog 206 tick × 30.9 ms），
//! 一旦 `dpm_suspend_start` 全序列超过它就 bark → `Kernel panic - not syncing: Handle a watchdog bite!`；
//! 挂在 `cpuidle_enter_s2idle` 的那个 CPU 正是被 watchdog 点名的 CPU。
//! ChiRi 在挂起期零动作（主线程停在 futex、脚本线程停在 `__refrigerator`），修不了驱动/PM 路径，只能兜底：
//! **息屏期间持一把系统 wake_lock**，PowerManager 的 `state_store` 因 `pm_wakeup_pending()` 直接吃 -EBUSY，
//! 系统根本走不到那条 6 s 的挂起路径。代价是息屏不再进 s2idle（待机功耗上升），
//! 故**默认关**，仅 sm8750 默认开启（meta.yaml `wake_guard_enabled`，缺键按机型判定，见 common::default_wake_guard_enabled）。
//!
//! 刻意不写内核超时（`<name> <timeout>`）：这个 sysfs 的超时单位在不同内核分支上是秒还是毫秒并无保证，
//! 写错只会「静默提前释放」——比不写更难发现。改为本模块自管：启动先释放一次清陈旧锁（上一实例被 SIGKILL 时锁会留到重启），
//! 屏幕事件与配置热重载对齐目标态，设备重启即彻底清除。
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::fluent_args;
use crate::i18n::t_with_args;

// [probe]
/// 保活锁节点：内核开 `CONFIG_PM_WAKELOCKS` 才有，部分 GKI 设备不带——缺节点即本模块整体停用
const WAKE_LOCK: &str = "/sys/power/wake_lock";
const WAKE_UNLOCK: &str = "/sys/power/wake_unlock";
/// 锁名：内核按名字去重，同名重复写只是续命，不会叠出两把
const LOCK_NAME: &str = "chiri_screen_off";

/// 节点可用性：只探测一次并缓存（屏幕每次翻转都探一遍 path 是白花 syscall）
static AVAILABLE: OnceLock<bool> = OnceLock::new();
/// 当前是否持锁：唯一事实来源，状态没变就不碰 sysfs
static HELD: AtomicBool = AtomicBool::new(false);
/// 写失败只告警一次（失败原因通常是 SELinux 策略，屏幕状态反复翻转会重复触发同一条告警）
static WARNED: AtomicBool = AtomicBool::new(false);

// [sysfs]
/// 写 sysfs 属性：刻意用裸 `std::fs::write` —— 不走 `utils::write_sysfs`（它的 `enable_perm` 会 chmod 0664，
/// `/sys/power` 下的属性权限由内核固定，chmod 必然失败且无意义），也不走 `write_to_file`（写完改 0444，
/// 下次写入直接 EACCES）。返回值交给调用方按 errno 分类打点。
fn write_node(path: &str, value: &str) -> std::io::Result<()> {
    std::fs::write(path, value)
}

/// 节点是否可用（探测一次）：不可用时打一条 info 说明本机没有这套机制，功能静默停用而不是报错
fn available() -> bool {
    *AVAILABLE.get_or_init(|| {
        let ok = Path::new(WAKE_LOCK).exists() && Path::new(WAKE_UNLOCK).exists();
        if !ok {
            log::info!(
                "{}",
                t_with_args(
                    "scheduler-wake-guard-unavailable",
                    &fluent_args!("node" => WAKE_LOCK.to_string())
                )
            );
        }
        ok
    })
}

// [hold]
/// 持锁（息屏）：已持锁直接返回（幂等）；节点缺失/写入被拒都只留痕不改状态，调度其余部分照常
pub fn acquire(reason: &str) -> bool {
    if !available() || HELD.load(Ordering::Acquire) {
        return false;
    }
    match write_node(WAKE_LOCK, LOCK_NAME) {
        Ok(()) => {
            HELD.store(true, Ordering::Release);
            WARNED.store(false, Ordering::Release);
            log::info!(
                "{}",
                t_with_args(
                    "scheduler-wake-guard-hold",
                    &fluent_args!("lock" => LOCK_NAME.to_string(), "reason" => reason.to_string())
                )
            );
            true
        }
        Err(e) => {
            if !WARNED.swap(true, Ordering::AcqRel) {
                log::warn!(
                    "{}",
                    t_with_args(
                        "scheduler-wake-guard-failed",
                        &fluent_args!(
                            "node" => WAKE_LOCK.to_string(),
                            "error" => e.to_string()
                        )
                    )
                );
            }
            false
        }
    }
}

// [release]
/// 释放（亮屏/停摆/启动清陈旧/退出）：**不看 HELD 也要写一次**——上一个实例被 SIGKILL 时锁会留在内核里，
/// 本实例的内存状态是「没持锁」，只有盲写一次 wake_unlock 才清得掉
pub fn release(reason: &str) -> bool {
    if !available() {
        return false;
    }
    let held = HELD.swap(false, Ordering::AcqRel);
    match write_node(WAKE_UNLOCK, LOCK_NAME) {
        Ok(()) => {
            // 写成功 = 内核里确实有这把锁（含上一实例遗留），必须留痕，否则「按了关却还在耗电」无从追查
            log::info!(
                "{}",
                t_with_args(
                    "scheduler-wake-guard-release",
                    &fluent_args!("lock" => LOCK_NAME.to_string(), "reason" => reason.to_string())
                )
            );
            held
        }
        // EINVAL = 这个名字当前没被持有（内核直接拒），是正常路径：首次启动、重复释放都会走到，不告警
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => held,
        Err(e) => {
            if !WARNED.swap(true, Ordering::AcqRel) {
                log::warn!(
                    "{}",
                    t_with_args(
                        "scheduler-wake-guard-failed",
                        &fluent_args!(
                            "node" => WAKE_UNLOCK.to_string(),
                            "error" => e.to_string()
                        )
                    )
                );
            }
            held
        }
    }
}

/// 对齐到目标态：`hold = true` 持锁、false 释放；调用方只表达「该不该持」，幂等由本模块保证
/// 返回 true 表示本次真的写了节点（发生状态变化）
pub fn sync(hold: bool, reason: &str) -> bool {
    if hold {
        acquire(reason)
    } else {
        release(reason)
    }
}
