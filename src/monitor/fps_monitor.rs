//! fps_monitor.rs: [consts] [probe] [manager] [loop] [gate] [pid-switch] [poll]

use std::collections::{HashMap, VecDeque};
use std::mem::size_of;
use std::num::NonZeroU32;
use std::os::unix::io::{AsRawFd, RawFd};
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
use crate::monitor::FasSignal;

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
/// 直方图档数：每档 4ms，覆盖 4ms–1020ms（<4ms 归 0 档、>=1020ms 归末档）
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
    last_ktime_ns: Option<u64>,
    frametimes: VecDeque<Duration>,
/// 本次 poll 尚未投喂给调度层的新帧间隔（ns）：ingest 后由 take_pending 全量取走；与 frametimes 分离保证投喂口径是「新帧」而非「最新一条」
    pending: VecDeque<u64>,
}

impl ProbeState {
    fn new() -> Self {
        Self {
            last_ktime_ns: None,
            frametimes: VecDeque::with_capacity(FRAMETIME_WINDOW),
            pending: VecDeque::new(),
        }
    }

    fn ingest(&mut self, ktime_ns: u64) {
        if let Some(last_ns) = self.last_ktime_ns {
            let delta_ns = ktime_ns.saturating_sub(last_ns);
            if (MIN_FRAME_NS..=MAX_FRAME_NS).contains(&delta_ns) {
                if self.frametimes.len() >= FRAMETIME_WINDOW {
                    self.frametimes.pop_back();
                }
                self.frametimes.push_front(Duration::from_nanos(delta_ns));
                self.pending.push_back(delta_ns);
            }
        }
        self.last_ktime_ns = Some(ktime_ns);
    }
}

// [manager]
// FpsManager：单 eBPF 实例，多 PID attach

struct FpsManager {
    bpf: Ebpf,
    ring_fd: RawFd,
    /// 当前活跃 PID → UProbeLinkId
    links: HashMap<u32, aya::programs::uprobe::UProbeLinkId>,
    /// 当前活跃 PID → 帧统计
    states: HashMap<u32, ProbeState>,
    /// 当前关注的目标 PID（最近一次 attach 的 PID）
    current_pid: u32,
/// libgui 扫描到的 queueBuffer 符号变体（建实例时扫一次；空 = 帧源永久不可用）
    symbol_candidates: Vec<String>,
    /// attach 连续失败次数（退避档位与 warn 降频共用）
    attach_fail_count: u32,
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
            symbol_candidates,
            attach_fail_count: 0,
            attach_retry_at: Instant::now(),
        })
    }

/// 切换到新 PID：detach 旧 + attach 新；new_pid==0` 为「纯 detach」：只摘探针并复位状态，用于 FAS 去激活时回到零开销待机（detach 后 has_active_probe()
/// ==false）
    fn switch_pid(&mut self, new_pid: u32) -> Result<(), anyhow::Error> {
        if new_pid == self.current_pid {
            return Ok(());
        }

        if self.current_pid > 0 {
            if let Some(link_id) = self.links.remove(&self.current_pid) {
                let program: &mut UProbe =
                    self.bpf.program_mut("handle_frame").unwrap().try_into()?;
                let _ = program.detach(link_id);
            }
// 旧 PID 的帧状态一并清理（PID 不会在同一次 attach 生命周期内复用）
            self.states.remove(&self.current_pid);
        }

// new_pid==0 纯 detach：失败计数一并清零——上一段会话的退避档位不带入新会话
        if new_pid == 0 {
            self.current_pid = 0;
            self.attach_fail_count = 0;
            self.attach_retry_at = Instant::now();
            debug!("{}", t("fps-monitor-detached"));
            return Ok(());
        }

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

        self.attach_fail_count = 0;
        self.attach_retry_at = Instant::now();
        debug!(
            "{}",
            t_with_args(
                "fps-monitor-attach-symbol-name",
                &fluent_args!("symbol" => sym)
            )
        );

        self.links.insert(new_pid, link);
        self.states.entry(new_pid).or_insert_with(ProbeState::new);
        self.current_pid = new_pid;

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

    /// 从共享 RingBuf 读取帧事件，按 PID 分派
    fn poll_frames(&mut self) {
        let ring_map = self.bpf.map_mut("RING_BUF").expect("RING_BUF not found");
        let mut ring = RingBuf::try_from(ring_map).expect("RingBuf::try_from failed");

        while let Some(data) = ring.next() {
            if data.len() < size_of::<FrameTimestampEvent>() {
                continue;
            }
            let event = unsafe { ptr::read_unaligned(data.as_ptr().cast::<FrameTimestampEvent>()) };

            if let Some(state) = self.states.get_mut(&event.pid) {
                state.ingest(event.ktime_ns);
            }
        }
    }

/// 取走全部待投喂的新帧间隔（ns），各 PID 队列拼接返回；不保证严格时间序（FAS 只消费活跃 PID 样本）
    fn take_pending(&mut self) -> Vec<u64> {
        let mut out = Vec::new();
        for state in self.states.values_mut() {
            out.extend(state.pending.drain(..));
        }
        out
    }

    fn has_active_probe(&self) -> bool {
        self.current_pid > 0
    }

/// 是否到了可重试 attach 的时刻（退避窗口内 false）：符号解析失败是持续故障，按 500ms 反复重试等于每秒两次读解析 libgui ELF——纯浪费且刷爆 daemon.log
    fn attach_retry_due(&self) -> bool {
        Instant::now() >= self.attach_retry_at
    }

/// 无探针时的 poll 超时：睡到下次可重试时刻，上限 1s（退避窗口内一次空转 poll，不 attach/不解析）上限理由：前台 PID 走 mpsc、不注册进 poll，超时返回是其唯一消费窗口，
/// 睡满退避会把 PID 切换拖到退避结束后
    fn idle_poll_timeout(&self) -> Duration {
        let left = self.attach_retry_at.saturating_duration_since(Instant::now());
        left.clamp(Duration::from_millis(100), Duration::from_millis(IDLE_POLL_MAX_MS))
    }

/// 记录一次 attach 失败：推进退避窗口并降频打日志——首次与每 10 次打 warn，其余 debug （此前每次 warn，一个游戏会话能刷数千行，日志写入本身成了负担）
    fn report_attach_failure(&mut self, err: &anyhow::Error) {
        self.attach_fail_count = self.attach_fail_count.saturating_add(1);
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

// [loop]

pub async fn start_fps_loop(
    tx: SyncSender<DaemonEvent>,
    rx_pid: watch::Receiver<u32>,
    fas_signal: Arc<FasSignal>,
) -> Result<(), anyhow::Error> {
    info!("{}", t("fps-monitor-init"));

// 订阅 pid_watcher 的共享前台 PID 广播（原 500ms 自轮询已删）：变化值桥接给 fps_probe 线程做 switch_pid；watch 接收端另克隆一份随闭包进线程，
// 供 FAS 激活门控补挂时读当前前台 PID （桥接任务只转发变化值，激活瞬间的最新值需从 watch 直接借）
    let (pid_tx, pid_rx) = std::sync::mpsc::channel::<u32>();
    let rx_pid_bridge = rx_pid.clone();
    tokio::spawn(async move {
        let mut rx = rx_pid_bridge;
        while rx.changed().await.is_ok() {
            let pid = *rx.borrow();
            if pid > 0 {
                let _ = pid_tx.send(pid);
            }
        }
    });

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
// [gate]
// FAS 激活门控（反偷跑核心）：未激活不消费 PID/不投喂帧；旧会话 uprobe 仍挂着则先 detach 回零开销待机，
// 再阻塞等激活信号（事件驱动，稳态 0 周期唤醒，改造前是 500ms 轮询）唤醒后不在此补挂、回循环顶部，
// 由 `!has_active_probe()` 分支从 watch 直接借当前前台 PID——桥接任务只转发 PID **变化**，FAS 应用可能
// 激活前就在前台、无后续变化事件，不读当前值则首个会话永远挂不上等的是「信号」而非「前台 PID」：
// FAS 会在 PID 未变时重新激活（息屏回前台/15s 冷却结束），只等 PID 会漏唤醒；
// 等待载体见 `crate::monitor::FasSignal`（含丢唤醒竞态推理）
// 帧源门控谓词（2026-09-27 起）= FAS 激活 **或**「`playback` 特调接管 + 诊断总闸开启」：播放态旁路帧只喂离线
// 直方图，诊断关闭时挂着探针每帧过 eBPF 纯属白烧，故播放态这一支要额外过 `diag_active()`
// 而诊断开关改动不经过本信号（`meta.dev_record` 无唤醒通道），故「播放态在接管但诊断关着」用 1s 有界等待兜住：
// 诊断一开最迟 1s 挂上；其余情形仍无限等待（稳态 0 周期唤醒）
                let fas_on = fas_signal.is_active();
                let playback_on = fas_signal.playback_active();
                let diag_on = crate::logger::diag_active();
                if !fas_on && !(playback_on && diag_on) {
                    if manager.has_active_probe() {
                        let _ = manager.switch_pid(0);
                    }
// 摘掉探针即离开旁路：窗口标记失效，下次挂上时从零起
                    play.suspend();
                    let timeout = if playback_on {
                        Some(Duration::from_secs(1))
                    } else {
                        None
                    };
                    fas_signal.wait_until_frame_source(timeout);
                    continue;
                }

// FAS 会话不参与旁路直方图（帧全喂控制路径）：窗口标记失效
                if fas_on {
                    play.suspend();
                }
                if !manager.has_active_probe() && manager.attach_retry_due() {
                    let cur = *rx_pid.borrow();
                    if cur > 0 {
                        if let Err(e) = manager.switch_pid(cur) {
                            manager.report_attach_failure(&e);
                        }
                    }
                }

// [pid-switch]
// PID 变化（tokio 订阅任务桥接的广播）：不走退避门控——新 PID 是新信息，值得立刻重试；失败照常计入退避
                while let Ok(new_pid) = pid_rx.try_recv() {
                    if let Err(e) = manager.switch_pid(new_pid) {
                        manager.report_attach_failure(&e);
                    }
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

                manager.poll_frames();

// 全量投喂本窗口真实新帧间隔此前每 100ms 只发 latest_frametime() 一条：60fps 下 6 帧丢 5 帧，所有以「帧数」为单位的控制常数实际时间尺度被拉长 6 倍，
// PID/防抖/jank 响应全面钝化（平均帧降、1% Low 崩塌）；且无新帧时同一条陈旧 delta 反复投喂，静态 UI 一条 heavy 帧重复计入即可累积出假 loading（perf 被钳 0.60~0.70）
                let new_deltas = manager.take_pending();
                for delta_ns in new_deltas {
// 分流：FAS 未激活 = 播放态旁路，只累直方图不投喂——FAS 未激活时调度侧对 FrameUpdate 是 no-op
//（chiri 侧 is_active 判定），投喂只白烧通道与唤醒；诊断总闸关闭时既不挂帧源也不累（门控处已挡，此处兜底）
                    if !fas_on {
                        if diag_on {
                            play.resume();
                            play.ingest(delta_ns);
                        }
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
                if !fas_on && diag_on && play.win_start.elapsed() >= PLAY_WINDOW {
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
