//! cpu_monitor.rs: [consts] [helpers] [setup] [global-util] [fg-util] [telemetry] [interval] [snap-procs]
//! [tgid-util] [thread-util]

use crate::common::{DaemonEvent, ProcSnap};
use crate::utils::get_ktime_ns;
use aya::maps::{Array, HashMap as BpfHashMap, PerCpuArray};
use aya::util::online_cpus;
use aya::{Ebpf, programs::TracePoint};
use log::{debug, info, warn};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::watch;

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// [consts]
/// 常规采样周期由调用方（main.rs 按 SoC）传入：ChiRi 160ms / 非 ChiRi 200ms，见 `start_cpu_loop` 的 `sample_ms_normal`；特调采样周期（ms）：
/// 特调激活时缩短到 40ms，保证档位判定的响应度
const SAMPLE_MS_TUNED: u64 = 40;
/// 前台应用 CPU 利用率计算开关：仅 FAS 消费 foreground_max_util；禁用期间调度器忽略该字段、跳过计算省开销start_cpu_loop 入口按「ChiRi 且 FAS 配置可用」动态置位，
/// 非 ChiRi 恒 false
static FAS_FG_UTIL_ENABLED: AtomicBool = AtomicBool::new(false);

// [core-state]
/// 每核心运行时状态：与 `yumi-ebpf/src/main.rs` 的 `CoreState` 逐字段一致，两侧必须同批发布（布局不一致会读到错位数据）`#[repr(C)
/// ]` u64×3+u32×2 = 32 字节无 padding；合并单 map 后探针每次 sched_switch、用户态每采样各只查 1 次（原先 5 次）
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CoreState {
    pub last_time: u64,
    pub idle: u64,
    pub busy: u64,
    pub cur_tid: u32,
    pub cur_tgid: u32,
}

// SAFETY: `#[repr(C)]` 纯 POD（全 u32/u64、32 字节、无 padding、无 Drop），满足 aya::Pod 按字节拷贝约束
unsafe impl aya::Pod for CoreState {}

// [helpers]
/// PerCpuArray 计数 map 的全核总和（key 0 各 cpu 槽累加）；map 缺失（产物与 daemon 版本偏差）返回 0，计数保持可选语义
#[inline]
fn percpu_total(map: Option<&PerCpuArray<&mut aya::maps::MapData, u64>>) -> u64 {
    let Some(map) = map else {
        return 0;
    };
    map.get(&0u32, 0)
        .map(|v| v.iter().sum::<u64>())
        .unwrap_or(0)
}

/// 读前台进程全部 TID消费方：foreground 利用率降级路径（仅 FAS_FG_UTIL_ENABLED 置位时）与 chiri 1s 块 aff `@S` 下钻（diag_active 时每秒一次）
pub fn get_thread_tids(pid: u32) -> Vec<u32> {
    let task_dir = format!("/proc/{}/task", pid);
    let mut tids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&task_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(tid) = name.parse::<u32>() {
                    tids.push(tid);
                }
            }
        }
    }
    tids
}

// [setup]
pub async fn start_cpu_loop(
    tx: SyncSender<DaemonEvent>,
    rx_pid: watch::Receiver<u32>,
    ak_active: Arc<AtomicBool>,
    sample_ms_normal: u64,
) -> Result<(), anyhow::Error> {
// 与 fps_monitor 一致：debug 构建嵌 debug 产物，release 嵌 release 产物
    #[cfg(debug_assertions)]
    let bpf = Box::leak(Box::new(Ebpf::load(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/ebpf_target/bpfel-unknown-none/debug/yumi-ebpf"
    )))?));
    #[cfg(not(debug_assertions))]
    let bpf = Box::leak(Box::new(Ebpf::load(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/ebpf_target/bpfel-unknown-none/release/yumi-ebpf"
    )))?));
    let program: &mut TracePoint = bpf.program_mut("handle_sched_switch").unwrap().try_into()?;
    program.load()?;
    program.attach("sched", "sched_switch")?;
    info!("{}", t("cpu-monitor-started"));

// FAS 恢复前台利用率计算：仅 ChiRi 且 FAS 配置可用时置位（非 ChiRi 恒 false，调度器忽略该字段）
    if crate::common::is_chiri_soc() && crate::common::fas_available() {
        FAS_FG_UTIL_ENABLED.store(true, Ordering::Relaxed);
    }

// ChiRi 专属可选扩展探针（唤醒/迁移/频切计数，遥测用）：仅 ChiRi 尝试挂载；内核缺对应 tracepoint 时 warn 一次后跳过，不影响主探针
    let chiri_telemetry = crate::common::is_chiri_soc();
    if chiri_telemetry {
        for (name, cat, tp) in [
            ("handle_sched_wakeup", "sched", "sched_wakeup"),
            ("handle_sched_migrate_task", "sched", "sched_migrate_task"),
            // 频切探针的 tracepoint 在 **power** 子系统下，且 name 早已由 `cpufreq_transition`
            // 改名为 `cpu_frequency`（v4.6 起）——原写 ("cpufreq","cpufreq_transition") 指向
            // `/sys/kernel/tracing/events/cpufreq/cpufreq_transition/id`，该路径不存在 → 挂载恒失败
            // （status/devimp 的 freq_trans 恒 0）。类别/名称按 mainline 口径修正（机型若另有
            // 厂商 tracepoint 再另加候选）。
            ("handle_cpufreq_transition", "power", "cpu_frequency"),
        ] {
            let result = (|| -> anyhow::Result<()> {
                let prog: &mut TracePoint = bpf
                    .program_mut(name)
                    .ok_or_else(|| anyhow::anyhow!("program {name} not found in ELF"))?
                    .try_into()?;
                prog.load()?;
                prog.attach(cat, tp)?;
                Ok(())
            })();
            match result {
                Ok(()) => info!(
                    "{}",
                    t_with_args("telemetry-probe-attached", &fluent_args!("name" => name))
                ),
                Err(e) => warn!(
                    "{}",
                    t_with_args(
                        "telemetry-probe-failed",
                        &fluent_args!("name" => name, "error" => e.to_string())
                    )
                ),
            }
        }
    }

// 获取物理在线核心列表
    let online_cpus_list = online_cpus().map_err(|e| {
        anyhow::anyhow!(
            "{}",
            t_with_args(
                "cpu-monitor-online-cpus-failed",
                &fluent_args!("error" => format!("{:?}", e))
            )
        )
    })?;
    let max_cpu_id = online_cpus_list.iter().copied().max().unwrap_or(0) as usize;
    info!(
        "{}",
        t_with_args(
            "cpu-monitor-online-cpus",
            &fluent_args!("cpus" => format!("{:?}", online_cpus_list))
        )
    );

    let bpf_ptr = bpf as *mut Ebpf;

// TGID_RUN_TIME 句柄全链只读（写入只在内核侧），不对同一 MapData 活 &mut；Ebpf 已 Box::leak 常驻、地址稳定，
// snapshot_procs 从静态量重建只读句柄（见 SNAP_TGID_MAP 注释）
    let tgid_map_ref: Option<&aya::maps::Map> = unsafe { &*bpf_ptr }.map("TGID_RUN_TIME");
    SNAP_TGID_MAP.store(
        tgid_map_ref
            .map(|m| m as *const aya::maps::Map as *mut aya::maps::Map)
            .unwrap_or(std::ptr::null_mut()),
        Ordering::Release,
    );

// 逐核运行时状态（合并后单 map，布局见 CoreState）
    let core_state_map: PerCpuArray<_, CoreState> =
        PerCpuArray::try_from(unsafe { &mut *bpf_ptr }.map_mut("CORE_STATE").unwrap())?;
    let thread_run_map: BpfHashMap<_, u32, u64> =
        BpfHashMap::try_from(unsafe { &mut *bpf_ptr }.map_mut("THREAD_RUN_TIME").unwrap())?;

// TGID 级聚合运行时间 map（只读视图，同 SNAP_TGID_MAP，全链无 &mut）
    let tgid_run_map: BpfHashMap<_, u32, u64> = BpfHashMap::try_from(tgid_map_ref.unwrap())?;

// 扩展探针计数 map：可选语义——ELF 缺失（产物偏差）时计数恒 0 并 warn 一次，绝不 panic
    let fetch_counter_map =
        |name: &'static str| -> Option<PerCpuArray<&mut aya::maps::MapData, u64>> {
            match unsafe { &mut *bpf_ptr }
                .map_mut(name)
                .and_then(|m| PerCpuArray::try_from(m).ok())
            {
                Some(m) => Some(m),
                None => {
                    warn!(
                        "{}",
                        t_with_args("telemetry-map-missing", &fluent_args!("name" => name))
                    );
                    None
                }
            }
        };
    let wakeup_map = fetch_counter_map("WAKEUP_COUNT");
    let migrate_map = fetch_counter_map("MIGRATE_COUNT");
    let freq_trans_map = fetch_counter_map("FREQ_TRANS_COUNT");

// 线程级记账开关（K2）：与 FAS_FG_UTIL_ENABLED 同条件置位，只被「TGID 主路径失败」的降级路径消费，关闭时探针侧跳过 THREAD_RUN_TIME 查找/插入；旧产物缺此 map 时仅告警、
// 探针保持恒记账，版本偏差无行为差异
    if FAS_FG_UTIL_ENABLED.load(Ordering::Relaxed) {
        match unsafe { &mut *bpf_ptr }.map_mut("THREAD_ACCT") {
            Some(m) => match Array::<&mut aya::maps::MapData, u32>::try_from(m) {
                Ok(mut arr) => {
                    if let Err(e) = arr.set(0, 1u32, 0) {
                        warn!("THREAD_ACCT set failed: {e}");
                    }
                }
                Err(e) => warn!("THREAD_ACCT type mismatch: {e}"),
            },
            None => warn!(
                "THREAD_ACCT map missing in eBPF object; thread-level accounting stays enabled"
            ),
        }
    }

    tokio::spawn(async move {
        let mut rx_pid = rx_pid;
// 前台 PID 由 app_detect 在 `set_current_package` 生效处即时广播（原 500ms 轮询已删），这里只消费最新值
        let mut fg_pid: u32 = *rx_pid.borrow();
        // 根据最大 CPU ID 初始化历史记录向量，避免越界
        let mut last_idle_times = vec![0u64; max_cpu_id + 1];
        let mut last_busy_times = vec![0u64; max_cpu_id + 1];
        let mut last_check_time = get_ktime_ns();

        debug!(
            "{}",
            t_with_args(
                "cpu-monitor-baseline",
                &fluent_args!(
                    "cpus" => format!("{:?}", online_cpus_list),
                    "max_cpu" => max_cpu_id.to_string()
                )
            )
        );

// TGID 级聚合：per-PID 的 adj（raw+pending）差分基线
        let mut last_tgid_adj: u64 = 0;
let mut last_tgid_pid: u32 = 0;
// 备用：线程级数据（TGID 主路径失败时降级用）
        let mut last_thread_run: std::collections::HashMap<u32, u64> =
            std::collections::HashMap::new();

        let mut log_counter: u32 = 0;

// 扩展探针统计：2s 读一次累计计数，发周期增量事件（ChiRi 专属）
        const STATS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
        let mut last_stats_check = std::time::Instant::now();
        let mut last_wakeup_total: u64 = 0;
        let mut last_migrate_total: u64 = 0;
        let mut last_freq_total: u64 = 0;

        let mut interval =
            tokio::time::interval(std::time::Duration::from_millis(sample_ms_normal));

        loop {
            interval.tick().await;
// 消费前台 PID 广播（推送源 = app_detect 变化点，无轮询延迟；160/40ms 节奏下足够及时）
            if rx_pid.has_changed().unwrap_or(false) {
                fg_pid = *rx_pid.borrow_and_update();
            }
            let now_ktime = get_ktime_ns();
            let real_delta_ns = now_ktime.saturating_sub(last_check_time);
            last_check_time = now_ktime;

            if real_delta_ns == 0 {
                continue;
            }

            let zero_key: u32 = 0;
            // 五个逐核字段来自同一个 CoreState map：一次查找即可（原先 5 次）
            let per_cpu_state = core_state_map.get(&zero_key, 0);

            let mut core_utils = vec![0.0_f32; max_cpu_id + 1];

// [global-util]
// 1. 全局单核利用率计算（含实时状态补偿）core_utils 按真实 CPU ID 索引（长度 max_cpu_id+1）与 CLG 端一致；
// 若按在线列表顺序 push，在线核不连续（如 [0,2,4,6]）时 CLG 取错核、负载归零
            for &cpu_id in &online_cpus_list {
                let idx = cpu_id as usize;

// 单次取值：四字段同源 CoreState（map/该核缺失按 0，与原逐字段 unwrap_or(0) 一致）
                let (raw_idle, raw_busy, last_switch_time, current_tid) = per_cpu_state
                    .as_ref()
                    .ok()
                    .and_then(|v| v.get(idx))
                    .map_or((0, 0, 0, 0), |s| (s.idle, s.busy, s.last_time, s.cur_tid));

                let mut adj_idle = raw_idle;
                let mut adj_busy = raw_busy;

// 计算当前任务已积累但未提交 sched_switch 的 pending 时间（>1s 异常值剔为 0）
                let mut pending_delta = now_ktime.saturating_sub(last_switch_time);
                if pending_delta > 1_000_000_000 {
pending_delta = 0;
                }

                if current_tid == 0 {
                    adj_idle += pending_delta;
                } else {
                    adj_busy += pending_delta;
                }

                let idle_diff = adj_idle.saturating_sub(last_idle_times[idx]);
                let busy_diff = adj_busy.saturating_sub(last_busy_times[idx]);
                let total_diff = idle_diff + busy_diff;

                let util = if total_diff > 0 {
                    (busy_diff as f32 / total_diff as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };

                core_utils[idx] = util;
                last_idle_times[idx] = adj_idle;
                last_busy_times[idx] = adj_busy;
            }

// [fg-util]
// 2. 前台利用率：主路径查 1 个 TGID key（不受 thread_run_time HASH 驱逐影响）；
// FAS 禁用时 foreground_max_util 无消费方，跳过计算（见 FAS_FG_UTIL_ENABLED）
            let foreground_max_util = if FAS_FG_UTIL_ENABLED.load(Ordering::Relaxed) {
                if fg_pid == 0 {
                    0.0_f32
                } else {
// PID 切换时重置 TGID 基线，避免跨进程累计值比较
                    if fg_pid != last_tgid_pid {
                        debug!(
                            "{}",
                            t_with_args(
                                "cpu-monitor-fg-baseline-reset",
                                &fluent_args!(
                                    "old" => last_tgid_pid.to_string(),
                                    "new" => fg_pid.to_string()
                                )
                            )
                        );
                        last_tgid_adj = 0;
                        last_tgid_pid = fg_pid;
// 同时清空线程级缓存（PID 变了，旧 TID 无意义）
                        last_thread_run.clear();
                    }

                    // ── 主路径: TGID 级聚合 ──
                    let tgid_util = compute_tgid_util(
                        fg_pid,
                        &tgid_run_map,
                        &per_cpu_state,
                        &online_cpus_list,
                        now_ktime,
                        real_delta_ns,
                        &mut last_tgid_adj,
                    );

                    if let Some(util) = tgid_util {
                        util
                    } else {
// ── 降级路径：逐 TID 遍历（原始逻辑，fallback）──
                        debug!(
                            "{}",
                            t_with_args(
                                "cpu-monitor-util-fallback",
                                &fluent_args!(
                                    "pid" => fg_pid.to_string(),
                                    "raw" => tgid_run_map.get(&fg_pid, 0).unwrap_or(0).to_string()
                                )
                            )
                        );
                        compute_thread_level_util(
                            fg_pid,
                            &thread_run_map,
                            &per_cpu_state,
                            &online_cpus_list,
                            now_ktime,
                            real_delta_ns,
                            &mut last_thread_run,
                        )
                    }
                }
            } else {
                0.0_f32
            };

            log_counter += 1;
// format! 在宏外求值，log_enabled! 门控省掉 INFO 级别下的分配
            if log_counter % 25 == 0 && log::log_enabled!(log::Level::Debug) {
                let cores_str = core_utils
                    .iter()
                    .map(|u| format!("{:.0}", u * 100.0))
                    .collect::<Vec<_>>()
                    .join(", ");

                debug!(
                    "{}",
                    t_with_args(
                        "cpu-monitor-tick-log",
                        &fluent_args!(
                            "cores" => cores_str,
                            "pid" => fg_pid.to_string(),
                            "util" => format!("{:.1}", foreground_max_util * 100.0),
                            "threads" => last_thread_run.len().to_string(),
                            "delta" => (real_delta_ns / 1_000_000).to_string()
                        )
                    )
                );
            }

            if tx
                .send(DaemonEvent::SystemLoadUpdate {
                    core_utils,
                    foreground_max_util,
                })
                .is_err()
            {
                warn!("{}", t("cpu-monitor-channel-closed"));
                break;
            }

// [telemetry]
// ChiRi 遥测：读扩展探针累计计数发周期增量；探针未挂载时增量恒 0、事件照发保下游 CSV 列对齐（watchdog 不消费）
            if chiri_telemetry && last_stats_check.elapsed() >= STATS_INTERVAL {
                last_stats_check = std::time::Instant::now();
                let w = percpu_total(wakeup_map.as_ref());
                let m = percpu_total(migrate_map.as_ref());
                let f = percpu_total(freq_trans_map.as_ref());
                let stats = DaemonEvent::BpfStats {
                    wakeups: w.saturating_sub(last_wakeup_total) as u32,
                    migrations: m.saturating_sub(last_migrate_total) as u32,
                    freq_transitions: f.saturating_sub(last_freq_total) as u32,
                };
                last_wakeup_total = w;
                last_migrate_total = m;
                last_freq_total = f;
                if tx.send(stats).is_err() {
                    warn!("{}", t("cpu-monitor-channel-closed"));
                    break;
                }
            }

// [interval]
// 按特调状态切采样周期：akmode 激活 40ms 快速跟随，否则常规间隔（ChiRi 160 / 非 ChiRi 200ms）；切换时按新周期重建
            let target = if ak_active.load(Ordering::Relaxed) {
                std::time::Duration::from_millis(SAMPLE_MS_TUNED)
            } else {
                std::time::Duration::from_millis(sample_ms_normal)
            };
            if interval.period() != target {
                interval = tokio::time::interval_at(tokio::time::Instant::now() + target, target);
            }
        }
    });

    std::future::pending::<()>().await;
    Ok(())
}

// [snap-procs]
/// 读进程名：cmdline 首段优先（应用即包名；native 取文件名段），退化 /proc/<pid>/comm（15 字节截断），全失败给 "<pid>"；
/// 供 `snapshot_procs` 的 pid→name 缓存填充（命中后不再读 /proc）
fn proc_name(pid: u32) -> String {
    if let Ok(s) = std::fs::read_to_string(format!("/proc/{pid}/cmdline")) {
        if let Some(first) = s.split('\0').find(|s| !s.is_empty()) {
            let seg = first.rsplit('/').next().unwrap_or(first);
            if !seg.is_empty() {
                return seg.to_string();
            }
        }
    }
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("<{pid}>"))
}

/// @S 快照的进程级差分状态；与 foreground 基线（`last_tgid_adj`）完全独立互不干扰
#[derive(Default)]
struct SnapState {
    /// 上次快照时刻（None = 首帧：只建基线、util 全 0）
    last_at: Option<std::time::Instant>,
    /// pid → 上轮 TGID 累计运行时间（raw ns）。每帧整体滚动重建（死进程条目随之清除）
    base: std::collections::HashMap<u32, u64>,
    /// pid → 进程名缓存（/proc 每进程只读一次；超量整体清空，防长会话无界增长）
    names: std::collections::HashMap<u32, String>,
}
static SNAP_STATE: OnceLock<Mutex<SnapState>> = OnceLock::new();

/// TGID_RUN_TIME 共享只读句柄（跨线程给 `snapshot_procs`）：指向 Box::leak 常驻 Ebpf 内的 Map（生命周期 'static、地址稳定），
/// 以原始指针存静态量绕开 MapData 的 Send/Sync 约束；只做 get/keys 读取，与 monitor 循环并发读安全
static SNAP_TGID_MAP: AtomicPtr<aya::maps::Map> = AtomicPtr::new(std::ptr::null_mut());

/// 每秒进程快照供数（aff `@S` 帧）：遍历 TGID_RUN_TIME 全 map（差分基线滚动重建）产出全系统每进程 util
/// util = 窗口（≈1s）运行时间增量 / 墙钟，**百分比**、多核并行可 >100（仅 9999 上限）；首帧/新进程只建基线 util=0
/// 进程名走 `proc_name` + pid→name 缓存，稳态每帧零 /proc 名字读取；调用方自行排序取 top-N，本函数不排序不落盘
/// 仅 `diag_active()` 开启时被 chiri 主循环每秒调用（关闭零开销，开启约毫秒级）
pub fn snapshot_procs() -> Vec<ProcSnap> {
    let ptr = SNAP_TGID_MAP.load(Ordering::Acquire);
    if ptr.is_null() {
// start_cpu_loop 尚未注册句柄 / map 缺失：无数据可采
        return Vec::new();
    }
// SAFETY: 指针指向 Box::leak 常驻 Ebpf 内的 Map（见 SNAP_TGID_MAP），生命周期覆盖进程；只构造只读句柄，不写入
    let map: &aya::maps::Map = unsafe { &*ptr };
    let Ok(tgid_run_map) = BpfHashMap::<&aya::maps::MapData, u32, u64>::try_from(map) else {
        return Vec::new();
    };
    let mut st = SNAP_STATE
        .get_or_init(|| Mutex::new(SnapState::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    let win_ns = st
        .last_at
        .map(|t| now.duration_since(t).as_nanos() as u64)
        .unwrap_or(0);
    st.last_at = Some(now);
    if st.names.len() > 4096 {
        st.names.clear();
    }

    let mut out = Vec::new();
    let mut next_base = std::collections::HashMap::new();
    // aya 0.14：keys() 直接返回 MapKeys 迭代器（逐项 Result）
    for k in tgid_run_map.keys().flatten() {
        let raw = tgid_run_map.get(&k, 0).unwrap_or(0);
        // raw 单调递增（BPF 侧只 += delta）；回退（map 重建等）按首见处理
        let util = match st.base.get(&k) {
            Some(&prev) if win_ns > 0 && raw >= prev => (raw - prev) as f64 / win_ns as f64 * 100.0,
            _ => 0.0,
        };
        next_base.insert(k, raw);
        let comm = match st.names.get(&k) {
            Some(n) => n.clone(),
            None => {
                let n = proc_name(k);
                st.names.insert(k, n.clone());
                n
            }
        };
        out.push(ProcSnap {
            pid: k,
            comm,
            util: util.min(9999.0) as f32,
        });
    }
    st.base = next_base;
    out
}

// [tgid-util]
/// 主路径：TGID 级聚合 map 计算前台利用率只查 1 个 key、map 容量 1024 足够，
/// 完全规避 thread_run_time HASH 容量不足 / LRU 驱逐问题
/// 关键设计：基线与本轮同口径 adj = raw + pending，util = adj 差分 / 墙钟，与降级路径一致；
/// 旧「raw 差分 + 当前 pending」会把上一轮 pending 中未提交时段重复计入，前台 util 系统性高估、FAS 输入失真
/// 返回 None 表示走降级路径仅 FAS_FG_UTIL_ENABLED 置位时被 foreground 计算调用
fn compute_tgid_util(
    fg_pid: u32,
    tgid_run_map: &BpfHashMap<&aya::maps::MapData, u32, u64>,
    per_cpu_state: &Result<aya::maps::PerCpuValues<CoreState>, aya::maps::MapError>,
    online_cpus: &[u32],
    now_ktime: u64,
    real_delta_ns: u64,
    last_tgid_adj: &mut u64,
) -> Option<f32> {
// 读取 TGID 累计运行时间（BPF 侧只在 sched_switch 更新）
    let raw_tgid_time = tgid_run_map.get(&fg_pid, 0).unwrap_or(0);

    // 如果 TGID 在 map 中完全不存在，且没有历史基线
    if raw_tgid_time == 0 && *last_tgid_adj == 0 {
        return None;
    }

// pending：正在核心上运行但未经过 sched_switch 的时间（瞬时快照值，随 adj 进入基线差分，不单独累积）
    let mut current_pending: u64 = 0;
    if let Ok(states) = per_cpu_state.as_ref() {
        for &cpu_id in online_cpus {
            let idx = cpu_id as usize;
            let Some(s) = states.get(idx) else { continue };

            if s.cur_tgid == fg_pid {
                let pending = now_ktime.saturating_sub(s.last_time);
                if pending < 1_000_000_000 {
                    current_pending += pending;
                }
            }
        }
    }

// adj = raw + pending：此刻该 TGID 已消耗的总 CPU 时间（含未提交时段）
    let adj = raw_tgid_time + current_pending;
    let prev_adj = *last_tgid_adj;
    *last_tgid_adj = adj;

    if prev_adj == 0 {
// 首次采样（PID 刚切换或首次运行），只建基线
        return Some(0.0);
    }

// adj 理论单调增；回退 = map 重置/条目驱逐重建，或 pending>1s 被守卫剔除的小幅回退，均本轮记 0
    if adj < prev_adj {
        return Some(0.0);
    }

    // 总增量 = adj 差分 = raw 增量 + pending 增量（不是 + 当前 pending）
    let total_delta = adj - prev_adj;

// 利用率 = 总增量 / 墙钟时间
    let util = (total_delta as f32 / real_delta_ns as f32).clamp(0.0, 1.0);

    Some(util)
}

// [thread-util]
/// 降级路径：逐 TID 遍历取最重线程利用率（原始逻辑）；含防驱逐保护（返回值 < 上次记录则跳过该 TID）仅 FAS_FG_UTIL_ENABLED 置位且 TGID 主路径失败时被调用
fn compute_thread_level_util(
    fg_pid: u32,
    thread_run_map: &BpfHashMap<&mut aya::maps::MapData, u32, u64>,
    per_cpu_state: &Result<aya::maps::PerCpuValues<CoreState>, aya::maps::MapError>,
    online_cpus: &[u32],
    now_ktime: u64,
    real_delta_ns: u64,
    last_thread_run: &mut std::collections::HashMap<u32, u64>,
) -> f32 {
    let tids = get_thread_tids(fg_pid);
    let mut max_util: f32 = 0.0;
    let mut current_thread_run = std::collections::HashMap::with_capacity(tids.len());
    for &tid in &tids {
        let mut adj_thread_time = thread_run_map.get(&tid, 0).unwrap_or(0);

// 线程正在某核上跑：补上其 pending delta
        for &cpu_id in online_cpus {
            let idx = cpu_id as usize;
            let Some(s) = per_cpu_state.as_ref().ok().and_then(|v| v.get(idx)) else {
                continue;
            };

            if s.cur_tid == tid {
                let pending_delta = now_ktime.saturating_sub(s.last_time);
                if pending_delta < 1_000_000_000 {
                    adj_thread_time += pending_delta;
                }
            }
        }

        current_thread_run.insert(tid, adj_thread_time);

        if let Some(&last_run) = last_thread_run.get(&tid) {
// 防驱逐：新值 < 旧值说明 HASH 条目被驱逐重建、数据不连续，跳过此 TID 本轮计算
            if adj_thread_time >= last_run {
                let thread_delta = adj_thread_time - last_run;
                let util = (thread_delta as f32 / real_delta_ns as f32).clamp(0.0, 1.0);
                if util > max_util {
                    max_util = util;
                }
            }
        }
    }

    *last_thread_run = current_thread_run;
    max_util
}
