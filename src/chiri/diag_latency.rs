//! diag_latency.rs - 区块索引: [tracks] [stats] [slow] [hooks] [emit] [tests]
//! 决策时延取证：调度线程「定时等待迟到多久」「收一条事件后忙多久」，以及后台 `/proc` 扫描与
//! `dumpsys` 查询的墙钟；慢唤醒/慢处理发生时快照「扫描/查询是否正在同时进行」，用来回答
//! 「延迟与抖动有多少来自后台探测」。全部实现只读时间戳与原子计数，**不改任何判定分支**。
//!
//! 输出：`devimp/selfcost_*.log`（与 `selfcost/1` 同文件，按 `schema=` prefixed 区分），每 60s 一窗：
//! `schema=latency/1 session=s<epoch> window_start=<epoch> window_end=<epoch> track=<名> count= sum_us= max_us= p50_us= p95_us= p99_us= ovf=`
//! `wake` / `work` 追加 ` slow_us= slow= slow_scan= slow_probe= slow_both= slow_neither=`
//!
//! 口径：`wake` 只统计被定时器超时唤醒的那一轮（早到的事件唤醒不产生「迟到」量），值 = 实际等待 - 请求等待；
//! `work` = 收事件到下一轮循环开头；`p50/p95/p99` 取直方图上界（含上界），`ovf` 为 > 200ms 的样本数；
//! 计数与直方图由多线程并发自增，窗口切换瞬间的样本可能落在相邻窗口（±1 条量级），不做跨窗口一致性承诺。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

// [tracks]
/// 输出行 schema 版本（同一文件里与 `selfcost/1`、`selfcost-build/2` 共存，解析方按本字段分流）
pub const SCHEMA: &str = "latency/1";
/// 窗口时长：与 selfcost 同节拍（60s 一窗，不足一窗在会话结束时冲刷）
pub const WINDOW: Duration = Duration::from_secs(60);
/// 「慢」的门槛（微秒）：唤醒迟到 / 单次处理超过该值即计入相关性计数
pub const SLOW_US: u64 = 10_000;
/// 小于该值的定时等待不做迟到判读：`wait` 会被动态超时压到 0，那种轮次测到的是空转而不是迟到
const MIN_WAIT: Duration = Duration::from_millis(1);
/// 直方图上界（微秒），最后一档之外统一进溢出桶
const BOUNDS_US: [u64; 12] = [
    50, 100, 200, 500, 1_000, 2_000, 5_000, 10_000, 20_000, 50_000, 100_000, 200_000,
];
/// 桶数 = 上界数 + 1（溢出桶）
const NB: usize = 13;
/// 归一化到「≤ 上界」的桶下标
fn bucket(us: u64) -> usize {
    match BOUNDS_US.iter().position(|&bound| us <= bound) {
        Some(index) => index,
        None => NB - 1,
    }
}

/// 计时轨道：前两条是调度线程自身，后三条是后台探测
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    /// 调度线程定时唤醒迟到量（实际等待 - 请求等待）
    Wake,
    /// 调度线程从收到事件到下一轮循环开头的忙碌时长
    Work,
    /// 前台线程级 util 扫描（`/proc/<pid>/task` 逐线程读 stat，500ms 节流）
    ScanFg,
    /// 诊断进程快照扫描（`snapshot_procs`，仅诊断开启时每秒一次）
    ScanProcs,
    /// `dumpsys` 单次查询墙钟（含子进程启动、读取、回收）
    Probe,
}

impl Track {
    pub const ALL: [Self; 5] = [
        Self::Wake,
        Self::Work,
        Self::ScanFg,
        Self::ScanProcs,
        Self::Probe,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Wake => "wake",
            Self::Work => "work",
            Self::ScanFg => "scan_fg",
            Self::ScanProcs => "scan_procs",
            Self::Probe => "probe",
        }
    }

    /// 是否附带慢样本相关性字段（只有调度线程自身两条轨道需要）
    fn with_slow(self) -> bool {
        matches!(self, Self::Wake | Self::Work)
    }
}

// [stats]
/// 单轨道累计器：计数 + 总和 + 峰值 + 直方图；全部 relaxed，读取方只求统计意义
struct TrackStats {
    count: AtomicU64,
    sum_us: AtomicU64,
    max_us: AtomicU64,
    hist: [AtomicU64; NB],
}

impl TrackStats {
    const fn new() -> Self {
        const ZERO: AtomicU64 = AtomicU64::new(0);
        Self {
            count: ZERO,
            sum_us: ZERO,
            max_us: ZERO,
            hist: [ZERO; NB],
        }
    }

    fn record(&self, us: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_us.fetch_add(us, Ordering::Relaxed);
        self.max_us.fetch_max(us, Ordering::Relaxed);
        self.hist[bucket(us)].fetch_add(1, Ordering::Relaxed);
    }

    /// 取走本窗口累计值并清零（`swap` 逐字段进行，切换瞬间的并发样本可能落进相邻窗口）
    fn take(&self) -> Snapshot {
        let mut hist = [0_u64; NB];
        for (dst, src) in hist.iter_mut().zip(self.hist.iter()) {
            *dst = src.swap(0, Ordering::Relaxed);
        }
        Snapshot {
            count: self.count.swap(0, Ordering::Relaxed),
            sum_us: self.sum_us.swap(0, Ordering::Relaxed),
            max_us: self.max_us.swap(0, Ordering::Relaxed),
            hist,
        }
    }
}

/// 一个窗口的快照
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Snapshot {
    count: u64,
    sum_us: u64,
    max_us: u64,
    hist: [u64; NB],
}

/// 分位值：返回直方图上界（≥ 实际分位点），溢出桶回落到实测峰值
fn quantile(hist: &[u64; NB], count: u64, q: u64, max_us: u64) -> u64 {
    if count == 0 {
        return 0;
    }
    // 目标样本序号（向上取整）：第 target 个样本所在桶的上界即分位值
    let target = (count.saturating_mul(q) + 99) / 100;
    let mut cum = 0_u64;
    for (index, &bucket_count) in hist.iter().enumerate() {
        cum = cum.saturating_add(bucket_count);
        if cum >= target {
            return if index + 1 == NB {
                max_us
            } else {
                BOUNDS_US[index]
            };
        }
    }
    max_us
}

static TRACKS: [TrackStats; 5] = [
    TrackStats::new(),
    TrackStats::new(),
    TrackStats::new(),
    TrackStats::new(),
    TrackStats::new(),
];

// [slow]
/// 后台扫描在跑（计数而非布尔：嵌套/并发安全）
static SCAN_INFLIGHT: AtomicU64 = AtomicU64::new(0);
/// `dumpsys` 查询在跑
static PROBE_INFLIGHT: AtomicU64 = AtomicU64::new(0);

/// 慢唤醒统计：总数 + 与扫描/查询的四种同时发生情形
static SLOW_WAKE: AtomicU64 = AtomicU64::new(0);
static SLOW_WAKE_SCAN: AtomicU64 = AtomicU64::new(0);
static SLOW_WAKE_PROBE: AtomicU64 = AtomicU64::new(0);
static SLOW_WAKE_BOTH: AtomicU64 = AtomicU64::new(0);
static SLOW_WAKE_NEITHER: AtomicU64 = AtomicU64::new(0);

/// 慢处理统计（同构）
static SLOW_WORK: AtomicU64 = AtomicU64::new(0);
static SLOW_WORK_SCAN: AtomicU64 = AtomicU64::new(0);
static SLOW_WORK_PROBE: AtomicU64 = AtomicU64::new(0);
static SLOW_WORK_BOTH: AtomicU64 = AtomicU64::new(0);
static SLOW_WORK_NEITHER: AtomicU64 = AtomicU64::new(0);

/// 慢样本归属：在**慢事件发生时**读两个 in-flight 计数，四类互斥且合计 = 总数
fn note_slow(track: Track) {
    let (total, scan, probe, both, neither) = match track {
        Track::Wake => (
            &SLOW_WAKE,
            &SLOW_WAKE_SCAN,
            &SLOW_WAKE_PROBE,
            &SLOW_WAKE_BOTH,
            &SLOW_WAKE_NEITHER,
        ),
        _ => (
            &SLOW_WORK,
            &SLOW_WORK_SCAN,
            &SLOW_WORK_PROBE,
            &SLOW_WORK_BOTH,
            &SLOW_WORK_NEITHER,
        ),
    };
    let scanning = SCAN_INFLIGHT.load(Ordering::Acquire) > 0;
    let probing = PROBE_INFLIGHT.load(Ordering::Acquire) > 0;
    total.fetch_add(1, Ordering::Relaxed);
    match (scanning, probing) {
        (true, true) => both.fetch_add(1, Ordering::Relaxed),
        (true, false) => scan.fetch_add(1, Ordering::Relaxed),
        (false, true) => probe.fetch_add(1, Ordering::Relaxed),
        (false, false) => neither.fetch_add(1, Ordering::Relaxed),
    };
}

/// 取走慢样本计数：(总数, 扫描, 查询, 两者, 都不是)
fn take_slow(track: Track) -> (u64, u64, u64, u64, u64) {
    let (total, scan, probe, both, neither) = match track {
        Track::Wake => (
            &SLOW_WAKE,
            &SLOW_WAKE_SCAN,
            &SLOW_WAKE_PROBE,
            &SLOW_WAKE_BOTH,
            &SLOW_WAKE_NEITHER,
        ),
        _ => (
            &SLOW_WORK,
            &SLOW_WORK_SCAN,
            &SLOW_WORK_PROBE,
            &SLOW_WORK_BOTH,
            &SLOW_WORK_NEITHER,
        ),
    };
    let take = |counter: &AtomicU64| counter.swap(0, Ordering::Relaxed);
    (
        take(total),
        take(scan),
        take(probe),
        take(both),
        take(neither),
    )
}

// [hooks]
fn micros(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}

/// 调度线程定时等待迟到：`requested` 为传给 `recv_timeout` 的等待，`elapsed` 为实际墙钟
/// 只记超时唤醒（早到的事件唤醒没有「迟到」语义），`requested` 过小的轮次跳过
pub fn record_wake(requested: Duration, elapsed: Duration) {
    if requested < MIN_WAIT {
        return;
    }
    let us = micros(elapsed.saturating_sub(requested));
    TRACKS[Track::Wake as usize].record(us);
    if us >= SLOW_US {
        note_slow(Track::Wake);
    }
}

/// 调度线程处理一段：从收到事件到下一轮循环开头
pub fn record_work(elapsed: Duration) {
    let us = micros(elapsed);
    TRACKS[Track::Work as usize].record(us);
    if us >= SLOW_US {
        note_slow(Track::Work);
    }
}

#[derive(Clone, Copy)]
enum GuardKind {
    ScanFg,
    ScanProcs,
    Probe,
}

/// 作用域计时守卫：进入时登记 in-flight，离开（含 panic 展开）时落一条样本并撤销登记
pub struct Guard {
    kind: GuardKind,
    start: Instant,
}

impl Guard {
    fn new(kind: GuardKind) -> Self {
        match kind {
            GuardKind::ScanFg | GuardKind::ScanProcs => {
                SCAN_INFLIGHT.fetch_add(1, Ordering::Release);
            }
            GuardKind::Probe => {
                PROBE_INFLIGHT.fetch_add(1, Ordering::Release);
            }
        }
        Self {
            kind,
            start: Instant::now(),
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let track = match self.kind {
            GuardKind::ScanFg => Track::ScanFg,
            GuardKind::ScanProcs => Track::ScanProcs,
            GuardKind::Probe => Track::Probe,
        };
        TRACKS[track as usize].record(micros(self.start.elapsed()));
        match self.kind {
            GuardKind::ScanFg | GuardKind::ScanProcs => {
                SCAN_INFLIGHT.fetch_sub(1, Ordering::Release);
            }
            GuardKind::Probe => {
                PROBE_INFLIGHT.fetch_sub(1, Ordering::Release);
            }
        }
    }
}

/// 前台线程级 util 扫描守卫
pub fn scan_fg() -> Guard {
    Guard::new(GuardKind::ScanFg)
}

/// 诊断进程快照扫描守卫
pub fn scan_procs() -> Guard {
    Guard::new(GuardKind::ScanProcs)
}

/// `dumpsys` 单次查询守卫
pub fn probe() -> Guard {
    Guard::new(GuardKind::Probe)
}

// [emit]
/// 当前窗口（None = 未开窗；诊断关闭时不累计也不落行）
struct Window {
    session: String,
    start: Instant,
    start_epoch: u64,
}

static WINDOW_STATE: Mutex<Option<Window>> = Mutex::new(None);

fn epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn reset_tracks() {
    for track in Track::ALL {
        let _ = TRACKS[track as usize].take();
    }
    for track in [Track::Wake, Track::Work] {
        let _ = take_slow(track);
    }
}

/// 渲染一个窗口的所有行（空轨道不落行）
fn render(session: &str, window_start: u64, window_end: u64) -> Vec<String> {
    let mut lines = Vec::new();
    for track in Track::ALL {
        let snap = TRACKS[track as usize].take();
        if snap.count == 0 {
            continue;
        }
        let p50 = quantile(&snap.hist, snap.count, 50, snap.max_us);
        let p95 = quantile(&snap.hist, snap.count, 95, snap.max_us);
        let p99 = quantile(&snap.hist, snap.count, 99, snap.max_us);
        let mut line = format!(
            "schema={SCHEMA} session={session} window_start={window_start} window_end={window_end} track={} count={} sum_us={} max_us={} p50_us={p50} p95_us={p95} p99_us={p99} ovf={}",
            track.name(),
            snap.count,
            snap.sum_us,
            snap.max_us,
            snap.hist[NB - 1]
        );
        if track.with_slow() {
            let (slow, scan, probe, both, neither) = take_slow(track);
            line.push_str(&format!(
                " slow_us={SLOW_US} slow={slow} slow_scan={scan} slow_probe={probe} slow_both={both} slow_neither={neither}"
            ));
        }
        lines.push(line);
    }
    lines
}

/// 每秒调用一次（调度线程）：诊断 假→真开窗、真→假冲刷尾窗、窗口到期结算；写入在锁外进行
pub fn tick(diag_now: bool) {
    let now = Instant::now();
    let now_epoch = epoch();
    let lines = {
        let mut guard = WINDOW_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let due = matches!(
            guard.as_ref(),
            Some(window) if diag_now && now.duration_since(window.start) >= WINDOW
        );
        if diag_now && (guard.is_none() || due) {
            let lines = match guard.as_ref() {
                Some(window) => render(&window.session, window.start_epoch, now_epoch),
                None => Vec::new(),
            };
            reset_tracks();
            let session = match guard.as_ref() {
                Some(window) => window.session.clone(),
                None => format!("s{now_epoch}"),
            };
            *guard = Some(Window {
                session,
                start: now,
                start_epoch: now_epoch,
            });
            lines
        } else if !diag_now {
            // 会话结束：不足一窗也冲刷，随后关窗（与 selfcost 尾窗口径一致）
            let lines = match guard.as_ref() {
                Some(window) => render(&window.session, window.start_epoch, now_epoch),
                None => Vec::new(),
            };
            reset_tracks();
            *guard = None;
            lines
        } else {
            Vec::new()
        }
    };
    for line in lines {
        if !crate::logger::selfcost_write_line(&line) {
            log::warn!("[latency] 摘要写入失败，本行丢弃");
        }
    }
}

// [tests]
#[cfg(test)]
mod tests {
    use super::{
        BOUNDS_US, NB, SLOW_US, TRACKS, Track, bucket, quantile, record_wake, record_work, render,
        reset_tracks, take_slow,
    };
    use std::time::Duration;

    #[test]
    fn bucket_covers_bounds_and_overflow() {
        assert_eq!(bucket(0), 0);
        assert_eq!(bucket(50), 0);
        assert_eq!(bucket(51), 1);
        assert_eq!(bucket(BOUNDS_US[11]), 11);
        assert_eq!(bucket(BOUNDS_US[11] + 1), NB - 1);
    }

    #[test]
    fn quantile_returns_bucket_upper_bound() {
        let mut hist = [0_u64; NB];
        hist[3] = 90; // ≤500µs
        hist[5] = 9; // ≤2ms
        hist[7] = 1; // ≤10ms
        assert_eq!(quantile(&hist, 100, 50, 9_999), 500);
        assert_eq!(quantile(&hist, 100, 95, 9_999), 2_000);
        // 100 条样本里第 99 / 100 条分别落在 ≤2ms / ≤10ms 桶：分位取该桶上界
        assert_eq!(quantile(&hist, 100, 99, 9_999), 2_000);
        assert_eq!(quantile(&hist, 100, 100, 9_999), 10_000);
        // 溢出桶回落到实测峰值
        let mut overflow = [0_u64; NB];
        overflow[NB - 1] = 1;
        assert_eq!(quantile(&overflow, 1, 99, 777_777), 777_777);
    }

    #[test]
    fn wake_below_min_wait_is_ignored_and_slow_work_counts() {
        reset_tracks();
        record_wake(Duration::from_micros(100), Duration::from_millis(5));
        assert_eq!(TRACKS[Track::Wake as usize].take().count, 0);
        record_wake(Duration::from_millis(10), Duration::from_millis(25));
        let wake = TRACKS[Track::Wake as usize].take();
        assert_eq!(wake.count, 1);
        assert_eq!(wake.max_us, 15_000);
        record_work(Duration::from_micros(SLOW_US + 1));
        let work = TRACKS[Track::Work as usize].take();
        assert_eq!(work.count, 1);
        let (slow, scan, probe, both, neither) = take_slow(Track::Work);
        assert_eq!((slow, scan, probe, both, neither), (1, 0, 0, 0, 1));
    }

    #[test]
    fn render_skips_empty_tracks_and_splits_slow_counters() {
        reset_tracks();
        record_wake(Duration::from_millis(10), Duration::from_millis(40));
        let lines = render("s1", 100, 160);
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert!(line.starts_with(
            "schema=latency/1 session=s1 window_start=100 window_end=160 track=wake "
        ));
        assert!(line.contains("count=1"));
        assert!(
            line.contains(
                "slow_us=10000 slow=1 slow_scan=0 slow_probe=0 slow_both=0 slow_neither=1"
            )
        );
        // 取走后同窗口不再出第二条
        assert!(render("s1", 160, 220).is_empty());
    }
}
