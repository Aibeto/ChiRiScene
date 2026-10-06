//! fps_monitor.rs: [consts] [probe] [identity] [manager] [session_tests] [loop] [attach_stats] [gate] [pid-switch] [poll]

use std::collections::{HashMap, VecDeque};
use std::mem::size_of;
use std::num::NonZeroU32;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::ptr;
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::{Duration, Instant};

use aya::Ebpf;
use aya::maps::RingBuf;
use aya::programs::UProbe;
use aya::programs::uprobe::{UProbeAttachLocation, UProbeAttachPoint, UProbeScope};
use log::{debug, info, warn};
use mio::{Events, Interest, Poll, Token, unix::SourceFd};
use tokio::sync::watch;

use crate::common::DaemonEvent;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};
use crate::monitor::{FasSignal, FrameSource};

// [consts]

/// uprobe 符号名（短签名）
const SYMBOL_SHORT: &str = "_ZN7android7Surface11queueBufferEP19ANativeWindowBufferi";
/// uprobe 符号名（长签名，fallback）
const SYMBOL_LONG: &str =
    "_ZN7android7Surface11queueBufferEP19ANativeWindowBufferiPNS_24SurfaceQueueBufferOutputE";
const LIBGUI_PATH: &str = "/system/lib64/libgui.so";

/// `android::Surface::queueBuffer` 的 mangled 前缀：类名/方法名固定，参数签名随 Android 版本变化，硬编码短/长名在新机型都可能解析失败（FAS 收不到帧、档位不动）；
/// 按前缀扫描，签名变化无感
const SYMBOL_MANGLED_PREFIX: &str = "_ZN7android7Surface11queueBufferE";

/// attach 失败退避（秒）：符号类故障按档位递增，之后钳在上限
const RETRY_BACKOFF_SECS: &[u64] = &[1, 2, 5];
/// libgui 无该符号（帧源永久不可用）时的退避：重试要读并解析 libgui ELF，60s 一次足够
const RETRY_BACKOFF_NO_SYMBOL_SECS: u64 = 60;
/// 无探针时的 poll 超时上限（退避窗口内按此周期醒来，不做 attach）
const IDLE_POLL_MAX_MS: u64 = 1_000;
/// [attach_stats] attach 失败摘要的最小间隔：主循环按事件驱动（有帧即刻唤醒，迭代间隔不固定），
/// 节流基准必须是墙钟而不是迭代计数（计数会随帧率变化，密集时可远快于预期）
const ATTACH_STATS_INTERVAL: Duration = Duration::from_secs(4);

/// RingBuf 输出的帧时间戳事件（与 yumi-ebpf 的 FrameTimestampEvent 内存布局一致）
#[repr(C)]
struct FrameTimestampEvent {
    pid: u32,
    ktime_ns: u64,
}

const MIN_FRAME_NS: u64 = 1_000_000;
const MAX_FRAME_NS: u64 = 200_000_000;
const FRAMETIME_WINDOW: usize = 144;

// [play_hist]
// 播放态旁路帧间隔直方图（2026-09-27，P2-3）：视频没有「应用申报的目标帧率」可对，且同一进程多 Surface 会混流
//（视频层 24/30fps + 弹幕层 120Hz）——在线判 jank、判 fps 列都无从谈起，故只按秒落帧间隔直方图，由离线按
//「占优簇反推基线 + 相对劣化」判读（口径见 `.cursor/commands/devimp-log-analysis.md` 播放态小节）。
// 落点复用 main_ 通道的 event 行（decision=playback_fps、reason=直方图），不新建 devimp 文件/目录
/// 直方图档数：每档 4ms。**实际覆盖上限 = `MAX_FRAME_NS`(200ms)**（探针侧只让 `[MIN_FRAME_NS,
/// MAX_FRAME_NS]` 的间隔进来，>200ms 的帧整帧丢弃、既不进直方图也不进 `n`）→ 档号 50 以后恒为空，
/// 离线长尾率的**分母不含** >200ms 的严重卡顿（2026-09-28 审查发现注释原写"覆盖 4ms–1020ms"与
/// 过滤范围不符）。若日后要让直方图覆盖到 1020ms，需给直方图单列上限而**不能**抬高 `MAX_FRAME_NS`
/// ——后者同时喂 FAS 的 `frametimes`（把 >200ms 的极端间隔算进窗口会拉低 avg_fps、引发过度降档）
const PLAY_HIST_BINS: usize = 256;
/// 单档宽度（ns）
const PLAY_HIST_BIN_NS: u64 = 4_000_000;
/// 统计窗口：到点落一行；窗口内无帧（暂停/静态页）不落行，离线据此把暂停排除出统计
const PLAY_WINDOW: Duration = Duration::from_secs(1);

/// 播放态直方图窗口：fps_probe 线程局部状态，不参与 FAS 控制路径（FAS 会话下不累不加）
struct PlayHist {
    hist: [u32; PLAY_HIST_BINS],
    frames: u32,
    win_start: Instant,
    /// 窗口是否在计时中：帧源摘除或 FAS 会话期间置 false（`suspend`），回到旁路时 `resume` 从零起——
    /// 防「上一段播放的半窗」与「新段首帧」被算进同一秒（n 偏小、看着像掉帧）
    live: bool,
}

impl PlayHist {
    fn new() -> Self {
        Self {
            hist: [0; PLAY_HIST_BINS],
            frames: 0,
            win_start: Instant::now(),
            live: false,
        }
    }

    /// 进入旁路：窗口从零起（已在旁路中则保持原窗口，不重置）
    fn resume(&mut self) {
        if !self.live {
            self.hist = [0; PLAY_HIST_BINS];
            self.frames = 0;
            self.win_start = Instant::now();
            self.live = true;
        }
    }

    /// 离开旁路（FAS 接管或帧源摘除）：只标记窗口失效，等 `resume` 再清零
    fn suspend(&mut self) {
        self.live = false;
    }

    /// 累加一帧间隔（ns；探针侧已按 `[MIN_FRAME_NS, MAX_FRAME_NS]` 过滤）
    fn ingest(&mut self, delta_ns: u64) {
        let bin = ((delta_ns / PLAY_HIST_BIN_NS) as usize).min(PLAY_HIST_BINS - 1);
        self.hist[bin] = self.hist[bin].saturating_add(1);
        self.frames = self.frames.saturating_add(1);
    }

    /// 窗口到期清零：起点按固定周期**递推**而非取 `now`，避免每次 poll 间隙（<=100ms）在数小时会话里累积漂移
    fn reset(&mut self) {
        self.hist = [0; PLAY_HIST_BINS];
        self.frames = 0;
        while self.win_start.elapsed() >= PLAY_WINDOW {
            self.win_start += PLAY_WINDOW;
        }
    }

    /// 直方图 → event 行 reason：`n=<帧数>;b<档号>=<计数>;…`（只落非零档，档号稀疏、可离线还原）
    fn reason(&self) -> String {
        let mut s = String::with_capacity(64);
        s.push_str("n=");
        s.push_str(&self.frames.to_string());
        for (i, c) in self.hist.iter().enumerate() {
            if *c > 0 {
                s.push_str(";b");
                s.push_str(&i.to_string());
                s.push('=');
                s.push_str(&c.to_string());
            }
        }
        s
    }
}

// [probe]
// ProbeState：单个 PID 的帧统计

struct ProbeState {
    source: FrameSource,
    cutoff_ns: u64,
    last_ktime_ns: Option<u64>,
    frametimes: VecDeque<Duration>,
    /// 在 ingest 时绑定会话；出队不重新标记。
    pending: VecDeque<PendingFrame>,
}

struct PendingFrame {
    delta_ns: u64,
    source: FrameSource,
}

#[derive(Clone, Copy)]
struct ProbeTarget {
    source: FrameSource,
    playback: bool,
}

fn same_target(left: Option<ProbeTarget>, right: Option<ProbeTarget>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.source.pid == right.source.pid
                && left.source.generation == right.source.generation
                && left.source.frame_feedback == right.source.frame_feedback
                && left.playback == right.playback
        }
        _ => false,
    }
}

fn select_target(
    fas_active: bool,
    source: Option<FrameSource>,
    playback_active: bool,
    diagnostics_active: bool,
    foreground_pid: u32,
) -> Option<ProbeTarget> {
    if fas_active || source.is_some() {
        return source
            .filter(|source| source.frame_feedback && source.pid > 0)
            .map(|source| ProbeTarget {
                source,
                playback: false,
            });
    }
    (playback_active && diagnostics_active && foreground_pid > 0).then_some(ProbeTarget {
        source: FrameSource {
            pid: foreground_pid,
            generation: 0,
            frame_feedback: false,
        },
        playback: true,
    })
}

fn parse_process_starttime(stat: &str) -> Option<u64> {
    let fields = stat.rsplit_once(')')?.1;
    let mut fields = fields.split_whitespace();
    let process_state = fields.next()?;
    if matches!(process_state, "Z" | "X" | "x") {
        return None;
    }
    fields.nth(18)?.parse().ok()
}

impl ProbeState {
    fn new(source: FrameSource, cutoff_ns: u64) -> Self {
        Self {
            source,
            cutoff_ns,
            last_ktime_ns: None,
            frametimes: VecDeque::with_capacity(FRAMETIME_WINDOW),
            pending: VecDeque::new(),
        }
    }

    fn ingest(&mut self, ktime_ns: u64) {
        if ktime_ns <= self.cutoff_ns
            || self
                .last_ktime_ns
                .is_some_and(|previous| ktime_ns <= previous)
        {
            return;
        }
        if let Some(last_ns) = self.last_ktime_ns {
            let delta_ns = ktime_ns.saturating_sub(last_ns);
            if (MIN_FRAME_NS..=MAX_FRAME_NS).contains(&delta_ns) {
                if self.frametimes.len() >= FRAMETIME_WINDOW {
                    self.frametimes.pop_back();
                }
                self.frametimes.push_front(Duration::from_nanos(delta_ns));
                self.pending.push_back(PendingFrame {
                    delta_ns,
                    source: self.source,
                });
            }
        }
        self.last_ktime_ns = Some(ktime_ns);
    }
}

// [identity]
/// 目标进程存活门：优先持有 `pidfd`（内核对象绑定 pid，PID 号复用后原 fd 指向已退出进程，天然区分
/// 旧/新进程）；内核不支持 `pidfd_open` 时退化为每个投喂批次的 `/proc/<pid>/stat` starttime 复验。
/// `pidfd` 的探针失败/退出由 RAII 关闭，不在热循环里泄漏 fd。
struct ProcessIdentity {
    pid: u32,
    starttime: u64,
    /// `Some` = pidfd 可用（`POLLIN`/`POLLHUP` 零超时判定退出）；`None` = 退化到 stat 复验。
    pidfd: Option<OwnedFd>,
}

impl ProcessIdentity {
    /// 取 `pidfd`（失败即退化），再以 `stat` 复验 starttime：fd 对应的进程身份必须与 attach 期望一致，
    /// 防止「取 fd 期间进程退出+号被复用」把新进程误当旧目标。
    fn acquire(pid: u32) -> Result<Self, anyhow::Error> {
        let pidfd = open_pidfd(pid);
        let starttime = read_process_starttime(pid)?;
        Ok(Self {
            pid,
            starttime,
            pidfd,
        })
    }

    /// 零超时非阻塞探测：pidfd 可读/半关闭 = 目标已退出（内核在进程退出时置该状态）。
    /// 无 pidfd 时返回 `None`，交由调用方走 stat 复验。
    fn exited(&self) -> Option<bool> {
        let pidfd = self.pidfd.as_ref()?;
        let mut descriptor = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: descriptor 指向单个有效可写的 pollfd；timeout=0 立即返回，不阻塞。
        let ready = unsafe { libc::poll(&mut descriptor, 1, 0) };
        if ready < 0 {
            return None;
        }
        Some(ready > 0 || descriptor.revents != 0)
    }

    /// 退化路径复验：仅与记录的 starttime 比对，进程消失或未读到 starttime 均视为退出。
    fn verify_starttime(&self) -> bool {
        read_process_starttime(self.pid).ok() == Some(self.starttime)
    }
}

/// `pidfd_open(2)`：Android/Linux 5.3+ 提供；返回的 fd 可零超时 `poll` 判定进程退出，旧内核返回 `ENOSYS`。
fn open_pidfd(pid: u32) -> Option<OwnedFd> {
    // SAFETY: 系统调用仅读取 pid 数值，成功时返回新 fd 的所有权（立即包进 OwnedFd 由 RAII 关闭）。
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::c_long, 0) };
    if raw < 0 {
        return None;
    }
    let raw = i32::try_from(raw).ok()?;
    // SAFETY: raw 由本函数刚取得的成功 pidfd，唯一所有权移交 OwnedFd。
    Some(unsafe { OwnedFd::from_raw_fd(raw) })
}

/// 存活判定纯决策：pidfd 结论优先采信；无 pidfd 时仅在确有新帧时以 starttime 复验（idle 不扫描 /proc）。
fn liveness_failed(
    pidfd_exited: Option<bool>,
    fallback_identity_ok: bool,
    has_new_frames: bool,
) -> bool {
    match pidfd_exited {
        Some(exited) => exited,
        None => has_new_frames && !fallback_identity_ok,
    }
}

/// 重挂封锁纯决策：已判定退出的同一身份 `(pid, generation)` 不得自动重挂，直到目标身份变化
/// （generation 递增或前台 pid 改变）——PID 号可能已被复用给新进程。
fn attach_blocked(blocked: Option<(u32, u64)>, target: FrameSource) -> bool {
    blocked == Some((target.pid, target.generation))
}

// [manager]
// FpsManager：单 eBPF 实例，单会话 attach

struct FpsManager {
    bpf: Ebpf,
    ring_fd: RawFd,
    /// 当前活跃 PID → UProbeLinkId
    links: HashMap<u32, aya::programs::uprobe::UProbeLinkId>,
    /// 当前活跃 PID → 帧统计
    states: HashMap<u32, ProbeState>,
    /// 当前关注的目标 PID（最近一次 attach 的 PID）
    current_pid: u32,
    requested_target: Option<ProbeTarget>,
    requested_starttime: Option<u64>,
    /// libgui 扫描到的 queueBuffer 符号变体（建实例时扫一次；空 = 帧源永久不可用）
    symbol_candidates: Vec<String>,
    /// attach 连续失败次数（退避档位与 warn 降频共用）
    attach_fail_count: u32,
    /// 当前目标会话的 attach 累计失败次数。
    attach_fail_total: u64,
    /// 当前 attach 目标的存活门（pidfd 优先，退化时记录 starttime 供批前复验）。
    identity: Option<ProcessIdentity>,
    /// 已判定退出的帧源身份 `(pid, generation)`：同一身份不得自动重挂（防 PID 号被复用后静默接上新进程），
    /// 直到目标身份变化（generation 递增或前台 pid 改变）才解除。
    blocked_source: Option<(u32, u64)>,
    /// 最后一次 attach 失败原因（与 attach_fail_total 同步更新），供周期摘要诊断
    last_attach_error: Option<String>,
/// 下次允许重试 attach 的时刻（退避窗口内不解析 libgui）
    attach_retry_at: Instant,
}

impl FpsManager {
    /// 加载 eBPF 程序（只执行一次），获取 RingBuf fd
    fn new() -> Result<Self, anyhow::Error> {
        #[cfg(debug_assertions)]
        let mut bpf = Ebpf::load(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/ebpf_target/bpfel-unknown-none/debug/yumi-ebpf"
        )))?;
        #[cfg(not(debug_assertions))]
        let mut bpf = Ebpf::load(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/ebpf_target/bpfel-unknown-none/release/yumi-ebpf"
        )))?;

        let program: &mut UProbe = bpf.program_mut("handle_frame").unwrap().try_into()?;
        program.load()?;

        let ring_fd = {
            let ring_map = bpf.map_mut("RING_BUF").expect("RING_BUF not found");
            let ring = RingBuf::try_from(ring_map).expect("RingBuf::try_from");
            ring.as_raw_fd()
        };

        let symbol_candidates = scan_queue_buffer_symbols(LIBGUI_PATH);
        debug!(
            "{}",
            t_with_args(
                "fps-monitor-symbol-scan",
                &fluent_args!("count" => symbol_candidates.len().to_string())
            )
        );

        Ok(Self {
            bpf,
            ring_fd,
            links: HashMap::new(),
            states: HashMap::new(),
            current_pid: 0,
            requested_target: None,
            requested_starttime: None,
            symbol_candidates,
            attach_fail_count: 0,
            attach_fail_total: 0,
            identity: None,
            blocked_source: None,
            last_attach_error: None,
            attach_retry_at: Instant::now(),
        })
    }

    /// 会话/PID/旁路变化均强制摘挂；失败目标也记账，退避不跨会话。
    fn switch_source(&mut self, target: Option<ProbeTarget>) -> Result<(), anyhow::Error> {
        if !same_target(self.requested_target, target) {
            self.detach_current()?;
            self.requested_target = target;
            self.requested_starttime = None;
            // 目标身份变化（generation 递增或前台 pid 改变）即解除退出封锁。
            if let Some(target) = target {
                if !attach_blocked(self.blocked_source, target.source) {
                    self.blocked_source = None;
                }
            }
            self.attach_fail_count = 0;
            self.attach_fail_total = 0;
            self.last_attach_error = None;
            self.attach_retry_at = Instant::now();
        }
        let Some(target) = target else {
            debug!("{}", t("fps-monitor-detached"));
            return Ok(());
        };
        // 已判定退出的同一身份不得自动重挂（PID 号可能已被复用给新进程）。
        if attach_blocked(self.blocked_source, target.source) {
            return Ok(());
        }
        if self.has_active_probe() || !self.attach_retry_due() {
            return Ok(());
        }

        let new_pid = target.source.pid;
        let process_starttime = read_process_starttime(new_pid)?;
        if self
            .requested_starttime
            .is_some_and(|expected| expected != process_starttime)
        {
            return Err(anyhow::anyhow!("frame source pid reused within session"));
        }
        self.requested_starttime = Some(process_starttime);
        let pid_i32 = new_pid as i32;
        let scope = NonZeroU32::new(new_pid).map(UProbeScope::OneProcess);
        let Some(scope) = scope else {
// 防御：非法 PID（调用方已过滤 0），不 panic
            warn!(
                "{}",
                t_with_args(
                    "fps-monitor-pid-switch-failed",
                    &fluent_args!("error" => format!("invalid pid {new_pid}"))
                )
            );
            return Ok(());
        };

// 建实例时没扫到符号（libgui 当时不可读）则重扫：重试最长 60s 一次成本可忽略，可救回开机早期读不到文件的场景
        if self.symbol_candidates.is_empty() {
            self.symbol_candidates = scan_queue_buffer_symbols(LIBGUI_PATH);
        }

// 候选顺序：短签名 → 长签名 → dynsym 扫描变体（覆盖短/长名都解析失败的机型，按 mangled 前缀匹配）
// TODO: 两个签名并存时先试短签名（旧版重载）——若真机帧数偏少，说明挂到非主路径重载，改长签名优先再验
        let mut candidates: Vec<String> = vec![SYMBOL_SHORT.to_string(), SYMBOL_LONG.to_string()];
        for sym in &self.symbol_candidates {
            if !candidates.contains(sym) {
                candidates.push(sym.clone());
            }
        }

        let mut attached = None;
        for (i, sym) in candidates.iter().enumerate() {
            let program: &mut UProbe = self.bpf.program_mut("handle_frame").unwrap().try_into()?;
            match program.attach(
                UProbeAttachPoint::from(UProbeAttachLocation::from(sym.as_str())),
                LIBGUI_PATH,
                scope,
            ) {
                Ok(link) => {
                    if i > 0 {
                        debug!("{}", t("fps-monitor-symbol-short-miss"));
                    }
                    attached = Some((sym.clone(), link));
                    break;
                }
                Err(e) => {
// 最后一个候选的失败原因才是有效诊断信息（前面失败只说明签名不匹配）
                    if i + 1 == candidates.len() {
                        return Err(e.into());
                    }
                }
            }
        }
        let Some((sym, link)) = attached else {
            return Err(anyhow::anyhow!("no queueBuffer symbol available"));
        };

        // 取存活门并按期望 starttime 复验：pidfd 失败即退化到 stat 复验，仍与本次 starttime 一致才继续。
        let identity = match ProcessIdentity::acquire(new_pid) {
            Ok(identity) if identity.starttime == process_starttime => identity,
            Ok(_) => {
                let program: &mut UProbe =
                    self.bpf.program_mut("handle_frame").unwrap().try_into()?;
                let _ = program.detach(link);
                return Err(anyhow::anyhow!("frame source identity changed during attach"));
            }
            Err(error) => {
                let program: &mut UProbe =
                    self.bpf.program_mut("handle_frame").unwrap().try_into()?;
                let _ = program.detach(link);
                return Err(error);
            }
        };

        self.links.insert(new_pid, link);
        self.current_pid = new_pid;
        // 摘除后清 ring；挂载完成后再清 ring 并取同源时钟界标，迟到的旧时间戳也不能进入新会话。
        self.drain_ring();
        let boundary = monotonic_timestamp_ns().and_then(|cutoff| {
            if read_process_starttime(new_pid)? != process_starttime {
                return Err(anyhow::anyhow!("frame source pid changed during attach"));
            }
            Ok(cutoff)
        });
        let cutoff_ns = match boundary {
            Ok(cutoff) => cutoff,
            Err(error) => {
                self.detach_current()?;
                return Err(error);
            }
        };
        self.states
            .insert(new_pid, ProbeState::new(target.source, cutoff_ns));
        self.identity = Some(identity);
        self.attach_fail_count = 0;
        // 一并清「最后一次失败原因」：不清则 attach 恢复后摘要会一直复读已失效的旧错误
        // （attach_fail_total 保留作会话累计，供「本次会话共失败多少次」观测）
        self.last_attach_error = None;
        self.attach_retry_at = Instant::now();
        debug!(
            "{}",
            t_with_args(
                "fps-monitor-attach-symbol-name",
                &fluent_args!("symbol" => sym)
            )
        );

        debug!(
            "{}",
            t_with_args(
                "fps-monitor-attach-symbol",
                &fluent_args!(
                    "pid" => pid_i32.to_string(),
                    "lib" => LIBGUI_PATH
                )
            )
        );

        info!(
            "{}",
            t_with_args(
                "fps-monitor-attached",
                &fluent_args!("pid" => pid_i32.to_string())
            )
        );
        Ok(())
    }

    fn detach_current(&mut self) -> Result<(), anyhow::Error> {
        self.current_pid = 0;
        self.states.clear();
        self.identity = None;
        let program: &mut UProbe = self.bpf.program_mut("handle_frame").unwrap().try_into()?;
        for (_, link) in self.links.drain() {
            program.detach(link)?;
        }
        self.drain_ring();
        Ok(())
    }

    fn drain_ring(&mut self) {
        let ring_map = self.bpf.map_mut("RING_BUF").expect("RING_BUF not found");
        let mut ring = RingBuf::try_from(ring_map).expect("RingBuf::try_from failed");
        while ring.next().is_some() {}
    }

    /// 出队/投喂前的存活门（零超时、无阻塞）：pidfd 可读即目标已退出 → 立即 detach、丢 pending、封锁该身份
    /// 拒绝自动重挂；无 pidfd 的目标仅在确有新帧时以 starttime 复验一次（idle 不扫描 /proc）。
    fn check_liveness(&mut self, has_new_frames: bool) -> Result<bool, anyhow::Error> {
        if !self.has_active_probe() {
            return Ok(false);
        }
        let Some(identity) = self.identity.as_ref() else {
            return Ok(false);
        };
        let pidfd_exited = identity.exited();
        let fallback_identity_ok =
            !(pidfd_exited.is_none() && has_new_frames) || identity.verify_starttime();
        if !liveness_failed(pidfd_exited, fallback_identity_ok, has_new_frames) {
            return Ok(false);
        }
        let blocked = self
            .requested_target
            .map(|target| (target.source.pid, target.source.generation));
        self.detach_current()?;
        self.blocked_source = blocked;
        warn!(
            "{}",
            t_with_args(
                "fps-monitor-pid-switch-failed",
                &fluent_args!("error" => format!("frame source process exited: {blocked:?}"))
            )
        );
        Ok(true)
    }

    /// 从共享 RingBuf 读取帧事件，按 PID 分派
    fn poll_frames(&mut self) {
        let ring_map = self.bpf.map_mut("RING_BUF").expect("RING_BUF not found");
        let mut ring = RingBuf::try_from(ring_map).expect("RingBuf::try_from failed");

        while let Some(data) = ring.next() {
            if data.len() < size_of::<FrameTimestampEvent>() {
                continue;
            }
            // SAFETY: 长度已验证，整数事件字段允许任意位模式；ring 事件不保证对齐。
            let event = unsafe { ptr::read_unaligned(data.as_ptr().cast::<FrameTimestampEvent>()) };

            if let Some(state) = self.states.get_mut(&event.pid) {
                state.ingest(event.ktime_ns);
            }
        }
    }

    /// 取走全部待投喂的新帧间隔（ns），各 PID 队列拼接返回；不保证严格时间序（FAS 只消费活跃 PID 样本）
    fn take_pending(&mut self) -> Vec<PendingFrame> {
        let mut out = Vec::new();
        for state in self.states.values_mut() {
            out.extend(state.pending.drain(..));
        }
        out
    }

    /// 本窗口是否有新帧待投喂（存活门退化路径据此决定要不要读 /proc，idle 不扫描）。
    fn has_pending(&self) -> bool {
        self.states.values().any(|state| !state.pending.is_empty())
    }

    fn has_active_probe(&self) -> bool {
        self.current_pid > 0
    }

/// 是否到了可重试 attach 的时刻（退避窗口内 false）：符号解析失败是持续故障，按 500ms 反复重试等于每秒两次读解析 libgui ELF——纯浪费且刷爆 daemon.log
    fn attach_retry_due(&self) -> bool {
        Instant::now() >= self.attach_retry_at
    }

    /// 无探针时睡到退避界标，上限 1s；新会话不继承旧目标退避。
    fn idle_poll_timeout(&self) -> Duration {
        let left = self
            .attach_retry_at
            .saturating_duration_since(Instant::now());
        left.clamp(
            Duration::from_millis(100),
            Duration::from_millis(IDLE_POLL_MAX_MS),
        )
    }

/// 记录一次 attach 失败：推进退避窗口并降频打日志——首次与每 10 次打 warn，其余 debug （此前每次 warn，一个游戏会话能刷数千行，日志写入本身成了负担）
    fn report_attach_failure(&mut self, err: &anyhow::Error) {
        self.attach_fail_count = self.attach_fail_count.saturating_add(1);
        // 累计计数 + 最后原因：供 [attach_stats] 周期摘要观测真实故障总量（warn 降频后总量不可见）
        self.attach_fail_total = self.attach_fail_total.saturating_add(1);
        self.last_attach_error = Some(err.to_string());
        let no_symbol = self.symbol_candidates.is_empty();
        let backoff = if no_symbol {
            RETRY_BACKOFF_NO_SYMBOL_SECS
        } else {
            let i = (self.attach_fail_count as usize).saturating_sub(1);
            RETRY_BACKOFF_SECS[i.min(RETRY_BACKOFF_SECS.len() - 1)]
        };
        self.attach_retry_at = Instant::now() + Duration::from_secs(backoff);

        if self.attach_fail_count % 10 != 1 {
            debug!(
                "{}",
                t_with_args(
                    "fps-monitor-pid-switch-failed",
                    &fluent_args!("error" => err.to_string())
                )
            );
            return;
        }
        if no_symbol {
            warn!(
                "{}",
                t_with_args(
                    "fps-monitor-frame-source-missing",
                    &fluent_args!(
                        "lib" => LIBGUI_PATH,
                        "secs" => backoff.to_string()
                    )
                )
            );
        } else {
            warn!(
                "{}",
                t_with_args(
                    "fps-monitor-pid-switch-failed",
                    &fluent_args!("error" => err.to_string())
                )
            );
        }
    }
}

/// CLOCK_MONOTONIC 与 bpf_ktime_get_ns 同源，不使用包含休眠时间的 BOOTTIME。
fn monotonic_timestamp_ns() -> Result<u64, anyhow::Error> {
    let mut timestamp = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: timestamp 指向有效可写的 timespec，调用不保留指针。
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut timestamp) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let seconds = u64::try_from(timestamp.tv_sec)?;
    let nanoseconds = u64::try_from(timestamp.tv_nsec)?;
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|seconds| seconds.checked_add(nanoseconds))
        .ok_or_else(|| anyhow::anyhow!("monotonic timestamp overflow"))
}

fn read_process_starttime(pid: u32) -> Result<u64, anyhow::Error> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    parse_process_starttime(&stat)
        .ok_or_else(|| anyhow::anyhow!("frame source process unavailable: {pid}"))
}

/// 从 ELF64 dynsym 取回所有 `android::Surface::queueBuffer` 变体名只支持小端 ELF64（Android 目标机皆如此）；解析不了返回空 vec，由调用方按「帧源不可用」
/// 退避处理——不拿硬编码名反复撞墙
fn scan_queue_buffer_symbols(path: &str) -> Vec<String> {
    let Ok(data) = std::fs::read(path) else {
        return Vec::new();
    };
    let Some(out) = scan_dynsym_names(&data) else {
        return Vec::new();
    };
    let mut hits: Vec<String> = out
        .into_iter()
        .filter(|n| n.starts_with(SYMBOL_MANGLED_PREFIX))
        .collect();
    hits.sort();
    hits.dedup();
    hits
}

/// 读出动态符号表里的全部符号名（SHT_DYNSYM=11，名字表由 sh_link 指向）
fn scan_dynsym_names(data: &[u8]) -> Option<Vec<String>> {
    if data.len() < 64 || data.first()? != &0x7f || data.get(1..4)? != b"ELF" || data[4] != 2 {
        return None;
    }
    let shoff = le_u64(data, 0x28)? as usize;
    let shentsize = le_u16(data, 0x3a)? as usize;
    let shnum = le_u16(data, 0x3c)? as usize;
    if shentsize < 64 || shnum == 0 {
        return None;
    }
    for i in 0..shnum {
        let sh = shoff + i * shentsize;
        if le_u32(data, sh + 4)? != 11 {
            continue;
        }
        let sym_off = le_u64(data, sh + 0x18)? as usize;
        let sym_size = le_u64(data, sh + 0x20)? as usize;
        let str_idx = le_u32(data, sh + 0x28)? as usize;
        let st = shoff + str_idx * shentsize;
        let str_off = le_u64(data, st + 0x18)? as usize;
        let str_size = le_u64(data, st + 0x20)? as usize;
        let strtab = data.get(str_off..str_off.saturating_add(str_size))?;

        let mut names = Vec::new();
        for s in 0..(sym_size / 24) {
            let so = sym_off + s * 24;
            let name_idx = le_u32(data, so)? as usize;
            let Some(rest) = strtab.get(name_idx..) else {
                continue;
            };
            let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            if let Ok(name) = std::str::from_utf8(&rest[..end]) {
                names.push(name.to_string());
            }
        }
        return Some(names);
    }
    None
}

fn le_u16(d: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(off..off + 2)?.try_into().ok()?))
}
fn le_u32(d: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(off..off + 4)?.try_into().ok()?))
}
fn le_u64(d: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(off..off + 8)?.try_into().ok()?))
}

// [session_tests]
#[cfg(test)]
mod session_tests {
    use super::*;

    fn source(generation: u64) -> FrameSource {
        FrameSource {
            pid: 42,
            generation,
            frame_feedback: true,
        }
    }

    #[test]
    fn stale_timestamp_does_not_replace_last_frame() {
        let mut state = ProbeState::new(source(1), 0);
        state.ingest(100_000_000);
        state.ingest(50_000_000);
        state.ingest(66_000_000);
        assert!(state.pending.is_empty());
    }

    #[test]
    fn cutoff_rejects_equality_and_delayed_same_pid_events() {
        let mut state = ProbeState::new(source(2), 100_000_000);
        state.ingest(84_000_000);
        state.ingest(100_000_000);
        state.ingest(116_000_000);
        assert!(state.pending.is_empty());
        state.ingest(132_000_000);
        let frame = state.pending.pop_front().unwrap();
        assert_eq!(frame.delta_ns, 16_000_000);
        assert_eq!(frame.source.generation, 2);
    }

    #[test]
    fn dequeued_frames_keep_ingest_session_identity() {
        let mut state = ProbeState::new(source(1), 0);
        state.ingest(100_000_000);
        state.ingest(116_000_000);
        let queued = state.pending.pop_front().unwrap();
        let mut replacement = ProbeState::new(source(2), 120_000_000);
        replacement.ingest(116_000_000);
        assert!(replacement.pending.is_empty());
        assert_eq!(queued.source.pid, 42);
        assert_eq!(queued.source.generation, 1);
        assert_eq!(queued.delta_ns, 16_000_000);
    }

    #[test]
    fn owner_has_priority_over_overlay_foreground() {
        let target = select_target(true, Some(source(1)), true, true, 900).unwrap();
        assert_eq!(target.source.pid, 42);
        assert!(!target.playback);
    }

    #[test]
    fn suspended_owner_never_falls_back_to_playback_or_foreground() {
        let mut owner = source(1);
        owner.frame_feedback = false;
        assert!(select_target(true, Some(owner), true, true, 900).is_none());
        assert!(select_target(false, Some(owner), true, true, 900).is_none());
        assert!(select_target(true, None, true, true, 900).is_none());
    }

    #[test]
    fn playback_requires_diagnostics_and_never_enables_feedback() {
        assert!(select_target(false, None, true, false, 900).is_none());
        assert!(select_target(false, None, false, true, 900).is_none());
        assert!(select_target(false, None, true, true, 0).is_none());
        let target = select_target(false, None, true, true, 900).unwrap();
        assert!(target.playback);
        assert!(!target.source.frame_feedback);
    }

    #[test]
    fn same_pid_new_generation_and_new_pid_are_different_targets() {
        let original = select_target(true, Some(source(1)), false, false, 42);
        let next_session = select_target(true, Some(source(2)), false, false, 42);
        assert!(!same_target(original, next_session));
        let mut next_process = source(1);
        next_process.pid = 99;
        assert!(!same_target(
            original,
            select_target(true, Some(next_process), false, false, 99)
        ));
        assert!(same_target(original, original));
    }

    #[test]
    fn process_identity_handles_parentheses_and_rejects_zombies() {
        let middle = vec!["0"; 18].join(" ");
        let stat = format!("42 (render ) worker) S {middle} 12345 0");
        assert_eq!(parse_process_starttime(&stat), Some(12345));
        assert_eq!(parse_process_starttime(&stat.replace(") S ", ") Z ")), None);
        assert_eq!(parse_process_starttime("42 malformed"), None);
    }

    #[test]
    fn pidfd_verdict_is_authoritative_and_fallback_is_frame_gated() {
        // pidfd 报退出：无论 fallback 与是否有新帧，都判退出。
        assert!(liveness_failed(Some(true), true, false));
        assert!(liveness_failed(Some(true), true, true));
        // pidfd 报存活：即便 fallback 复验失败也不误杀。
        assert!(!liveness_failed(Some(false), false, true));
        // 无 pidfd：仅在有新帧时以 starttime 复验，idle（无新帧）不扫描 /proc。
        assert!(!liveness_failed(None, false, false));
        assert!(liveness_failed(None, false, true));
        assert!(!liveness_failed(None, true, true));
    }

    #[test]
    fn pidfd_detects_child_exit_on_this_host() {
        // 本机 fork 子进程：存活时 pidfd 不应报退出，退出后应零超时报退出（无 Android 设备也可验证）。
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let identity = ProcessIdentity::acquire(child.id()).expect("acquire identity");
        if identity.pidfd.is_none() {
            // 旧内核无 pidfd_open：退化路径也必须能识别存活进程，不误杀。
            assert!(identity.verify_starttime());
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        assert_eq!(identity.exited(), Some(false));
        child.kill().expect("kill child");
        child.wait().expect("reap child");
        // 退出后 poll 结果可能滞后极短时间，重试几次仍必须报退出。
        let exited = (0..50).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            identity.exited() == Some(true)
        });
        assert!(exited, "pidfd must report exit after child reaped");
    }

    #[test]
    fn exited_identity_stays_blocked_until_target_identity_changes() {
        let dead = (42, 7);
        // 同一身份（同 pid 同 generation）拒绝自动重挂。
        assert!(attach_blocked(Some(dead), source(7)));
        // generation 递增 = 新会话，解除封锁。
        assert!(!attach_blocked(Some(dead), source(8)));
        // 前台 pid 改变（播放态/换游戏）同样解除。
        let mut other_pid = source(7);
        other_pid.pid = 99;
        assert!(!attach_blocked(Some(dead), other_pid));
        // 无封锁时一律放行。
        assert!(!attach_blocked(None, source(7)));
    }
}

// [loop]

pub async fn start_fps_loop(
    tx: SyncSender<DaemonEvent>,
    rx_pid: watch::Receiver<u32>,
    fas_signal: Arc<FasSignal>,
) -> Result<(), anyhow::Error> {
    info!("{}", t("fps-monitor-init"));

    let tx_clone = tx.clone();
    std::thread::Builder::new()
        .name("fps_probe".into())
        .spawn(move || {
            let mut manager = match FpsManager::new() {
                Ok(m) => m,
                Err(e) => {
                    warn!(
                        "{}",
                        t_with_args(
                            "fps-monitor-attach-failed-initial",
                            &fluent_args!("error" => e.to_string())
                        )
                    );
                    return;
                }
            };

// 反偷跑：FAS 未激活时不挂任何 uprobe（eBPF 已加载但无 attach 点即零执行）此前 daemon 启动即 attach，
// 桌面/普通应用每帧 queueBuffer 都过探针而 FrameUpdate 被调度侧丢弃，纯白烧；首个 FAS 应用激活后由 PID 广播驱动 switch_pid 正常挂载
            info!("{}", t("fps-monitor-passive"));

// mio 轮询（只创建一次；创建失败本线程无法工作，告警退出交看门狗自愈）
            let mut poll = match Poll::new() {
                Ok(p) => p,
                Err(e) => {
                    warn!(
                        "{}",
                        t_with_args(
                            "fps-monitor-attach-failed-initial",
                            &fluent_args!("error" => e.to_string())
                        )
                    );
                    return;
                }
            };
            let mut events = Events::with_capacity(64);
            let token = Token(0);
            let mut frame_counter: u32 = 0;
// [attach_stats] 摘要节流基准（上次落摘要的时刻；None = 本轮循环还没落过）
            let mut last_stats_at: Option<Instant> = None;
// 事件通道拥塞丢弃的帧样本数（仅计数，EMA 平滑可容忍少量丢失）
            let mut dropped_frames: u64 = 0;
// 播放态旁路直方图窗口（只在 FAS 未激活时累加，见下方分流）
            let mut play = PlayHist::new();

// 注册 RingBuf fd（只一次）。fd 与 attach 无关（attach/detach 前后不变），无条件注册最简单——未注册仅丢事件驱动唤醒，poll 超时兜底仍可处理帧（代价是无谓的 500ms 轮询）
            let fd = manager.ring_fd;
            let mut source = SourceFd(&fd);
            if let Err(e) =
                poll.registry()
                    .register(&mut source, token, Interest::READABLE)
            {
                warn!(
                    "{}",
                    t_with_args(
                        "fps-monitor-attach-failed-initial",
                        &fluent_args!("error" => e.to_string())
                    )
                );
            }

            loop {
// [attach_stats] attach 失败可见性：失败只在「首次 + 每 10 次」warn 且消息不带总数，长尾偶发与持续故障
// 无从区分；距上次摘要满 ATTACH_STATS_INTERVAL 落一行，log_enabled! 门控省掉 INFO 级别下的 format!
                let stats_due = last_stats_at.map_or(true, |t| t.elapsed() >= ATTACH_STATS_INTERVAL);
                if manager.attach_fail_total > 0
                    && stats_due
                    && log::log_enabled!(log::Level::Debug)
                {
                    last_stats_at = Some(Instant::now());
                    debug!(
                        "{}",
                        t_with_args(
                            "fps-monitor-attach-stats",
                            &fluent_args!(
                                "count" => manager.attach_fail_total.to_string(),
                                "error" => manager.last_attach_error.clone().unwrap_or_default()
                            )
                        )
                    );
                }
// [gate]
                let target = select_target(
                    fas_signal.is_active(),
                    fas_signal.frame_source(),
                    fas_signal.playback_active(),
                    crate::logger::diag_active(),
                    *rx_pid.borrow(),
                );
// [pid-switch]
                let target_changed = !same_target(manager.requested_target, target);
                if target_changed {
                    play.suspend();
                    frame_counter = 0;
                    last_stats_at = None;
                }
                if let Err(error) = manager.switch_source(target) {
                    manager.report_attach_failure(&error);
                }
                let Some(target) = target else {
                    play.suspend();
                    let needs_bounded_wait = fas_signal.is_active()
                        || fas_signal.frame_source().is_some()
                        || fas_signal.playback_active();
                    let timeout = needs_bounded_wait.then_some(Duration::from_secs(1));
                    let wait_started = Instant::now();
                    fas_signal.wait_until_frame_source(timeout);
                    // 播放态诊断关闭时信号谓词可能仍为 true，保留有界阻塞而不是空转。
                    if needs_bounded_wait {
                        let still_idle = select_target(
                            fas_signal.is_active(), fas_signal.frame_source(),
                            fas_signal.playback_active(), crate::logger::diag_active(),
                            *rx_pid.borrow(),
                        ).is_none();
                        if still_idle {
                            std::thread::sleep(Duration::from_secs(1).saturating_sub(wait_started.elapsed()));
                        }
                    }
                    continue;
                };
                if !target.playback {
                    play.suspend();
                }

// [poll]
                let timeout = if manager.has_active_probe() {
                    Some(Duration::from_millis(100))
                } else {
                    Some(manager.idle_poll_timeout())
                };

// mio poll 出错只意味着被信号打断，sleep 后重试即可
                if poll.poll(&mut events, timeout).is_err() {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }

                let latest_target = select_target(
                    fas_signal.is_active(), fas_signal.frame_source(),
                    fas_signal.playback_active(), crate::logger::diag_active(),
                    *rx_pid.borrow(),
                );
                if !same_target(Some(target), latest_target) {
                    continue;
                }
                manager.poll_frames();

// 出队/投喂前存活门：pidfd 零超时判定目标已退出则立即 detach 丢 pending（旧 generation 封锁不重挂）；
// 无 pidfd 时仅在确有新帧时以 starttime 复验一次。
                let has_new_frames = manager.has_pending();
                if manager.check_liveness(has_new_frames).unwrap_or(false) {
                    continue;
                }

// 全量投喂本窗口真实新帧间隔此前每 100ms 只发 latest_frametime() 一条：60fps 下 6 帧丢 5 帧，所有以「帧数」为单位的控制常数实际时间尺度被拉长 6 倍，
// PID/防抖/jank 响应全面钝化（平均帧降、1% Low 崩塌）；且无新帧时同一条陈旧 delta 反复投喂，静态 UI 一条 heavy 帧重复计入即可累积出假 loading（perf 被钳 0.60~0.70）
                let new_deltas = manager.take_pending();
                for frame in new_deltas {
                    let delta_ns = frame.delta_ns;
// 分流：FAS 未激活 = 播放态旁路，只累直方图不投喂——FAS 未激活时调度侧对 FrameUpdate 是 no-op
//（chiri 侧 is_active 判定），投喂只白烧通道与唤醒；诊断总闸关闭时既不挂帧源也不累（门控处已挡，此处兜底）
                    if target.playback {
                        if !fas_signal.is_active() && fas_signal.frame_source().is_none()
                            && fas_signal.playback_active() && crate::logger::diag_active()
                        {
                            play.resume();
                            play.ingest(delta_ns);
                        }
                        continue;
                    }

                    let current_source = fas_signal.frame_source();
                    if !fas_signal.is_active() || !current_source.is_some_and(|source| {
                        source.frame_feedback && source.pid == frame.source.pid
                            && source.generation == frame.source.generation
                    }) {
                        continue;
                    }

                    frame_counter += 1;
                    if frame_counter % 60 == 0 {
                        let avg_ns = manager.states.get(&manager.current_pid)
                            .and_then(|s| s.frametimes.iter().map(|d| d.as_nanos()).sum::<u128>().checked_div(s.frametimes.len() as u128))
                            .unwrap_or(0);
                        debug!("{}", t_with_args("fps-monitor-frame-summary", &fluent_args!(
                            "pid" => manager.current_pid.to_string(),
                            "window" => manager.states.get(&manager.current_pid).map(|s| s.frametimes.len().to_string()).unwrap_or_default(),
                            "latest_ms" => format!("{:.2}", delta_ns as f64 / 1_000_000.0),
                            "avg_ms" => format!("{:.2}", avg_ns as f64 / 1_000_000.0)
                        )));
                    }

                    match tx_clone.try_send(DaemonEvent::FrameUpdate {
                        frame_delta_ns: delta_ns,
                        source_pid: frame.source.pid,
                        source_generation: frame.source.generation,
                    }) {
                        Ok(()) => {}
// 通道拥塞：丢弃本帧样本。绝不能阻塞发送——fps_probe 阻塞会让 eBPF ring buffer 被新事件覆盖，丢更多帧
                        Err(TrySendError::Full(_)) => {
                            dropped_frames = dropped_frames.saturating_add(1);
                            if dropped_frames % 300 == 1 {
                                warn!("{}", t_with_args("fps-monitor-frames-dropped", &fluent_args!(
                                    "count" => dropped_frames.to_string()
                                )));
                            }
                        }
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                }

// 播放态旁路落行（每轮 poll 至多一次；FAS 会话不落此表）：整秒落一行，窗口内无帧则只清窗口不落行
                if target.playback && !fas_signal.is_active() && fas_signal.frame_source().is_none()
                    && fas_signal.playback_active() && crate::logger::diag_active()
                    && play.win_start.elapsed() >= PLAY_WINDOW
                {
                    if play.frames > 0 {
// event 行：decision=playback_fps、reason=直方图（ts/mode/package 列由 logger 自动填充）
                        crate::logger::main_event("playback_fps", "", &play.reason());
                    }
                    play.reset();
                }
            }
        })?;

    info!("{}", t("fps-monitor-started"));
    std::future::pending::<()>().await;
    Ok(())
}
