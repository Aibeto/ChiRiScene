//! window_visibility.rs: [api] [worker] [query] [parse] [tests]

use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
            shared.0.lock().unwrap().stopped = true;
        }
        Self { shared, worker }
    }

    pub fn request(&self, request: WindowRequest) {
        let mut state = self.shared.0.lock().unwrap();
        if !state.stopped {
            state.enqueue(request);
            self.shared.1.notify_one();
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

fn run_worker(shared: Arc<(Mutex<ProbeState>, Condvar)>) {
    run_worker_with_query(shared, query_visibility);
}

fn run_worker_with_query(
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
    let mut child = Command::new("/system/bin/dumpsys")
        .args(["-t", "1", "window", "windows"])
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
        let deadline = Instant::now() + QUERY_TIMEOUT;
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
fn window_identity(text: &str) -> Option<(&str, &str)> {
    let body = text.strip_prefix("Window{")?.split_once('}')?.0;
    let mut fields = body.split_whitespace();
    let token = fields.next()?;
    let user = fields.next()?.strip_prefix('u')?;
    user.parse::<u32>().ok()?;
    let component = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    let package = component.split_once('/')?.0;
    Some((token, package))
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

fn android_uid(value: &str) -> Option<u32> {
    if let Ok(uid) = value.parse() {
        return Some(uid);
    }
    let body = value.strip_prefix('u')?;
    let (user, application) = body.split_once('a')?;
    user.parse::<u32>()
        .ok()?
        .checked_mul(100000)?
        .checked_add(10000)?
        .checked_add(application.parse().ok()?)
}

fn window_session(block: &str) -> Option<(u32, u32)> {
    let mut sessions = block
        .lines()
        .filter_map(|line| line.trim().strip_prefix("mSession=Session{"));
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

fn parse_windows(snapshot: &str, request: &WindowRequest, uid: u32) -> WindowVisibility {
    if !snapshot
        .lines()
        .any(|line| line.trim() == "WINDOW MANAGER WINDOWS (dumpsys window windows)")
    {
        return WindowVisibility::Unknown;
    }
    let focus = snapshot
        .lines()
        .filter_map(|line| line.trim().strip_prefix("mCurrentFocus="))
        .map(str::trim)
        .collect::<Vec<_>>();
    let focused = if focus.len() == 1 {
        window_identity(focus[0])
    } else {
        None
    };
    let lines = snapshot.lines().collect::<Vec<_>>();
    let mut owned_count = 0;
    let mut unknown_owned = false;
    let mut visible = false;
    let mut obscured = false;
    for (index, line) in lines.iter().enumerate() {
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
