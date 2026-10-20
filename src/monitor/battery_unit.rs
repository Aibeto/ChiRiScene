//! battery_unit.rs - 区块索引: [check] [apply] [write]
//! 启动期电流/电压倍数自检：只在放电态连续采样功率，量级明显异常时修正 divisor 并 WARN
//! 必须由 main 在 meta.yaml 自愈（sync_meta_snapshot）之后异步拉起；非放电态靠 power_supply uevent 唤醒等待

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::common;
use crate::i18n::{t, t_with_args};
use crate::monitor::telemetry;

/// 采样间隔：与遥测线程 1s 轮询同频，连续 10 次即约 10s
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
/// 非放电态的兜底轮询间隔：uevent 丢失或机型不广播 power_supply 时仍能醒来重判
const WAIT_FALLBACK: Duration = Duration::from_secs(30);
/// 连续越界命中次数
const STREAK: u32 = 15;
/// 连续正常次数：达到即判定除数正确并退出，避免放电空闲功率 < 0.1W 时误判
const NORMAL_STREAK: u32 = 10;
/// 功率下限（W）：低于此值说明除数偏大（mV/mA 原始值按 µV/µA 除）
const LOW_POWER_W: f32 = 0.1;
/// 功率上限（W）：高于此值说明除数偏小（µV/µA 原始值按 mV/mA 除）
const HIGH_POWER_W: f32 = 100.0;
/// 修正目标：节点原始单位为 mV/mA
const DIVISOR_MILLI_TEXT: &str = "1000";
const DIVISOR_MILLI: f32 = 1000.0;
/// 修正目标：节点原始单位为 µV/µA（标准 Android ABI）
const DIVISOR_MICRO_TEXT: &str = "1000000";
const DIVISOR_MICRO: f32 = 1_000_000.0;

// [check]
/// 自检主循环：先等放电，再按 1s 采样累计连续越界次数；命中一次即修正并退出
pub fn battery_unit_check(config_path: PathBuf) {
    // 非放电（充电/满电/未知/节点缺失）一律等待，只有 discharging 才取样
    while crate::chiri::read_battery_charge_state() != "discharging" {
        crate::monitor::wait_power_supply(WAIT_FALLBACK);
    }
    let mut low_streak = 0u32;
    let mut high_streak = 0u32;
    let mut normal_streak = 0u32;
    loop {
        // 充放电状态一变就清零重来：充电/满电期间的功率没有校准意义
        if crate::chiri::read_battery_charge_state() != "discharging" {
            low_streak = 0;
            high_streak = 0;
            normal_streak = 0;
            crate::monitor::wait_power_supply(WAIT_FALLBACK);
            continue;
        }
        match telemetry::telemetry().batt_power_w() {
            // 0 是占位读数不是量级证据（驱动未就绪/节点瞬时为 0），不计入低功率连击
            Some(w) if w > 0.0 && w < LOW_POWER_W => {
                low_streak += 1;
                high_streak = 0;
                normal_streak = 0;
                if low_streak >= STREAK {
                    apply(&config_path, DIVISOR_MILLI_TEXT, DIVISOR_MILLI, w, true);
                    return;
                }
            }
            Some(w) if w > HIGH_POWER_W => {
                high_streak += 1;
                low_streak = 0;
                normal_streak = 0;
                if high_streak >= STREAK {
                    apply(&config_path, DIVISOR_MICRO_TEXT, DIVISOR_MICRO, w, false);
                    return;
                }
            }
            // 正常量级：连续 NORMAL_STREAK 次即判定除数正确，退出线程（启动期自检只做一次）
            Some(w) if w > 0.0 => {
                low_streak = 0;
                high_streak = 0;
                normal_streak += 1;
                if normal_streak >= NORMAL_STREAK {
                    log::debug!(
                        "{}",
                        t_with_args(
                            "telemetry-unit-fix-ok",
                            &crate::fluent_args!("n" => NORMAL_STREAK.to_string())
                        )
                    );
                    return;
                }
            }
            // 读数不可用或占位 0：连击中断
            _ => {
                low_streak = 0;
                high_streak = 0;
                normal_streak = 0;
            }
        }
        thread::sleep(SAMPLE_INTERVAL);
    }
}

// [apply]
/// 命中后先改本次运行的原子除数（立即生效），再在非 nofix 下把两行写回 meta.yaml；
/// 写盘会触发 Chiri 的 config_watcher 热重载，Config::load 用同一组除数重新 set_battery_options，幂等
fn apply(path: &Path, divisor_text: &str, divisor: f32, power: f32, low: bool) {
    telemetry::set_battery_divisors(divisor, divisor);
    let key = if low {
        "telemetry-unit-fix-low"
    } else {
        "telemetry-unit-fix-high"
    };
    log::warn!(
        "{}",
        t_with_args(
            key,
            &crate::fluent_args!(
                "power" => format!("{power:.3}"),
                "n" => STREAK.to_string(),
                "divisor" => divisor_text.to_string()
            )
        )
    );
    if common::nofix_active() {
        log::info!("{}", t("telemetry-unit-fix-nofix"));
        return;
    }
    match write_divisors(path, divisor_text) {
        Ok(true) => {}
        Ok(false) => log::warn!("{}", t("telemetry-unit-fix-write-skip")),
        Err(e) => log::warn!(
            "{}",
            t_with_args(
                "telemetry-unit-fix-write-failed",
                &crate::fluent_args!("error" => e.to_string())
            )
        ),
    }
}

// [write]
/// 两行 divisor 一次写盘（分两次写会触发两轮热重载）：字段缺失/读写失败返回 false/Err，调用方只告警不重试
fn write_divisors(path: &Path, divisor: &str) -> std::io::Result<bool> {
    let original = std::fs::read_to_string(path)?;
    let mut text = original;
    for field in ["voltage_divisor", "current_divisor"] {
        match common::replace_top_level_value(&text, field, divisor) {
            Some(next) => text = next,
            None => return Ok(false),
        }
    }
    std::fs::write(path, text)?;
    Ok(true)
}
