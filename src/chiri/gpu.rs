//! gpu.rs: [detect] [types] [lock] [release]GPU 频率锁（contingency 使用）：启动时探测一次 devfreq 节点，能确定硬件最高频的才收录；
//! lock 时快照当前min/max 并锁 min=max=硬件最高频，release 按快照恢复；探测不到任何可用节点保持空表、lock 为 no-op（只打一次 warn）——GPU 节点各 SoC 差异很大，
//! 宁可不管也不乱写。节点覆盖：-高通 Adreno /sys/class/kgsl/kgsl-3d0/`（硬件上限 `max_gpu_clk`，写入走同名 devfreq）
//! + 通用 devfreq /sys/class/devfreq/` 下名字含 gpu / kgsl / mali 的设备
use crate::utils::FastWriter;
use log::{debug, error, info, warn};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

const GPU_DEVFREQ_ROOT: &str = "/sys/class/devfreq";
const ADRENO_DIR: &str = "/sys/class/kgsl/kgsl-3d0";
const ADRENO_MAX_CLK: &str = "/sys/class/kgsl/kgsl-3d0/max_gpu_clk";

fn read_u32(path: &str) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_freq_range(dir: &str) -> Option<(u32, u32)> {
    // available_frequencies 是唯一可信来源：max 是硬件上限、min 是硬件最低频（release 残留防护的兜底恢复值）；devfreq 的 max_freq/min_freq 只是当前限值
    let mut freqs: Vec<u32> = fs::read_to_string(format!("{dir}/available_frequencies"))
        .ok()?
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    freqs.sort_unstable();
    freqs.dedup();
    Some((*freqs.first()?, *freqs.last()?))
}

/// devimp 快照全 "-" 只报一次：这是硬件事实（机型有没有 GPU 节点），不会变
static SNAPSHOT_EMPTY_LOGGED: AtomicBool = AtomicBool::new(false);

// [detect]
// 「有 GPU 目录却全部被拒」是最需要看见的失效态。探测在进程内只跑一次（GpuGuard::new 仅 mod 一处调用），
// 候选目录有限、逐条如实报不会刷屏，故不做去重
/// 排除某个候选目录并记录原因（逐目录逐原因各报一条）
fn reject(dir: &str, reason: String) {
    warn!(
        "{}",
        t_with_args(
            "gpu-probe-rejected",
            &fluent_args!("path" => dir.to_string(), "reason" => reason)
        )
    );
}

/// 收录一个 devfreq 目录：写入器有效且能确定 hw_max/hw_min 才要
fn probe_devfreq(dir: &str, out: &mut Vec<GpuNode>) {
    // available_frequencies 缺失时，Adreno 用 max_gpu_clk 兜底硬件上限
    let (hw_min, hw_max) = match read_freq_range(dir) {
        Some(r) => r,
        None if dir.contains("kgsl") => match read_u32(ADRENO_MAX_CLK) {
            Some(hw_max) => (
                read_u32(&format!("{dir}/min_freq")).unwrap_or(hw_max),
                hw_max,
            ),
            None => {
                reject(
                    dir,
                    format!("无 available_frequencies，且 {ADRENO_MAX_CLK} 读不到硬件上限"),
                );
                return;
            }
        },
        None => {
            reject(dir, "读不到 available_frequencies".to_string());
            return;
        }
    };
    if hw_max == 0 || hw_min == 0 || hw_min > hw_max {
        reject(dir, format!("频点表非法 hw_min={hw_min} hw_max={hw_max}"));
        return;
    }
    let max_writer = FastWriter::new(format!("{dir}/max_freq"));
    let min_writer = FastWriter::new(format!("{dir}/min_freq"));
    if !max_writer.is_valid() {
        reject(dir, format!("{dir}/max_freq 不可写（含卸载重试）"));
        return;
    }
    let min_writer = min_writer.is_valid().then_some(min_writer);
    out.retain(|n: &GpuNode| n.label != dir);
    out.push(GpuNode {
        label: dir.to_string(),
        max_writer,
        min_writer,
        orig_max: fs::read_to_string(format!("{dir}/max_freq"))
            .ok()
            .map(|s| s.trim().to_string()),
        orig_min: fs::read_to_string(format!("{dir}/min_freq"))
            .ok()
            .map(|s| s.trim().to_string()),
        hw_min,
        hw_max,
    });
}

// [types]
struct GpuNode {
    label: String,
    max_writer: FastWriter,
    min_writer: Option<FastWriter>,
    /// 快照：恢复时按原样写回（含内核格式差异，不做数值规整）
    orig_max: Option<String>,
    orig_min: Option<String>,
    hw_min: u32,
    hw_max: u32,
}

/// 两条持续性失败各用各的标志：共用会让「锁不上」 silencing 掉后来的「恢复不回去」，而两者后果不同
static LOCK_FAILED_LOGGED: AtomicBool = AtomicBool::new(false);
static RELEASE_STUCK_LOGGED: AtomicBool = AtomicBool::new(false);

/// [critical] 关键写失败且已判定为持久性失败：功能已丢失（未锁上 / 已残留），**不会重试**，打 error
fn error_critical_once(flag: &AtomicBool, key: &str, args: &fluent::FluentArgs) {
    if flag.swap(true, Ordering::Relaxed) {
        return;
    }
    error!("{}", t_with_args(key, args));
}

pub struct GpuGuard {
    nodes: Vec<GpuNode>,
    active: bool,
}

/// devimp snap 行用：GPU 各节点的**实际**当前频率 / 上下限 / 调速器。多节点以 `;` 分隔，每项 `<设备名>:<值>`，读不到写 `-`
/// **只在 devimp 开启时调用**（每秒一次 sysfs 读），故不做缓存——值会被内核 devfreq 与其它进程改动节点口径与探测一致（Adreno kgsl-3d0) + 通用 devfreq gpu/kgsl/mali）
/// **当前未被调用**：`chiri/mod.rs` 的 `GPU_SNAPSHOT_ENABLED` 默认 false——读 GPU 节点会拉起 GPU 出低功耗（8550 实测同场景功耗 +47%），保留实现待该问题有解或确需数据时再开
#[allow(dead_code)]
pub fn devfreq_snapshot() -> (String, String, String, String) {
    // 目录按**设备名**去重且标准 devfreq 优先：kgsl-3d0 会同时出现在 /sys/class/devfreq/ 与 /sys/class/kgsl/ 下，
    // 两处节点语义不同（标准 devfreq 接口 vs Adreno 私有接口），混读会把 min/cur 拼成互不相干的数(dir, 是否标准 devfreq)
    let mut dirs: Vec<(String, bool)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir(GPU_DEVFREQ_ROOT) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.contains("gpu") || name.contains("kgsl") || name.contains("mali") {
                let dir = entry.path().to_string_lossy().to_string();
                seen.push(name);
                dirs.push((dir, true));
            }
        }
    }
    if std::path::Path::new(ADRENO_DIR).exists() {
        let lb = ADRENO_DIR
            .rsplit('/')
            .next()
            .unwrap_or(ADRENO_DIR)
            .to_string();
        if !seen.contains(&lb) {
            dirs.push((ADRENO_DIR.to_string(), false));
        }
    }

    // 读频率并**统一成 kHz**：devfreq 的 cur_freq 与 Adreno 的 gpuclk 都以 Hz 计，直接写日志会得到荒谬值（与 CPU 的 kHz 口径也对不上）；
    // 已是 kHz 的厂商节点（数值小）保持原样
    // 返回 None 表示**该字段在本机型上读不到**（与「读到了但值是空」区分开）：外层靠它区分「节点缺失」与「GPU 空闲」
    let read_freq = |dir: &str, names: &[&str]| -> Option<String> {
        for n in names {
            if let Ok(s) = fs::read_to_string(format!("{dir}/{n}")) {
                let s = s.trim();
                if s.is_empty() {
                    continue;
                }
                return Some(match s.parse::<u64>() {
                    Ok(v) if v > 10_000_000 => (v / 1_000).to_string(), // Hz -> kHz
                    _ => s.to_string(),
                });
            }
        }
        None
    };
    // 调速器：只在标准 devfreq 下读（Adreno 私有目录里的同名节点可能是频率值，不是 governor 名）；读到的内容整体是数字也判定无效
    let read_gov = |dir: &str, is_devfreq: bool| -> Option<String> {
        if !is_devfreq {
            return None;
        }
        match fs::read_to_string(format!("{dir}/governor")) {
            Ok(s) => {
                let s = s.trim();
                if s.is_empty() || s.chars().all(|c| c.is_ascii_digit()) {
                    None
                } else {
                    Some(s.to_string())
                }
            }
            Err(_) => None,
        }
    };

    let (mut cur, mut max, mut min, mut gov) =
        (String::new(), String::new(), String::new(), String::new());
    // 任一字段读到过值就置 true：用于区分「本机型没有可读节点」与「读到了真实值」
    let mut any_ok = false;
    for (dir, is_devfreq) in dirs {
        let label = dir.rsplit('/').next().unwrap_or("gpu").to_string();
        let push = |dst: &mut String, v: Option<String>| {
            if !dst.is_empty() {
                dst.push(';');
            }
            // 读不到照旧写 "-"（devimp 列口径不变）
            dst.push_str(&format!("{label}:{}", v.unwrap_or_else(|| "-".to_string())));
        };
        let vals = [
            read_freq(&dir, &["cur_freq", "gpuclk", "clock"]),
            read_freq(&dir, &["max_freq", "max_gpuclk"]),
            read_freq(&dir, &["min_freq"]),
        ];
        any_ok |= vals.iter().any(|v| v.is_some());
        let [c, mx, mn] = vals;
        push(&mut cur, c);
        push(&mut max, mx);
        push(&mut min, mn);
        push(&mut gov, read_gov(&dir, is_devfreq));
    }
    // [critical] 同形防护：读不到就写 "-"，于是「GPU 节点不存在 / 全部读失败」与「GPU 空闲或快照未开启」
    // 在 devimp 上完全同形，真机排查无法区分。只报一次（有无节点是硬件事实，不会变）
    if !any_ok && !SNAPSHOT_EMPTY_LOGGED.swap(true, Ordering::Relaxed) {
        warn!("{}", t("gpu-snapshot-unavailable"));
    }
    (cur, max, min, gov)
}

impl GpuGuard {
    /// 启动时构造并探测一次；探测结果缓存，之后不再扫文件系统
    pub fn new() -> Self {
        let mut nodes = Vec::new();
        if std::path::Path::new(ADRENO_DIR).exists() {
            probe_devfreq(ADRENO_DIR, &mut nodes);
        }
        if let Ok(entries) = fs::read_dir(GPU_DEVFREQ_ROOT) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                let dir = entry.path().to_string_lossy().to_string();
                if name.contains("gpu") || name.contains("kgsl") || name.contains("mali") {
                    probe_devfreq(&dir, &mut nodes);
                }
            }
        }
        if nodes.is_empty() {
            warn!("{}", t("gpu-detect-miss"));
        }
        Self {
            nodes,
            active: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    // [lock]
    /// min=max 锁到硬件最高频先升 max 再升 min（避免 min>max 被内核拒绝）重复激活/无节点均 no-op（探测已打过 warn）
    pub fn lock(&mut self) {
        if self.active || self.nodes.is_empty() {
            return;
        }
        let mut ok = 0usize;
        for node in &mut self.nodes {
            if !node.max_writer.write_value_force(node.hw_max) {
                continue;
            }
            if let Some(min) = &mut node.min_writer {
                min.write_value_force(node.hw_max);
            }
            ok += 1;
        }
        // 写失败细节由 FastWriter 自己按 errno 分级打（EINVAL/EBUSY 是瞬态、已 debug），这里只报**后果**
        if ok == 0 {
            // [critical] 一个节点都没锁上：contingency 的 GPU 锁频能力丢失，且 lock 只在模式切换时调用一次、无重试——
            // 故障态与正常态同形（都是没锁上），不打这条真机上看不出来
            error_critical_once(
                &LOCK_FAILED_LOGGED,
                "gpu-lock-failed",
                &fluent_args!("nodes" => self.nodes.len().to_string()),
            );
            return;
        }
        if ok < self.nodes.len() {
            // 部分成功：能力还在，属降级
            warn!(
                "{}",
                t_with_args(
                    "gpu-lock-partial",
                    &fluent_args!(
                        "ok" => ok.to_string(),
                        "total" => self.nodes.len().to_string(),
                    ),
                )
            );
        }
        self.active = true;
        info!(
            "{}",
            t_with_args(
                "gpu-locked",
                &fluent_args!(
                    "khz" => self.nodes.first().map(|n| n.hw_max).unwrap_or(0).to_string(),
                    "nodes" => ok.to_string(),
                ),
            )
        );
    }

    // [release]
    /// 按快照恢复原 min/max。先降 min 再降 max（避免 max<min 被内核拒绝）
    pub fn release(&mut self) {
        if !self.active {
            return;
        }
        let mut max_failed = 0usize;
        for node in &mut self.nodes {
            if let (Some(min), Some(orig)) = (&mut node.min_writer, &node.orig_min) {
                // 残留防护：orig_min 若等于 hw_max（上次 SIGKILL 残留 min=max 锁频），恢复硬件最低频而不是把残留值当「原值」固化
                let restore_min = match orig.parse::<u32>() {
                    Ok(v) if v != node.hw_max => v,
                    _ => node.hw_min,
                };
                min.write_value_force(restore_min);
            }
            // 只统计 max 写失败：min 没写回的后果只是地板偏低（可降级），max 卡在 hw_max 就是**永久锁最高频**
            if let Some(orig) = &node.orig_max {
                if let Ok(v) = orig.parse::<u32>() {
                    if !node.max_writer.write_value_force(v) {
                        max_failed += 1;
                    }
                }
            }
        }
        // 写失败细节由 FastWriter 自己按 errno 分级打；这里只报恢复不完整的**后果**
        if max_failed > 0 {
            // [critical] 写入本身可能只是瞬态失败，但 release 是唯一一次机会：下面 active 已置 false，
            // 之后 release 直接 return，再无人重试写回。状态卡在 min=max=hw_max（耗电/发热）且无任何标志反映——必须 error
            error_critical_once(
                &RELEASE_STUCK_LOGGED,
                "gpu-release-max-stuck",
                &fluent_args!("nodes" => max_failed.to_string()),
            );
        } else if log::log_enabled!(log::Level::Debug) {
            // 残留防护生效时说明上次是被 SIGKILL 打断的，属异常但已自愈；常态不出现，走 debug 免打扰
            if self.nodes.iter().any(|n| {
                n.orig_min
                    .as_ref()
                    .and_then(|s| s.parse::<u32>().ok())
                    .map(|v| v == n.hw_max)
                    .unwrap_or(false)
            }) {
                debug!("[GPU] 释放时命中残留防护：orig_min 等于 hw_max，已按硬件最低频恢复");
            }
        }
        self.active = false;
        info!("{}", t("gpu-released"));
    }
}
