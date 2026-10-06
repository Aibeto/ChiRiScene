//! fas_process.rs: [read] [identity] [alive]
//! 进程身份取证：starttime 快照 + 可选 pidfd。用于 FAS owner 的 PID 复用判定与存活轮询。

use std::fs;

// pidfd 仅在真机（Linux/Android 且非 test profile）编译：test/runtime 不依赖 libc，也不引入自定义 cfg
#[cfg(all(not(test), any(target_os = "linux", target_os = "android")))]
type PidFd = std::os::fd::OwnedFd;
#[cfg(not(all(not(test), any(target_os = "linux", target_os = "android"))))]
type PidFd = ();

/// 读取进程 starttime（`/proc/<pid>/stat` 第 22 字段，pid 后第 20 项）；Z/X 僵尸或读取失败返回 None。
pub fn read_process_starttime(pid: i32) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = stat.rsplit_once(')')?.1;
    let mut fields = fields.split_whitespace();
    let process_state = fields.next()?;
    if matches!(process_state, "Z" | "X" | "x") {
        return None;
    }
    fields.nth(18)?.parse().ok()
}

// [identity]
/// 进程身份快照。pidfd 可用时 `is_alive` 为零开销 poll；否则回退 starttime 比对（仅有基准时）。
pub struct ProcessIdentity {
    starttime: Option<u64>,
    #[cfg(all(not(test), any(target_os = "linux", target_os = "android")))]
    pidfd: Option<PidFd>,
}

impl ProcessIdentity {
    /// attach：先读 starttime、再开 pidfd、再读一次交叉校验；前后不一致（attach 窗口内被替换）则放弃身份基准。
    pub fn attach(pid: i32) -> Self {
        let before = read_process_starttime(pid);
        #[cfg(all(not(test), any(target_os = "linux", target_os = "android")))]
        let pidfd = open_pidfd(pid);
        let after = read_process_starttime(pid);
        let starttime = if before == after { before } else { None };
        Self {
            starttime,
            #[cfg(all(not(test), any(target_os = "linux", target_os = "android")))]
            pidfd,
        }
    }

    /// 与给定 starttime 快照是否同一进程。任一侧未知即判不同（fail-closed：无法证明同一进程时不认会话）。
    pub fn matches_starttime(&self, starttime: Option<u64>) -> bool {
        match (self.starttime, starttime) {
            (Some(stored), Some(provided)) => stored == provided,
            _ => false,
        }
    }

    /// 存活判定：优先 pidfd 零 poll；pidfd 不可用时按 starttime 比对。基准未知判死（fail-closed，交由主线退出）。
    pub fn is_alive(&self, pid: i32) -> bool {
        if self.starttime.is_none() {
            return false;
        }
        #[cfg(all(not(test), any(target_os = "linux", target_os = "android")))]
        if let Some(pidfd) = self.pidfd.as_ref() {
            return pidfd_alive(pidfd);
        }
        match self.starttime {
            Some(stored) => read_process_starttime(pid) == Some(stored),
            None => false,
        }
    }

    /// 廉价存活提示：有 pidfd 时返回 Some(是否存活)（零 poll，可每帧调用）；仅回退路径返回 None——
    /// 每帧读 /proc 代价高，改由主线 `owner_is_alive` 低频 reconcile。
    pub fn cheap_alive(&self) -> Option<bool> {
        if self.starttime.is_none() {
            return Some(false);
        }
        #[cfg(all(not(test), any(target_os = "linux", target_os = "android")))]
        if let Some(pidfd) = self.pidfd.as_ref() {
            return Some(pidfd_alive(pidfd));
        }
        None
    }

    #[cfg(test)]
    pub fn for_test(starttime: Option<u64>) -> Self {
        Self {
            starttime,
            #[cfg(all(not(test), any(target_os = "linux", target_os = "android")))]
            pidfd: None,
        }
    }
}

// [alive]
/// pidfd 零超时 poll：仅「返回 0（无事件）」算存活；就绪、POLLIN/POLLERR/POLLHUP/POLLNVAL、
/// 或 poll 出错一律判死（fail-closed，绝不把异常当存活）。
#[cfg(all(not(test), any(target_os = "linux", target_os = "android")))]
fn pidfd_alive(pidfd: &PidFd) -> bool {
    use std::os::fd::AsRawFd;
    let mut poll_fd = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut poll_fd, 1, 0) };
    if ready < 0 {
        return false;
    }
    if ready == 0 {
        return true;
    }
    let fatal = libc::POLLIN | libc::POLLERR | libc::POLLHUP | libc::POLLNVAL;
    poll_fd.revents & fatal == 0
}

/// pidfd_open：用 syscall 直调，避免依赖各平台 libc 的包装可用性；不可用时返回 None 走 starttime 回退。
#[cfg(all(not(test), any(target_os = "linux", target_os = "android")))]
fn open_pidfd(pid: i32) -> Option<PidFd> {
    use std::os::fd::FromRawFd;
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::c_long, 0) };
    if fd < 0 {
        return None;
    }
    Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_starttimes_compare_exactly() {
        let identity = ProcessIdentity::for_test(Some(4321));
        assert!(identity.matches_starttime(Some(4321)));
        assert!(!identity.matches_starttime(Some(4322)));
    }

    #[test]
    fn unknown_starttime_never_matches() {
        // 存储侧未知
        assert!(!ProcessIdentity::for_test(None).matches_starttime(Some(4321)));
        assert!(!ProcessIdentity::for_test(None).matches_starttime(None));
        // 提供侧未知
        assert!(!ProcessIdentity::for_test(Some(4321)).matches_starttime(None));
    }

    #[test]
    fn missing_baseline_is_not_alive() {
        // 无 pidfd、无 starttime 基准：判死（fail-closed）
        assert!(!ProcessIdentity::for_test(None).is_alive(4321));
        assert!(!ProcessIdentity::for_test(None).cheap_alive().unwrap_or(false));
    }
}
