//! window_visibility.rs: [activity] [api] [worker] [query] [parse] [tests]

use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::i18n::t;

/// 最近一次 dumpsys 解析出的焦点 Activity 查表键（空 = 未知）。
/// 写入有两个来源：① FAS 可见性探针（`query_visibility`，窗口核验时）；
/// ② 全局 `ActivitySampler`（屏幕亮且有有效前台时按节流采样，**不限于 FAS**，覆盖普通应用）。
/// 消费方必须把空值当作「未知」并回退包名级判定。
static LAST_ACTIVITY: LazyLock<Mutex<Arc<str>>> = LazyLock::new(|| Mutex::new(Arc::from("")));

/// 读最近一次焦点 Activity 查表键（零分配，Arc 克隆）。未知/未采到返回空串。
pub fn last_activity_key() -> Arc<str> {
    LAST_ACTIVITY.lock().unwrap().clone()
}

// [activity]
/// Activity 采样器：与 FAS 可见性探针解耦，只跑 `dumpsys -t 1 window` 取 `mCurrentFocus`。
/// 供 Activity 级规则与 devimp `activity` 列使用；屏幕亮且存在有效前台时由调度 1s 块请求。
/// **不限 FAS**：普通应用（微信/B 站等页面内多 Activity）同样采样——游戏通常只有一个
/// Activity，Activity 级规则真正的用武之地是这类多页面应用。
/// 换包后的第一次请求立即执行（1s 最小间隔），同包稳态按 [`ACTIVITY_SAMPLE_INTERVAL`] 节流；
/// 重复请求合并为一次查询。屏幕熄灭/前台为空时调用方不请求，不产生 dumpsys 开销。
pub struct ActivitySampler {
    shared: Arc<(Mutex<ActivitySampleState>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct ActivitySampleState {
    /// 待采样的前台包名（`request` 写入；None = 无请求）
    pending: Option<String>,
    /// 上次查询时刻（节流基准；失败也算一次，避免失败重试风暴）
    last_query: Option<Instant>,
    /// 上次查询的包名（失败也算；用于同包 3s / 换包 1s 的节流判断）
    last_query_package: String,
    stopped: bool,
}

/// 同包稳态下的 Activity 采样间隔。页面内切换（聊天/视频通话、浏览/播放）通常在秒级，
/// 3s 采样足以让 activity 规则跟上；再密会明显增加 dumpsys 自耗。
const ACTIVITY_SAMPLE_INTERVAL: Duration = Duration::from_secs(3);
/// 换包后的最小采样间隔：应用切换时立即采一次，但不允许连续刷屏式查询。
const ACTIVITY_SAMPLE_MIN_GAP: Duration = Duration::from_secs(1);

/// 下一次允许查询的时刻（纯函数，便于测试）：首次立即；同包按 3s 节流；换包只要求 1s 最小间隔。
fn allowed_query_at(last_query: Option<Instant>, same_package: bool, now: Instant) -> Instant {
    match last_query {
        None => now,
        Some(last) if same_package => last + ACTIVITY_SAMPLE_INTERVAL,
        Some(last) => last + ACTIVITY_SAMPLE_MIN_GAP,
    }
}

impl ActivitySampler {
    pub fn new() -> Self {
        let shared = Arc::new((Mutex::new(ActivitySampleState::default()), Condvar::new()));
        let worker_shared = shared.clone();
        let worker = thread::Builder::new()
            .name("activity-sampler".into())
            .spawn(move || run_activity_worker(worker_shared))
            .ok();
        if worker.is_none() {
            log::error!("{}", t("activity-sampler-worker-failed"));
            shared.0.lock().unwrap_or_else(|e| e.into_inner()).stopped = true;
        }
        Self { shared, worker }
    }

    /// 请求采样当前前台：换包立即（≥1s 间隔），同包按 3s 节流；请求合并。
    /// 采样线程已停时只留 debug 痕（与窗口探针 stopped 语义一致），不做无痕丢弃。
    pub fn request(&self, foreground: &str) {
        if foreground.is_empty() {
            return;
        }
        let mut state = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped {
            log::debug!("{}", t("activity-sampler-stopped"));
            return;
        }
        state.pending = Some(foreground.to_string());
        self.shared.1.notify_one();
    }
}

impl Default for ActivitySampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ActivitySampler {
    fn drop(&mut self) {
        {
            // 与 worker panic 路径共用同一把锁；毒化时取回内部值（Drop 中再 panic = abort）
            let mut state = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
            state.stopped = true;
            state.pending = None;
        }
        self.shared.1.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_activity_worker(shared: Arc<(Mutex<ActivitySampleState>, Condvar)>) {
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        activity_loop(shared.clone())
    }))
    .is_err();
    let mut state = shared.0.lock().unwrap_or_else(|e| e.into_inner());
    state.stopped = true;
    state.pending = None;
    drop(state);
    shared.1.notify_all();
    if panicked {
        log::error!("{}", t("activity-sampler-worker-failed"));
    }
}

fn activity_loop(shared: Arc<(Mutex<ActivitySampleState>, Condvar)>) {
    loop {
        let observed_package = {
            let mut state = shared.0.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if state.stopped {
                    return;
                }
                let Some(package) = state.pending.clone() else {
                    state = match shared.1.wait(state) {
                        Ok(guard) => guard,
                        Err(error) => error.into_inner(),
                    };
                    continue;
                };
                let now = Instant::now();
                let allowed_at =
                    allowed_query_at(state.last_query, package == state.last_query_package, now);
                if now >= allowed_at {
                    state.pending = None;
                    // 节流基准以「上次查询的包」为准（失败也算），否则换包后失败/空焦点会永久
                    // 退化为 1s 重试并刷 warn
                    state.last_query_package = package.clone();
                    break package;
                }
                state = match shared
                    .1
                    .wait_timeout(state, allowed_at.saturating_duration_since(now))
                {
                    Ok((guard, _)) => guard,
                    Err(error) => error.into_inner().0,
                };
            }
        };
        let result = query_focus_key();
        // 先更新时间戳并放锁：失败告警与 LAST_ACTIVITY 写入都不在 ActivitySampleState 锁内做 IO
        {
            let mut state = shared.0.lock().unwrap_or_else(|e| e.into_inner());
            if state.stopped {
                return;
            }
            state.last_query = Some(Instant::now());
        }
        match result {
            Ok(key) => {
                let mut slot = LAST_ACTIVITY.lock().unwrap_or_else(|e| e.into_inner());
                if *slot != key {
                    *slot = key;
                }
            }
            Err(error) => {
                // 单次失败可自愈（同包 3s / 换包 1s 后重试），warn 便于真机排查
                log::warn!(
                    "{}",
                    crate::i18n::t_with_args(
                        "activity-sample-failed",
                        &crate::fluent_args!(
                            "pkg" => observed_package,
                            "error" => error.to_string()
                        )
                    )
                );
            }
        }
    }
}

/// 轻量焦点查询：只跑 `dumpsys window`（不取 `window windows` 段），解析 `mCurrentFocus`。
#[cfg(any(target_os = "linux", target_os = "android"))]
fn query_focus_key() -> io::Result<Arc<str>> {
    let snapshot = collect_one(&["-t", "1", "window"], &|| false)?;
    let text = std::str::from_utf8(&snapshot)
        .map_err(|_| io::Error::other("invalid window snapshot encoding"))?;
    Ok(focused_activity_key(text))
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn query_focus_key() -> io::Result<Arc<str>> {
    Err(io::Error::other(
        "window snapshot unsupported on this platform",
    ))
}

// [api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowVisibility {
    Focused,
    VisibleUnfocused,
    Hidden,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowRequest {
    pub package: String,
    pub pid: u32,
    pub generation: u64,
    pub foreground: String,
}

#[derive(Clone, Debug)]
pub struct WindowObservation {
    pub request: WindowRequest,
    pub visibility: WindowVisibility,
    pub observed_at: Instant,
    /// 当前焦点窗口的 Activity 名（`mCurrentFocus` 的 `pkg/Activity` 段，已归一为短形
    /// `pkg/.MainActivity`；无焦点/解析失败为空）。`LAST_ACTIVITY` 由全局 [`ActivitySampler`]
    /// 与 FAS 可见性探针共同刷新，**不再限于 FAS**；消费方仍必须把空值当作「未知」并回退包名级判定。
    pub activity: Arc<str>,
}

pub const OBSERVATION_TTL: Duration = Duration::from_secs(5);
const QUERY_INTERVAL: Duration = Duration::from_secs(3);
const QUERY_TIMEOUT: Duration = Duration::from_millis(800);
const ERROR_BACKOFF: Duration = Duration::from_secs(5);
const UNSUPPORTED_BACKOFF: Duration = Duration::from_secs(15);
const OUTPUT_CAP: usize = 512 * 1024;

pub struct WindowVisibilityProbe {
    shared: Arc<(Mutex<ProbeState>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl WindowVisibilityProbe {
    pub fn new() -> Self {
        let shared = Arc::new((Mutex::new(ProbeState::default()), Condvar::new()));
        let worker_shared = shared.clone();
        let worker = thread::Builder::new()
            .name("window-visibility".into())
            .spawn(move || run_worker(worker_shared))
            .ok();
        if worker.is_none() {
            // [FAS] 窗口查询线程起不来 ⇒ 后续所有 request 都石沉大海，probe.latest() 恒返回
            // None ⇒ reconcile_fas_focus 的 evidence 为空 ⇒ 走 Grace ⇒ 延迟退出 FAS。
            // 现象上「 praying 」和「应用真的失去焦点」完全一致（都不聚焦+无 FPS），
            // 稳态打不出来、日志也没有 ⇒ 排查时必踩坑。这是功能性失效且不可自愈，按 error!。
            log::error!("{}", t("window-probe-worker-failed"));
            shared.0.lock().unwrap().stopped = true;
        }
        Self { shared, worker }
    }

    pub fn request(&self, request: WindowRequest) {
        let mut state = self.shared.0.lock().unwrap();
        if !state.stopped {
            state.enqueue(request);
            self.shared.1.notify_one();
        } else if log::log_enabled!(log::Level::Debug) {
            // [FAS] 探针已停摆（工作线程未起来/已退出）却仍在被请求：请求被静默丢弃，
            // 调用方拿到的 latest() 依旧是 None，与「窗口还没查完」同形。高频路径仅 debug。
            log::debug!("{}", t("window-probe-stopped"));
        }
    }

    pub fn latest(&self, request: &WindowRequest) -> Option<WindowObservation> {
        let state = self.shared.0.lock().unwrap();
        state
            .observation
            .as_ref()
            .filter(|observation| matches_observation(observation, request, Instant::now()))
            .cloned()
    }

    pub fn invalidate(&self) {
        let mut state = self.shared.0.lock().unwrap();
        state.invalidate();
        self.shared.1.notify_one();
    }
}

impl Default for WindowVisibilityProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WindowVisibilityProbe {
    fn drop(&mut self) {
        {
            let mut state = self.shared.0.lock().unwrap();
            state.stopped = true;
            state.invalidate();
            self.shared.1.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

// [worker]
#[derive(Default)]
struct ProbeState {
    revision: u64,
    desired: Option<WindowRequest>,
    pending: Option<WindowRequest>,
    observation: Option<WindowObservation>,
    in_flight: bool,
    stopped: bool,
}

impl ProbeState {
    fn enqueue(&mut self, request: WindowRequest) {
        if self.desired.as_ref() != Some(&request) {
            self.revision = self.revision.wrapping_add(1);
            self.observation = None;
            self.desired = Some(request.clone());
            self.pending = Some(request);
        } else if self.pending.is_none() && !self.in_flight {
            self.pending = Some(request);
        }
    }

    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.desired = None;
        self.pending = None;
        self.observation = None;
    }

    fn publish(&mut self, revision: u64, observation: WindowObservation) {
        if !self.stopped
            && revision == self.revision
            && self.desired.as_ref() == Some(&observation.request)
        {
            self.observation = Some(observation);
        }
    }
}

/// worker 退出哨兵：Drop 无条件置位 `stopped` 并失效全部状态、唤醒等待者。
/// 覆盖 worker 的两条死亡路径 —— 正常 return（probe Drop 已先行置位 ⇒ 幂等）与
/// 运行期 panic unwind（此前无人置位 ⇒ 唯一补救）。缺了后者：worker 线程 panic 死亡后
/// `stopped` 恒 false，`request()` 恒走 enqueue、请求静默石沉大海、`latest()` 恒 None，
/// `:86-90` 的 `window-probe-stopped` 分支永不可达 —— 与「启动失败」同级的静默失效，
/// 而 ftl 文案 `window-probe-worker-failed`（启动/运行失败）的「运行」一半无留痕。
/// 锁可能已被 worker panic 毒化：Drop 里禁 unwrap（Drop 中再 panic = abort）。
struct WorkerStopGuard(Arc<(Mutex<ProbeState>, Condvar)>);

impl Drop for WorkerStopGuard {
    fn drop(&mut self) {
        let mut state = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        state.stopped = true;
        state.invalidate();
        self.0.1.notify_one();
    }
}

fn run_worker(shared: Arc<(Mutex<ProbeState>, Condvar)>) {
    run_worker_with_query(shared, query_visibility);
}

fn run_worker_with_query(
    shared: Arc<(Mutex<ProbeState>, Condvar)>,
    query: impl Fn(&WindowRequest, &dyn Fn() -> bool) -> io::Result<WindowVisibility>,
) {
    // panic 隔离：与 `mod.rs` 的 `spawn_guarded` 同属 monitor 口径 —— worker 循环任何 panic
    // 都就地收场（catch_unwind），随后 guard Drop 置位 stopped，`request()` 从此转入
    // `window-probe-stopped` 分支，不再静默吞请求。error 只补一次：若 panic 时 stopped 已为
    // true，说明启动失败路径（:70-77 已打 `window-probe-worker-failed`）或 probe 自身 Drop
    // 已先收尾——前者已有留痕，后者意味着探针正在析构、调用方即将不再使用它，均无需再报。
    let guard = WorkerStopGuard(shared.clone());
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        worker_loop(shared.clone(), query)
    }))
    .is_err();
    if panicked && !shared.0.lock().unwrap_or_else(|e| e.into_inner()).stopped {
        log::error!("{}", t("window-probe-worker-failed"));
    }
    drop(guard);
}

/// worker 主体循环（由 `run_worker_with_query` 在 catch_unwind 内调用）。
fn worker_loop(
    shared: Arc<(Mutex<ProbeState>, Condvar)>,
    query: impl Fn(&WindowRequest, &dyn Fn() -> bool) -> io::Result<WindowVisibility>,
) {
    let mut next_query = Instant::now();
    loop {
        let (request, revision) = {
            let mut state = shared.0.lock().unwrap();
            loop {
                if state.stopped {
                    return;
                }
                let now = Instant::now();
                if state.pending.is_some() && now >= next_query {
                    state.in_flight = true;
                    break (state.pending.take().unwrap(), state.revision);
                }
                state = if state.pending.is_some() {
                    shared
                        .1
                        .wait_timeout(state, next_query.saturating_duration_since(now))
                        .unwrap()
                        .0
                } else {
                    shared.1.wait(state).unwrap()
                };
            }
        };
        let observed_at = Instant::now();
        let cancelled = || {
            let state = shared.0.lock().unwrap();
            state.stopped || state.revision != revision
        };
        let result = query(&request, &cancelled);
        // [FAS] 查询失败原本是完全静默的：只体现在 backoff 变长、observation 不更新。
        // 下游 `reconcile_fas_focus` 拿到 `None` 证据就走 Grace ⇒ 关帧反馈 + 延迟退出 FAS，
        // 和「应用真的失去焦点」在调度行为、status.csv 输出上完全同形，排查时无从区分。
        // 这里补一条 warn：单次失败可自愈（下次 3~5s 后重试），故不提 error。
        if let Err(error) = &result {
            log::warn!(
                "{}",
                crate::i18n::t_with_args(
                    "window-probe-query-failed",
                    &crate::fluent_args!(
                        "pkg" => request.package.to_string(),
                        "pid" => request.pid.to_string(),
                        "error" => error.to_string()
                    )
                )
            );
        }
        let delay = match result {
            Ok(WindowVisibility::Unknown) => UNSUPPORTED_BACKOFF,
            Ok(_) => QUERY_INTERVAL,
            Err(_) => ERROR_BACKOFF,
        };
        let mut state = shared.0.lock().unwrap();
        next_query = Instant::now()
            + if state.revision == revision {
                delay
            } else {
                QUERY_INTERVAL
            };
        state.in_flight = false;
        if let Ok(visibility) = result {
            state.publish(
                revision,
                WindowObservation {
                    request,
                    visibility,
                    observed_at,
                    activity: last_activity_key(),
                },
            );
        }
    }
}

fn matches_observation(
    observation: &WindowObservation,
    request: &WindowRequest,
    now: Instant,
) -> bool {
    observation.request == *request
        && now
            .checked_duration_since(observation.observed_at)
            .is_some_and(|age| age < OBSERVATION_TTL)
}

// [query]
fn read_process_uid(pid: u32) -> io::Result<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|values| values.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| io::Error::other("missing process uid"))
}

fn query_visibility(
    request: &WindowRequest,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<WindowVisibility> {
    if request.pid == 0 || request.package.is_empty() || cancelled() {
        return Err(io::Error::other("invalid or cancelled request"));
    }
    let uid = read_process_uid(request.pid)?;
    let process_start = read_process_start(request.pid)?;
    let output = collect_snapshot(cancelled)?;
    if read_process_uid(request.pid)? != uid
        || read_process_start(request.pid)? != process_start
        || cancelled()
    {
        return Err(io::Error::other("process changed during query"));
    }
    let snapshot = std::str::from_utf8(&output)
        .map_err(|_| io::Error::other("invalid window snapshot encoding"))?;
    {
        let mut slot = LAST_ACTIVITY.lock().unwrap();
        let fresh = focused_activity_key(snapshot);
        if *slot != fresh {
            *slot = fresh;
        }
    }
    Ok(parse_windows(snapshot, request, uid))
}

fn read_process_start(pid: u32) -> io::Result<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    status
        .rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| io::Error::other("missing process start time"))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn make_nonblocking(pipe: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    use std::ffi::c_int;
    unsafe extern "C" {
        fn fcntl(descriptor: c_int, command: c_int, ...) -> c_int;
    }
    let descriptor = pipe.as_raw_fd();
    // SAFETY: The live pipe owns this descriptor; F_GETFL takes no variadic argument.
    let flags = unsafe { fcntl(descriptor, 3) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: F_SETFL takes an integer flag mask; O_NONBLOCK is 0x800 on Linux/Android.
    if unsafe { fcntl(descriptor, 4, flags | 0x800) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn collect_snapshot(cancelled: &dyn Fn() -> bool) -> io::Result<Vec<u8>> {
    // 两条 dumpsys 子命令**互补**，实测（8550/PHB110, ColorOS）：
    //   `window`          → 有 mCurrentFocus/mFocusedApp，**没有** mObscured/mViewVisibility
    //   `window windows`  → 有 mObscured/mViewVisibility，**没有** mCurrentFocus/mFocusedApp
    // 只用 `window windows`（旧实现）会让焦点判定恒不可达（Focused 永不成立）；只用 `window`
    // 会让遮挡字段缺失、parse_windows 的 match 落 `_` 分支而连 VisibleUnfocused/Hidden 都拿不到。
    // 故两段都取，用分隔标记拼接，由 parse_windows 按段解析。
    // 预算：单命令实测 简单场景 79/63ms、复杂场景（多叠加窗口 + 输入法）180/114ms；
    // 两次**各自**给 QUERY_TIMEOUT(800ms) 预算（最坏 1600ms），而**不是**共享一份——
    // 共享会让第一段吃掉大部分预算后第二段只剩零头，复杂场景实测已达 294ms 合计，
    // 低端机/更多窗口时第二段会先超时 → 整次查询失败 → Unknown → Grace（误判）。
    // cancelled() 在上层被取代时会提前中止，故实际延迟由请求生命周期约束，不会长期占满。
    // 输出上限：单段 ≤512KB，两段合计 ≤2×OUTPUT_CAP。
    let focus_segment = collect_one(&["-t", "1", "window"], cancelled)?;
    let visibility_segment = collect_one(&["-t", "1", "window", "windows"], cancelled)?;
    // 注意：不再共享 deadline —— 每段在 collect_one 内各自起算 QUERY_TIMEOUT
    let mut merged = Vec::with_capacity(focus_segment.len() + visibility_segment.len() + 32);
    merged.extend_from_slice(&focus_segment);
    merged.extend_from_slice(SEGMENT_MARKER);
    merged.extend_from_slice(&visibility_segment);
    Ok(merged)
}

/// 两段 dumpsys 输出之间的分隔标记：以 dumpsys 不可能产出的行形式出现，
/// 便于 `parse_windows` 按段取字段而不会把两段混读。
const SEGMENT_MARKER: &[u8] = b"\n== CHIRI-DUMPSYS-SEGMENT ==\n";

#[cfg(any(target_os = "linux", target_os = "android"))]
fn collect_one(args: &[&str], cancelled: &dyn Fn() -> bool) -> io::Result<Vec<u8>> {
    // 本段自己的预算：不与其他段共享，避免「第一段耗时挤掉第二段」
    let deadline = Instant::now() + QUERY_TIMEOUT;
    let mut child = Command::new("/system/bin/dumpsys")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| {
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing stdout"))?;
        make_nonblocking(&stdout)?;
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];
        let mut reached_eof = false;
        loop {
            if cancelled() || Instant::now() >= deadline {
                return Err(io::Error::other("window query cancelled or timed out"));
            }
            match stdout.read(&mut buffer) {
                Ok(0) => reached_eof = true,
                Ok(count) => {
                    if output.len() + count > OUTPUT_CAP {
                        return Err(io::Error::other("window snapshot output cap exceeded"));
                    }
                    output.extend_from_slice(&buffer[..count]);
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(io::Error::other("dumpsys failed"));
                }
                if reached_eof {
                    return Ok(output);
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
    })();
    // Every completion path reaps the direct child, including setup and read failures.
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn collect_snapshot(_cancelled: &dyn Fn() -> bool) -> io::Result<Vec<u8>> {
    Err(io::Error::other(
        "window queries unsupported on this platform",
    ))
}

// [parse]
/// `Window{token u0 pkg/Activity}` 的公共解析：拆出 `(token, package, activity)`。
/// 窗口可见性判定与焦点 Activity 提取共用同一口径，避免解析逻辑分叉。
/// 校验：恰好 3 个字段、第二字段必须是合法 `u<user>`、组件必须含 `/`。
/// `activity` 为 `/` 后的原文（未归一），**允许为空**（`pkg/` 形式）——调用方按需拒绝。
fn window_component(text: &str) -> Option<(&str, &str, &str)> {
    let body = text.strip_prefix("Window{")?.split_once('}')?.0;
    let mut fields = body.split_whitespace();
    let token = fields.next()?;
    let user = fields.next()?.strip_prefix('u')?;
    user.parse::<u32>().ok()?;
    let component = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    let (package, activity) = component.split_once('/')?;
    (!package.is_empty()).then_some((token, package, activity))
}

fn window_identity(text: &str) -> Option<(&str, &str)> {
    let (token, package, _) = window_component(text)?;
    Some((token, package))
}

/// 把 dumpsys 的 `pkg/Activity` 归一成**查表键短形** `pkg/.Activity`：
/// - `.MainActivity`（同包内短形）原样；
/// - `com.xx.MainActivity`（全限定且以包名开头）剥掉包名前缀 → `.MainActivity`；
/// - 其余（外部包、含 `$` 的内部类、裸类名）保留原样，内部类**不剥 `$`**（同一 Activity
///   的不同内部类语义不同）。
/// 归一后同一 Activity 的两种写法落到同一键，用户从 `dumpsys`/`am` 复制任一形式都可用。
pub fn activity_key(package: &str, activity: &str) -> String {
    let short = match activity.strip_prefix(package) {
        // `rest.starts_with('.')` 守卫防「包名前缀相同但不是该包」的误坍缩：
        // com.example.game + com.example.gameX.Main → rest = "X.Main"，不剥前缀、保留全串
        Some(rest) if rest.starts_with('.') && rest.len() > 1 => rest,
        _ => activity,
    };
    let short = if short.starts_with('.') || short.contains('.') {
        short.to_string()
    } else {
        format!(".{short}")
    };
    format!("{package}/{short}")
}

/// 取一段文本里唯一的 `mCurrentFocus=` 值；0 条或多条（快照歧义）都返回 None。
fn unique_focus(segment: &str) -> Option<&str> {
    let mut focus = segment
        .lines()
        .filter_map(|line| line.trim().strip_prefix("mCurrentFocus="))
        .map(str::trim);
    let first = focus.next()?;
    focus.next().is_none().then_some(first)
}

/// 从 dumpsys 快照里取当前焦点窗口的 Activity 查表键；无唯一焦点或解析失败返回空
/// （调用方按「未知」处理并回退包名级判定）。
/// 合并快照下焦点来自 `window` 段，片段缺失时回退到整份（兼容旧快照/测试注入）。
fn focused_activity_key(snapshot: &str) -> Arc<str> {
    let (focus_segment, visibility_segment) = split_segments(snapshot);
    let Some(first) = unique_focus(focus_segment).or_else(|| unique_focus(visibility_segment))
    else {
        return Arc::from("");
    };
    match window_component(first) {
        // 空 activity（`pkg/`）视为未知：没有可查表的键
        Some((_, package, activity)) if !activity.is_empty() => {
            Arc::from(activity_key(package, activity))
        }
        _ => Arc::from(""),
    }
}

fn field<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let mut values = block
        .split_whitespace()
        .filter_map(|token| token.strip_prefix(name));
    let value = values.next()?;
    if values.any(|other| other != value) {
        return None;
    }
    Some(value)
}

/// `u0a10457` / `10457` 两种写法都归一成同一个 Android uid。
///
/// AOSP 的 AID 文本形式是 `u<userId>a<appId>`，其中 uid = userId*100000 + appId。
/// **appId 在文本里已经是最终值**（含 AID 区偏移），不要再加 10000 ——
/// 真机同块对照证据（8550/PHB110，两次独立采样）：
///   `mSession=Session{… u0a10231}` 与同块 `mOwnerUid=10231`  → 必须都归一到 10231；
///   `mSession=Session{… u0a10457}` 与同块 `mOwnerUid=10457`  → 必须都归一到 10457。
/// 旧实现多加了 10000（算成 20231/20457），与 `mOwnerUid` 侧恒不相等，
/// 使 `window_session(&block) != Some((pid, uid))` 永远成立 → 窗口全部计入 `unknown_owned`
/// → `owned_count == 0` → 恒返回 `Unknown`（`Focused`/`VisibleUnfocused` 都不可达）。
fn android_uid(value: &str) -> Option<u32> {
    if let Ok(uid) = value.parse() {
        return Some(uid);
    }
    let body = value.strip_prefix('u')?;
    let (user, application) = body.split_once('a')?;
    let user = user.parse::<u32>().ok()?;
    let app_id = application.parse::<u32>().ok()?;
    // 多用户：userId*100000 + appId；单用户 userId=0 时即 appId 本身
    user.checked_mul(100000)?.checked_add(app_id)
}

/// 从窗口块里取 `(pid, uid)`。
///
/// 旧实现要求 `mSession=Session{` 位于**行首**（`line.trim().strip_prefix(...)`），但真机
/// （8550/PHB110）该字段是**行内**出现的：
///   `    mDisplayId=0 taskId=851 mSession=Session{24fa3ae 5630:u0a10457} mClient=…`
/// → 行首前缀匹配失败 → 返回 None → 该窗口被判 `unknown_owned` → `owned_count == 0`
/// → 恒 `Unknown`（`Focused`/`VisibleUnfocused` 都不可达）。旧测试 fixture 恰好把
/// `mSession` 写在行首，掩盖了真实形态。
/// 现改为按**子串**定位，并保留「恰好一条 session」的歧义拒绝。
fn window_session(block: &str) -> Option<(u32, u32)> {
    let mut sessions = block.lines().filter_map(|line| {
        line.trim()
            .split_once("mSession=Session{")
            .map(|(_, rest)| rest)
    });
    let body = sessions.next()?.split_once('}')?.0;
    if sessions.next().is_some() {
        return None;
    }
    let mut fields = body.split_whitespace();
    fields.next()?;
    let (pid, uid) = fields.next()?.split_once(':')?;
    if fields.next().is_some() {
        return None;
    }
    Some((pid.parse().ok()?, android_uid(uid)?))
}

/// 按 `SEGMENT_MARKER` 把合并快照拆成两段。只有一段时（旧快照/测试注入）视为「可见性段」，
/// 兼容原有单命令行为。
fn split_segments(snapshot: &str) -> (&str, &str) {
    match snapshot.split_once(SEGMENT_TEXT) {
        // 段序：先 `window`（焦点），后 `window windows`（可见性）
        Some((focus, visibility)) => (focus, visibility),
        None => (snapshot, snapshot),
    }
}

const SEGMENT_TEXT: &str = "== CHIRI-DUMPSYS-SEGMENT ==";

fn parse_windows(snapshot: &str, request: &WindowRequest, uid: u32) -> WindowVisibility {
    // 两段来源不同：靠 `WINDOW MANAGER WINDOWS` 头是否存在判定可用性。
    // 任一段缺少该头不直接判 Unknown —— 真机上 `window` 段实测含该头（计数 1），
    // 但若某 ROM 两者都缺，仍应回退 Unknown 而不是误判为 Hidden。
    if !snapshot
        .lines()
        .any(|line| line.trim() == "WINDOW MANAGER WINDOWS (dumpsys window windows)")
    {
        return WindowVisibility::Unknown;
    }
    let (focus_segment, visibility_segment) = split_segments(snapshot);
    // 焦点**只**从 `window` 段取（`window windows` 段实测没有 mCurrentFocus；
    // 若某 ROM 反转，则两段都能命中，见下方回退）
    let focused = unique_focus(focus_segment)
        .or_else(|| unique_focus(visibility_segment))
        .and_then(window_identity);
    // 可见性字段（mObscured/mViewVisibility/mHasSurface/isOnScreen）从 `window windows` 段取。
    // 该段缺失时回退到整份快照，保持旧行为。
    let visibility_source = if visibility_segment.trim().is_empty() {
        snapshot
    } else {
        visibility_segment
    };
    // [token] 焦点来自焦点段、窗口块来自可见性段，判据仍是 `token` 相等。
    // 若某 ROM 两段 token 编码不一致，Focused 会不可达 —— 这**必须靠真机数据确认**
    // （见 devimpbin/1016-001727/probe3.sh 的 C 段）。这里**不做**包名放宽：包名宽松匹配
    // 会让「叠加窗口盖住游戏」时错判 Focused，直接破坏「有叠加画面时退出是预期行为」。
    let lines = visibility_source.lines().collect::<Vec<_>>();
    let mut owned_count = 0;
    let mut unknown_owned = false;
    let mut visible = false;
    let mut obscured = false;
    for (index, line) in lines.iter().enumerate() {
        // 兼容两种格式：`Window #0 Window{token u0 pkg/Act}`（window windows）
        // 与 `Window #0: WindowStateAnimator{...}`（window，带冒号）。后者无组件名，
        // 其可见性信息在 `window windows` 段里已覆盖，故解析不出组件时跳过而不计 unknown。
        let Some(header) = line.trim().strip_prefix("Window #") else {
            continue;
        };
        let Some((number, identity)) = header.split_once(' ') else {
            continue;
        };
        if number.parse::<u32>().is_err() {
            continue;
        }
        let Some((token, package)) = window_identity(identity) else {
            continue;
        };
        if package != request.package {
            continue;
        }
        let end = lines[index + 1..]
            .iter()
            .position(|line| {
                let text = line.trim();
                text.starts_with("Window #") || text.starts_with("mCurrentFocus=")
            })
            .map_or(lines.len(), |offset| index + 1 + offset);
        let block = lines[index + 1..end].join("\n");
        if window_session(&block) != Some((request.pid, uid))
            || field(&block, "mOwnerUid=").and_then(android_uid) != Some(uid)
        {
            unknown_owned = true;
            continue;
        }
        owned_count += 1;
        let surface = field(&block, "mHasSurface=");
        let on_screen = field(&block, "isOnScreen=").or_else(|| {
            match (
                field(&block, "isReadyForDisplay()="),
                field(&block, "mViewVisibility="),
            ) {
                (Some("true"), Some("0x0")) => Some("true"),
                (Some("false"), Some("0x0" | "0x4" | "0x8")) => Some("false"),
                (Some("true"), Some("0x4" | "0x8")) => Some("false"),
                _ => None,
            }
        });
        match (surface, on_screen, field(&block, "mObscured=")) {
            (Some("true"), Some("true"), Some("false")) => {
                if focused == Some((token, package)) {
                    return WindowVisibility::Focused;
                }
                visible = true;
            }
            // A fully obscured game window is still visible to the window service but must not keep load-only alive.
            (_, Some(_), Some("true")) => obscured = true,
            (Some("false"), Some("false"), _) | (Some("true"), Some("false"), _) => {}
            _ => unknown_owned = true,
        }
    }
    if visible {
        WindowVisibility::VisibleUnfocused
    } else if obscured {
        WindowVisibility::Hidden
    } else if owned_count > 0 && !unknown_owned {
        WindowVisibility::Hidden
    } else {
        WindowVisibility::Unknown
    }
}

// [tests]
#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> WindowRequest {
        WindowRequest {
            package: "com.example.game".into(),
            pid: 42,
            generation: 7,
            foreground: "com.android.systemui".into(),
        }
    }

    fn snapshot(surface: bool, on_screen: bool) -> String {
        format!(
            "WINDOW MANAGER WINDOWS (dumpsys window windows)\n  Window #0 Window{{abc u0 com.example.game/.Main}}:\n    mSession=Session{{def 42:10042}}\n    mOwnerUid=10042\n    mHasSurface={surface} isOnScreen={on_screen} mObscured=false\n  mCurrentFocus=Window{{abc u0 com.example.game/.Main}}\n"
        )
    }

    #[test]
    fn focused_requires_visible_owned_window() {
        assert_eq!(
            parse_windows(&snapshot(true, true), &request(), 10042),
            WindowVisibility::Focused
        );
        assert_eq!(
            parse_windows(&snapshot(false, false), &request(), 10042),
            WindowVisibility::Hidden
        );
    }

    #[test]
    fn overlay_keeps_visible_unfocused_distinct() {
        let output = snapshot(true, true).replace(
            "mCurrentFocus=Window{abc u0 com.example.game/.Main}",
            "mCurrentFocus=Window{overlay u0 com.android.systemui/.Panel}",
        );
        assert_eq!(
            parse_windows(&output, &request(), 10042),
            WindowVisibility::VisibleUnfocused
        );
    }

    #[test]
    fn rejects_substrings_wrong_pid_and_wrong_uid() {
        for output in [
            snapshot(true, true).replace("com.example.game/", "com.example.game.fake/"),
            snapshot(true, true).replace("42:10042", "43:10042"),
            snapshot(true, true).replace("mOwnerUid=10042", "mOwnerUid=10043"),
        ] {
            assert_eq!(
                parse_windows(&output, &request(), 10042),
                WindowVisibility::Unknown
            );
        }
    }

    #[test]
    fn missing_window_and_unsupported_fields_are_unknown() {
        for output in [
            "mCurrentFocus=Window{abc u0 com.example.game/.Main}".into(),
            snapshot(true, true).replace("isOnScreen=true", "vendorVisible=true"),
            snapshot(true, true).replace("mSession=Session{def 42:10042}", "mSession=unsupported"),
        ] {
            assert_eq!(
                parse_windows(&output, &request(), 10042),
                WindowVisibility::Unknown
            );
        }
    }

    #[test]
    fn fresh_observations_match_every_session_key() {
        let now = Instant::now();
        let observation = WindowObservation {
            request: request(),
            visibility: WindowVisibility::Focused,
            observed_at: now,
            activity: Arc::from("com.example.game/.MainActivity"),
        };
        assert!(matches_observation(&observation, &request(), now));
        let mut changed = request();
        changed.generation += 1;
        assert!(!matches_observation(&observation, &changed, now));
        changed = request();
        changed.pid += 1;
        assert!(!matches_observation(&observation, &changed, now));
        changed = request();
        changed.foreground.clear();
        assert!(!matches_observation(&observation, &changed, now));
        changed = request();
        changed.package.push_str(".other");
        assert!(!matches_observation(&observation, &changed, now));
        assert!(!matches_observation(
            &observation,
            &request(),
            now + OBSERVATION_TTL
        ));
    }

    #[test]
    fn pending_requests_coalesce_and_invalidation_rejects_inflight() {
        let mut state = ProbeState::default();
        state.enqueue(request());
        let original_revision = state.revision;
        let mut newer = request();
        newer.generation += 1;
        state.enqueue(newer.clone());
        assert_eq!(state.pending, Some(newer));
        state.invalidate();
        state.enqueue(request());
        state.publish(
            original_revision,
            WindowObservation {
                request: request(),
                visibility: WindowVisibility::Focused,
                observed_at: Instant::now(),
                activity: Arc::from(""),
            },
        );
        assert!(state.observation.is_none());
    }

    #[test]
    fn duplicate_inflight_request_does_not_schedule_another_query() {
        let mut state = ProbeState::default();
        state.enqueue(request());
        state.pending.take();
        state.in_flight = true;
        let revision = state.revision;
        state.enqueue(request());
        assert_eq!(state.revision, revision);
        assert!(state.pending.is_none());
    }

    #[test]
    fn async_worker_coalesces_and_drops_invalidated_snapshot() {
        use std::sync::mpsc;
        let shared = Arc::new((Mutex::new(ProbeState::default()), Condvar::new()));
        let mut newer = request();
        newer.generation += 1;
        {
            let mut state = shared.0.lock().unwrap();
            state.enqueue(request());
            state.enqueue(newer.clone());
        }
        let (started_sender, started_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let worker_shared = shared.clone();
        let worker = thread::spawn(move || {
            run_worker_with_query(worker_shared, |request, _cancelled| {
                started_sender.send(request.clone()).unwrap();
                release_receiver.recv().unwrap();
                Ok(WindowVisibility::Focused)
            })
        });
        assert_eq!(
            started_receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            newer
        );
        {
            let mut state = shared.0.lock().unwrap();
            state.invalidate();
            state.enqueue(newer);
        }
        release_sender.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            let mut state = shared.0.lock().unwrap();
            if !state.in_flight {
                assert!(state.observation.is_none());
                assert!(state.pending.is_some());
                state.stopped = true;
                shared.1.notify_one();
                break;
            }
            assert!(Instant::now() < deadline);
            drop(state);
            thread::yield_now();
        }
        worker.join().unwrap();
    }

    #[test]
    fn ambiguous_owned_window_prevents_hidden_but_not_positive_visibility() {
        let hidden = snapshot(false, false);
        let ambiguous = "  Window #1 Window{other u0 com.example.game/.Popup}:\n    mSession=Session{def 42:10042}\n    mOwnerUid=10042\n    mHasSurface=true vendorVisible=false\n";
        assert_eq!(
            parse_windows(&(hidden + ambiguous), &request(), 10042),
            WindowVisibility::Unknown
        );
        assert_eq!(
            parse_windows(&(snapshot(true, true) + ambiguous), &request(), 10042),
            WindowVisibility::Focused
        );
    }

    #[test]
    fn contradictory_flags_or_wrong_focus_token_cannot_prove_focus() {
        let contradictory =
            snapshot(true, true).replace("isOnScreen=true", "isOnScreen=true isOnScreen=false");
        assert_eq!(
            parse_windows(&contradictory, &request(), 10042),
            WindowVisibility::Unknown
        );
        let wrong_token =
            snapshot(true, true).replace("mCurrentFocus=Window{abc", "mCurrentFocus=Window{other");
        assert_eq!(
            parse_windows(&wrong_token, &request(), 10042),
            WindowVisibility::VisibleUnfocused
        );
    }

    #[test]
    fn aosp_display_readiness_requires_explicit_view_visibility() {
        let ready = snapshot(true, true).replace(
            "isOnScreen=true",
            "isReadyForDisplay()=true mViewVisibility=0x0",
        );
        assert_eq!(
            parse_windows(&ready, &request(), 10042),
            WindowVisibility::Focused
        );
        assert_eq!(
            parse_windows(
                &ready.replace(" mViewVisibility=0x0", ""),
                &request(),
                10042
            ),
            WindowVisibility::Unknown
        );
        assert_eq!(
            parse_windows(
                &ready.replace("mViewVisibility=0x0", "mViewVisibility=0x8"),
                &request(),
                10042
            ),
            WindowVisibility::Hidden
        );
    }

    #[test]
    fn fully_obscured_visible_window_is_not_visible_unfocused() {
        let obscured = snapshot(true, true)
            .replace("mObscured=false", "mObscured=true")
            .replace(
                "mCurrentFocus=Window{abc u0 com.example.game/.Main}",
                "mCurrentFocus=Window{other u0 com.example.other/.Activity}",
            );
        assert_eq!(
            parse_windows(&obscured, &request(), 10042),
            WindowVisibility::Hidden
        );
    }

    #[test]
    fn missing_obscured_evidence_cannot_sustain_visible_unfocused() {
        let missing = snapshot(true, true)
            .replace(" mObscured=false", "")
            .replace(
                "mCurrentFocus=Window{abc u0 com.example.game/.Main}",
                "mCurrentFocus=Window{other u0 com.example.other/.Activity}",
            );
        assert_eq!(
            parse_windows(&missing, &request(), 10042),
            WindowVisibility::Unknown
        );
    }
}

// [activity] Activity 级前台识别：解析 / 归一 / 回退
#[cfg(test)]
mod activity_tests {
    use super::*;

    fn request() -> WindowRequest {
        WindowRequest {
            package: "com.example.game".into(),
            pid: 42,
            generation: 7,
            foreground: "com.android.systemui".into(),
        }
    }

    fn snapshot(surface: bool, on_screen: bool) -> String {
        format!(
            "WINDOW MANAGER WINDOWS (dumpsys window windows)\n  Window #0 Window{{abc u0 com.example.game/.Main}}:\n    mSession=Session{{def 42:10042}}\n    mOwnerUid=10042\n    mHasSurface={surface} isOnScreen={on_screen} mObscured=false\n  mCurrentFocus=Window{{abc u0 com.example.game/.Main}}\n"
        )
    }

    #[test]
    fn short_form_activity_is_normalized_to_key() {
        assert_eq!(
            activity_key("com.example.game", ".MainActivity"),
            "com.example.game/.MainActivity"
        );
    }

    #[test]
    fn fully_qualified_activity_collapses_to_same_key() {
        assert_eq!(
            activity_key("com.example.game", "com.example.game.MainActivity"),
            "com.example.game/.MainActivity"
        );
        // 两种写法必须落到同一键，否则用户按 dumpsys 原文配置会失配
        assert_eq!(
            activity_key("com.example.game", ".MainActivity"),
            activity_key("com.example.game", "com.example.game.MainActivity")
        );
    }

    #[test]
    fn bare_class_name_gets_leading_dot() {
        assert_eq!(
            activity_key("com.example.game", "MainActivity"),
            "com.example.game/.MainActivity"
        );
    }

    #[test]
    fn inner_class_dollar_is_preserved() {
        // `$` 不剥：同一 Activity 的不同内部类语义不同
        assert_eq!(
            activity_key("com.example.game", ".Main$Sub"),
            "com.example.game/.Main$Sub"
        );
    }

    #[test]
    fn focused_activity_key_reads_unique_current_focus() {
        assert_eq!(
            focused_activity_key(&snapshot(true, true)).as_ref(),
            "com.example.game/.Main"
        );
    }

    #[test]
    fn focused_activity_key_empty_without_component_slash() {
        // 焦点窗口没有 pkg/Activity 组件名（系统窗口）→ 未知，调用方回退包名级
        let no_slash = "WINDOW MANAGER WINDOWS (dumpsys window windows)\n  mCurrentFocus=Window{abc u0 com.example.game}\n";
        assert_eq!(focused_activity_key(no_slash).as_ref(), "");
    }

    #[test]
    fn focused_activity_key_empty_on_ambiguous_multiple_focus_lines() {
        let two = "WINDOW MANAGER WINDOWS (dumpsys window windows)\n  mCurrentFocus=Window{a u0 com.a/.A}\n  mCurrentFocus=Window{b u0 com.b/.B}\n";
        assert_eq!(focused_activity_key(two).as_ref(), "");
    }

    #[test]
    fn focused_activity_key_empty_when_missing() {
        assert_eq!(focused_activity_key("nothing here").as_ref(), "");
    }

    // [merged] 双查询合并快照：焦点来自 `window` 段、可见性来自 `window windows` 段。
    // 真机依据见 HANDOFF.md 第 10.1 节（`window windows` 无 mCurrentFocus，`window` 无 mObscured）。
    fn merged(focus_lines: &str, vis_body: &str) -> String {
        format!(
            "WINDOW MANAGER WINDOWS (dumpsys window windows)\n{focus_lines}\n== CHIRI-DUMPSYS-SEGMENT ==\nWINDOW MANAGER WINDOWS (dumpsys window windows)\n{vis_body}\n"
        )
    }

    #[test]
    fn merged_snapshot_takes_focus_from_first_segment() {
        // 第一段（window）有 mCurrentFocus 与冒号格式的 Window #；第二段（window windows）没有焦点
        // 注意 token 必须两段一致（真实 dumpsys 是同一个 window token；不一致会正确地判为未聚焦）
        let snap = merged(
            "  Window #0: WindowStateAnimator{14d9077 com.android.systemui.wallpapers.ImageWallpaper}\n  mCurrentFocus=Window{abc u0 com.example.game/.Main}",
            "  Window #0 Window{abc u0 com.example.game/.Main}:\n    mSession=Session{def 42:10042}\n    mOwnerUid=10042\n    mHasSurface=true isOnScreen=true mObscured=false",
        );
        assert_eq!(
            focused_activity_key(&snap).as_ref(),
            "com.example.game/.Main"
        );
        assert_eq!(
            parse_windows(&snap, &request(), 10042),
            WindowVisibility::Focused,
            "focus from segment 1 + visibility fields from segment 2 must prove Focused"
        );
    }

    #[test]
    fn merged_snapshot_obscured_from_second_segment_wins() {
        // 第二段 mObscured=true（第一段完全没有该字段）→ 必须判 Hidden，而不是 Unknown/VisibleUnfocused
        let snap = merged(
            "  mCurrentFocus=Window{abc u0 com.example.game/.Main}",
            "  Window #0 Window{abc u0 com.example.game/.Main}:\n    mSession=Session{def 42:10042}\n    mOwnerUid=10042\n    mHasSurface=true isOnScreen=true mObscured=true",
        );
        assert_eq!(
            parse_windows(&snap, &request(), 10042),
            WindowVisibility::Hidden
        );
    }

    #[test]
    fn merged_snapshot_visible_unfocused_when_focus_is_other_package() {
        let snap = merged(
            "  mCurrentFocus=Window{zzz u0 com.other.app/.A}",
            "  Window #0 Window{abc u0 com.example.game/.Main}:\n    mSession=Session{def 42:10042}\n    mOwnerUid=10042\n    mHasSurface=true isOnScreen=true mObscured=false",
        );
        assert_eq!(
            parse_windows(&snap, &request(), 10042),
            WindowVisibility::VisibleUnfocused
        );
    }

    #[test]
    fn single_segment_snapshot_keeps_legacy_behaviour() {
        // 无分隔标记（旧快照/测试注入）：两段都取整份，行为与改动前一致
        assert_eq!(
            parse_windows(&snapshot(true, true), &request(), 10042),
            WindowVisibility::Focused
        );
        assert_eq!(
            parse_windows(&snapshot(false, false), &request(), 10042),
            WindowVisibility::Hidden
        );
        assert_eq!(
            parse_windows(
                &"WINDOW MANAGER WINDOWS (dumpsys window windows)\n  nothing\n",
                &request(),
                10042
            ),
            WindowVisibility::Unknown
        );
    }

    #[test]
    fn aid_uid_form_matches_owner_uid_in_same_block() {
        // 真机证据（8550/PHB110，两次独立采样）：同块内 `u0aNNNNN` 与 `mOwnerUid=NNNNN` 必须相等。
        // 旧实现给 AID 形式多加 10000 → 两者恒不等 → 所有窗口落 unknown_owned → 恒 Unknown。
        assert_eq!(android_uid("u0a10457"), Some(10457));
        assert_eq!(android_uid("u0a10231"), Some(10231));
        assert_eq!(android_uid("10457"), Some(10457));
        // 多用户仍按 userId*100000 + appId
        assert_eq!(android_uid("u10a10457"), Some(10 * 100000 + 10457));
    }

    #[test]
    fn real_device_window_block_proves_focused() {
        // 用真机 D 段原文构造的块（AID 形式 uid + 无 isOnScreen= 字段，靠 isReadyForDisplay()+mViewVisibility）
        let block = "WINDOW MANAGER WINDOWS (dumpsys window windows)\n  Window #18 Window{b1b876b u0 com.levelinfinite.sgameGlobal/com.levelinfinite.sgameGlobal.SGameGlobalActivity}:\n    mDisplayId=0 taskId=851 mSession=Session{24fa3ae 5630:u0a10457} mClient=android.os.BinderProxy@9f64cba\n    mOwnerUid=10457 showForAllUsers=false package=com.levelinfinite.sgameGlobal appop=NONE\n    mHasSurface=true isReadyForDisplay()=true mWindowRemovalAllowed=false\n    mViewVisibility=0x0 mHaveFrame=true mObscured=false\n  mCurrentFocus=Window{b1b876b u0 com.levelinfinite.sgameGlobal/com.levelinfinite.sgameGlobal.SGameGlobalActivity}\n";
        let req = WindowRequest {
            package: "com.levelinfinite.sgameGlobal".into(),
            pid: 5630,
            generation: 7,
            foreground: "".into(),
        };
        // uid 参数这里传 10457（= android_uid("u0a10457")），必须判 Focused
        assert_eq!(
            parse_windows(&block, &req, android_uid("u0a10457").unwrap()),
            WindowVisibility::Focused,
            "real device block with AID-form uid must prove Focused"
        );
    }

    #[test]
    fn merged_snapshot_without_header_is_unknown() {
        // 两段都缺 `WINDOW MANAGER WINDOWS` 头 → Unknown（不误判 Hidden）
        let snap =
            "== CHIRI-DUMPSYS-SEGMENT ==\n  mCurrentFocus=Window{abc u0 com.example.game/.Main}\n";
        assert_eq!(
            parse_windows(snap, &request(), 10042),
            WindowVisibility::Unknown
        );
    }

    #[test]
    fn focused_activity_key_survives_empty_first_segment() {
        // 第一段为空（`window` 查询失败已在上游整体失败，但防御性保留）：回退第二段
        let snap = merged("", "  mCurrentFocus=Window{abc u0 com.example.game/.Main}");
        assert_eq!(
            focused_activity_key(&snap).as_ref(),
            "com.example.game/.Main"
        );
    }

    #[test]
    fn activity_key_is_stable_across_dumpsys_spellings() {
        // 用户从 dumpsys 复制的两种写法必须落到同一键，否则规则静默失配
        let a = activity_key("com.tencent.tmgp.sgame", ".SGameActivity");
        let b = activity_key(
            "com.tencent.tmgp.sgame",
            "com.tencent.tmgp.sgame.SGameActivity",
        );
        assert_eq!(a, b);
        assert_eq!(a, "com.tencent.tmgp.sgame/.SGameActivity");
    }

    #[test]
    fn activity_key_keeps_foreign_package_component_intact() {
        // 组件属于别的包（如 systemui 面板盖在游戏上）时不伪造前缀
        assert_eq!(
            activity_key("com.example.game", "com.android.systemui/.Panel"),
            "com.example.game/com.android.systemui/.Panel"
        );
    }

    #[test]
    fn observation_carries_activity_but_identity_match_ignores_it() {
        let now = Instant::now();
        let observation = WindowObservation {
            request: request(),
            visibility: WindowVisibility::Focused,
            observed_at: now,
            activity: Arc::from("com.example.game/.Main"),
        };
        // activity 不参与 request 匹配（身份 = 包名/pid/generation/foreground），
        // 否则每次 Activity 切换都会让窗口观测失效
        assert!(matches_observation(&observation, &request(), now));
        assert_eq!(observation.activity.as_ref(), "com.example.game/.Main");
    }

    #[test]
    fn activity_query_throttle_is_3s_same_package_1s_switched() {
        let now = Instant::now();
        // 首次请求立即允许
        assert_eq!(allowed_query_at(None, true, now), now);
        // 同包稳态：距上次查询 3s（失败重试也走这条，不会退化成 1s）
        assert_eq!(
            allowed_query_at(Some(now), true, now),
            now + ACTIVITY_SAMPLE_INTERVAL
        );
        // 换包：只要求 1s 最小间隔
        assert_eq!(
            allowed_query_at(Some(now), false, now),
            now + ACTIVITY_SAMPLE_MIN_GAP
        );
    }
}
