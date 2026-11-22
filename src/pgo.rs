//! pgo.rs: [probe] [sample] [manifest] [prune] [hot]
//! PGO 采样（长期任务）：优先 CoreSight ETM，失败回退 cpu-cycles 采样，再失败则关闭并在 REPROBE_SECS 后重试。
//! 全程事件驱动——开关热重载、屏态变化、采样进程退出、到点这四类判断各有真实信号源，管理器只在这些事件上被唤醒，
//! 代码里没有任何 sleep 或 tick 轮询；息屏期间线程无限期挂起，CPU 占用严格为 0。
//! 产物落 logs/pgo/，随启动归档（logs/ 整体打包）进 logd/<ts>.tar.zst，离线换算用，daemon 自身不做 profile 解析。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

/// 系统自带 simpleperf（Android 平台工具链，root 域可执行）
const SIMPLEPERF: &str = "/system/bin/simpleperf";
/// 采样产物目录（相对模块根）：启动归档会把它整体打包，勿改名
const DIR_REL: &str = "logs/pgo";
/// 产物清单文件名
const MANIFEST: &str = "manifest.txt";
/// 单轮采样时长（秒）：2026-11-22 由 60 提到 120（真机 9 轮实测每轮约 1,100 样本，样本量不足）
const SESSION_SECS: u64 = 120;
/// 启动后首次采样延迟（秒）：错开开机高峰
const FIRST_DELAY_SECS: u64 = 60;
/// 两轮采样之间的空闲间隔（秒）
const IDLE_SECS: u64 = 1800;
/// 三级回退判定为「均不可用」后的重探间隔（秒）
const REPROBE_SECS: u64 = 1800;
/// 启动采样失败后的重试间隔（秒）
const RETRY_SECS: u64 = 300;
/// 探测试录时长（秒）：ETM 是否真的可用只能靠实录判定（sysfs 有节点也可能报 not available）
const PROBE_SECS: u64 = 1;
/// cpu-cycles 回退档的采样频率（Hz）：真机实测 200 Hz × 61 s 只有约 1,100 样本、98% 函数零样本，
/// 2026-11-22 提到 1000（每 CPU 秒 1000 样本，单轮约 1.1 万样本、单文件约 9 MB）
const SAMPLE_FREQ: u32 = 1000;
/// 产物份数上限（提到 1000 Hz 后单文件约 9 MB，份数按字节上限更早触顶）
const MAX_FILES: usize = 24;
/// 产物体积上限（字节）：64 MiB 约 7 轮（≈3.5 小时历史），归档时由 zstd 压到约 1/8
const MAX_FILES_BYTES: u64 = 64 * 1024 * 1024;
/// 产物最小有效体积（字节）：低于此值视为空采样，直接丢弃
const MIN_SAMPLES_BYTES: u64 = 1024;
/// 连续启动失败次数上限，超过后重新探测档位
const MAX_START_FAILS: u32 = 3;

/// 档位：尚未探测
const TIER_UNKNOWN: u8 = 0;
/// 档位：CoreSight ETM（AutoFDO 可用的分支/路径数据）
const TIER_ETM: u8 = 1;
/// 档位：cpu-cycles 采样（无 ETM 时的回退，只有函数热度）
const TIER_SAMPLE: u8 = 2;
/// 档位：两条路都不可用（关闭采样，REPROBE_SECS 后重探）
const TIER_OFF: u8 = 3;

/// 采样总开关（meta.yaml 的 pgo_sampling，缺省 true）：Config::load（启动 + 热重载）同步到这里
static ENABLED: AtomicBool = AtomicBool::new(true);
/// 事件闸门：Condvar::new 是 const，可直接声明为 static（同 monitor::POWER_SUPPLY_CVAR）
static WAKE: Condvar = Condvar::new();
/// 事件代次：notify 时在锁内自增，等待方比对代次判是否被唤醒（防丢唤醒，也防虚假唤醒被当成事件）
static WAKE_GEN: Mutex<u64> = Mutex::new(0);

/// 取事件代次锁；中毒（持锁线程 panic）时拿回 guard，绝不让一次 panic 把采样器永久哑掉
fn lock_gen() -> MutexGuard<'static, u64> {
    match WAKE_GEN.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 唤醒采样管理器：配置开关变化、屏态变化、采样进程退出三处调用；幂等，多调几次只是多一次空转
pub fn notify() {
    let mut g = lock_gen();
    *g = (*g).wrapping_add(1);
    WAKE.notify_all();
}

/// 同步采样开关（Config::load 的启动与热重载两条路径）：值未变不发事件，避免每次写配置都空转唤醒
pub fn set_enabled(on: bool) {
    if ENABLED.swap(on, Ordering::AcqRel) != on {
        notify();
    }
}

/// 事件代次快照：**必须在本轮所有状态判定之前取**——先快照再读屏态/开关/会话状态，
/// 才不会出现「判定用旧值 → 事件已发生 → 睡下去错过唤醒」的丢唤醒竞态（事件侧先改状态后递增代次）
fn snapshot() -> u64 {
    *lock_gen()
}

/// 阻塞到「代次前进」或「到点」为止；seen 是本轮判定前取的快照，返回即代表需要重新判态。
/// deadline = None 表示没有定时器，纯等事件；代次已前进时立刻返回（不吞事件）
fn wait_until(deadline: Option<Instant>, seen: u64) {
    let mut g = lock_gen();
    loop {
        if *g != seen {
            return;
        }
        g = match deadline {
            None => match WAKE.wait(g) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            },
            Some(dl) => {
                let now = Instant::now();
                if now >= dl {
                    return;
                }
                match WAKE.wait_timeout(g, dl - now) {
                    Ok((guard, _)) => guard,
                    Err(poisoned) => poisoned.into_inner().0,
                }
            }
        };
    }
}

/// 一轮采样会话：子进程由 reaper 线程持有并 wait（阻塞在内核 wait4，不是轮询）
struct Session {
    /// 子进程 pid（只用于发信号，回收由 reaper 完成）
    pid: u32,
    /// perf.data 路径
    path: PathBuf,
    /// 本轮档位
    tier: u8,
    /// 起采时刻（写 manifest 的 dur 用它实测值）
    started: Instant,
    /// reaper 的完成信号
    done: Receiver<()>,
}

impl Session {
    /// 是否已收尾（reaper 已 wait 返回；发送端被丢弃也按已收尾处理，避免卡死）
    fn finished(&self) -> bool {
        !matches!(self.done.try_recv(), Err(TryRecvError::Empty))
    }

    /// 中止本轮采样：先 SIGINT 让 simpleperf 收尾，超时强杀；产物由调用方丢弃
    fn stop(&self) {
        if self.finished() {
            return;
        }
        // SAFETY: kill 只读 pid 与信号值，失败返回 -1（进程已退出时为 ESRCH），不产生未定义行为
        unsafe {
            libc::kill(self.pid as libc::pid_t, libc::SIGINT);
        }
        if self.done.recv_timeout(Duration::from_secs(3)).is_err() {
            // SAFETY: 同上
            unsafe {
                libc::kill(self.pid as libc::pid_t, libc::SIGKILL);
            }
            let _ = self.done.recv_timeout(Duration::from_secs(1));
        }
    }
}

/// 起采样管理器线程（main 启动尾段调用，不 join）
pub fn spawn(root: PathBuf) {
    let dir = root.join(DIR_REL);
    let handle = std::thread::Builder::new()
        .name("pgo".to_string())
        .spawn(move || run(dir));
    if let Err(e) = handle {
        // 采样是纯收益能力，线程起不来只影响 PGO 数据，绝不阻断调度
        log::warn!(
            "{}",
            t_with_args("pgo-thread-failed", &fluent_args!("error" => e.to_string()))
        );
    }
}

/// 管理器主循环：只判态、起会话、等事件，没有任何 sleep
fn run(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let mut tier = TIER_UNKNOWN;
    let mut next_at = Some(Instant::now() + Duration::from_secs(FIRST_DELAY_SECS));
    let mut session: Option<Session> = None;
    let mut fails: u32 = 0;

    loop {
        // 先取代次快照、再判态：事件侧是「先改状态、后递增代次」，反序取会漏掉这期间发生的事件
        let seen = snapshot();

        // [gate_off] 关闭态：收掉在跑的会话，然后无限期等事件（没有定时器，重启开关立刻响应）
        if !ENABLED.load(Ordering::Acquire) {
            if let Some(s) = session.take() {
                s.stop();
                discard(&s);
                log::info!("{}", t("pgo-disabled"));
            }
            wait_until(None, seen);
            continue;
        }

        // [session] 会话进行中：等 reaper 事件，不查子进程状态
        if let Some(s) = session.as_ref() {
            if s.finished() {
                let s = session.take().expect("session just checked");
                settle(&dir, &s);
                next_at = Some(Instant::now() + Duration::from_secs(IDLE_SECS));
            } else {
                wait_until(None, seen);
            }
            continue;
        }

        // [deadline] 到点闸门：首次延迟 / 空闲间隔 / 重探间隔都收敛到这里
        if let Some(at) = next_at {
            if Instant::now() < at {
                wait_until(Some(at), seen);
                continue;
            }
        }

        // [screen] 息屏闸门：纯事件等待（亮屏时 screen 事件唤醒），息屏期间零 CPU
        if !crate::common::screen_on() {
            wait_until(None, seen);
            continue;
        }

        // [probe] 未探测或上次判定不可用：重探一次（ETM 优先，cpu-cycles 回退）
        if tier == TIER_UNKNOWN || tier == TIER_OFF {
            tier = probe(&dir);
            if tier == TIER_OFF {
                next_at = Some(Instant::now() + Duration::from_secs(REPROBE_SECS));
                continue;
            }
        }

        // [start] 起一轮采样；连续失败到阈值就重新探测档位
        match start(&dir, tier) {
            Some(s) => {
                session = Some(s);
                fails = 0;
                next_at = None;
            }
            None => {
                fails = fails.saturating_add(1);
                if fails >= MAX_START_FAILS {
                    tier = TIER_UNKNOWN;
                }
                next_at = Some(Instant::now() + Duration::from_secs(RETRY_SECS));
            }
        }
    }
}

/// 探测采样档位：ETM 优先，失败退 cpu-cycles，两者皆不可用返回 TIER_OFF
fn probe(dir: &Path) -> u8 {
    if !Path::new(SIMPLEPERF).exists() {
        log::info!("{}", t("pgo-tool-missing"));
        return TIER_OFF;
    }
    if trial(dir, "cs-etm", false) {
        log::info!("{}", t("pgo-probe-etm"));
        return TIER_ETM;
    }
    if trial(dir, "cpu-cycles", true) {
        log::info!("{}", t("pgo-probe-sample"));
        return TIER_SAMPLE;
    }
    log::info!("{}", t("pgo-off"));
    TIER_OFF
}

/// 试录一秒判定该事件是否真的可用：进程退出码 + 产物非空双条件，产物用完即删
fn trial(dir: &Path, event: &str, sampling: bool) -> bool {
    let tmp = dir.join(format!(
        ".probe_{}_{}.data",
        event.replace('-', "_"),
        std::process::id()
    ));
    let mut cmd = Command::new(SIMPLEPERF);
    cmd.arg("record")
        .arg("-e")
        .arg(event)
        .arg("-p")
        .arg(std::process::id().to_string())
        .arg("--duration")
        .arg(PROBE_SECS.to_string());
    if sampling {
        cmd.arg("-f").arg(SAMPLE_FREQ.to_string());
    }
    cmd.arg("-o")
        .arg(&tmp)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let ok = match cmd.status() {
        Ok(status) => {
            status.success()
                && std::fs::metadata(&tmp)
                    .map(|m| m.len() > 0)
                    .unwrap_or(false)
        }
        Err(e) => {
            log::debug!(
                "{}",
                t_with_args(
                    "pgo-probe-failed",
                    &fluent_args!("event" => event.to_string(), "error" => e.to_string())
                )
            );
            false
        }
    };
    let _ = std::fs::remove_file(&tmp);
    ok
}

/// 起一轮采样：命令参数与探测档一致，子进程交给 reaper 线程回收
fn start(dir: &Path, tier: u8) -> Option<Session> {
    let event = if tier == TIER_ETM {
        "cs-etm"
    } else {
        "cpu-cycles"
    };
    let path = dir.join(format!("perf_{}.data", epoch_ms()));
    let mut cmd = Command::new(SIMPLEPERF);
    cmd.arg("record")
        .arg("-e")
        .arg(event)
        .arg("-p")
        .arg(std::process::id().to_string())
        .arg("--duration")
        .arg(SESSION_SECS.to_string());
    if tier != TIER_ETM {
        cmd.arg("-f").arg(SAMPLE_FREQ.to_string());
    }
    cmd.arg("-o")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // 看门狗重启 daemon 时子进程必须一起死：否则采样进程孤儿化，还攥着旧会话的 perf.data
    // SAFETY: pre_exec 只调 prctl（async-signal-safe），不分配、不加锁
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            let rc = libc::prctl(
                libc::PR_SET_PDEATHSIG,
                libc::SIGKILL as libc::c_ulong,
                0,
                0,
                0,
            );
            if rc != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log::warn!(
                "{}",
                t_with_args(
                    "pgo-start-failed",
                    &fluent_args!("tier" => tier.to_string(), "error" => e.to_string())
                )
            );
            let _ = std::fs::remove_file(&path);
            return None;
        }
    };
    let pid = child.id();
    let started = Instant::now();
    let (tx, done) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("pgo-reaper".to_string())
        .spawn(move || {
            let mut child = child;
            let _ = child.wait();
            let _ = tx.send(());
            notify();
        });
    if let Err(e) = spawned {
        // reaper 起不来就无法回收子进程：强杀并丢弃产物，本轮按启动失败处理
        // SAFETY: 同 Session::stop
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
        let _ = std::fs::remove_file(&path);
        log::warn!(
            "{}",
            t_with_args("pgo-reaper-failed", &fluent_args!("error" => e.to_string()))
        );
        return None;
    }
    // pid 必须是被采样进程（daemon 自己）：真机日志此前打的是 simpleperf 子进程 pid，
    // 与 perf.data cmdline 里的 -p 对不上，离线判读要先反推一次
    log::info!(
        "{}",
        t_with_args(
            "pgo-session-start",
            &fluent_args!(
                "tier" => tier.to_string(),
                "pid" => std::process::id().to_string(),
                "sampler" => pid.to_string()
            )
        )
    );
    Some(Session {
        pid,
        path,
        tier,
        started,
        done,
    })
}

/// 收尾一轮采样：空产物丢弃，有效产物写 manifest 并做预算清理
fn settle(dir: &Path, s: &Session) {
    let size = std::fs::metadata(&s.path).map(|m| m.len()).unwrap_or(0);
    let name = s
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if size < MIN_SAMPLES_BYTES {
        let _ = std::fs::remove_file(&s.path);
        log::debug!(
            "{}",
            t_with_args(
                "pgo-session-empty",
                &fluent_args!("file" => name, "size" => size.to_string())
            )
        );
        return;
    }
    append_manifest(dir, &name, s, size);
    log::info!(
        "{}",
        t_with_args(
            "pgo-session-done",
            &fluent_args!("file" => name, "size" => size.to_string())
        )
    );
    prune(dir);
}

/// 丢弃产物（配置关闭导致的强制中止：半截 perf.data 会误导离线换算）
fn discard(s: &Session) {
    let _ = std::fs::remove_file(&s.path);
}

/// 追加一行 manifest：离线换算要看的是「哪轮、什么档、多大、多久」，soc/kernel/exe 用于跨版本比对时确认同源
fn append_manifest(dir: &Path, name: &str, s: &Session, size: u64) {
    use std::io::Write;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let soc = crate::common::matched_soc_config_name().unwrap_or("unknown");
    // 必须用 metadata 跟随符号链接：symlink_metadata 取到的是 /proc 的链接本身（size 恒 0、mtime 随读取变化），
    // 真机实测会让同一进程内的多轮 manifest 写出不同的 exe 指纹
    let exe = std::fs::metadata("/proc/self/exe")
        .ok()
        .map(|m| {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("{}:{}", m.len(), mtime)
        })
        .unwrap_or_else(|| "unknown".to_string());
    let line = format!(
        "ts={} file={} tier={} size={} dur={} freq={} soc={} kernel={} exe={}\n",
        ts,
        name,
        s.tier,
        size,
        s.started.elapsed().as_secs(),
        SAMPLE_FREQ,
        soc,
        kernel,
        exe
    );
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(MANIFEST))
    {
        Ok(mut f) => {
            if let Err(e) = f.write_all(line.as_bytes()) {
                log::warn!(
                    "{}",
                    t_with_args(
                        "pgo-manifest-failed",
                        &fluent_args!("error" => e.to_string())
                    )
                );
            }
        }
        Err(e) => {
            log::warn!(
                "{}",
                t_with_args(
                    "pgo-manifest-failed",
                    &fluent_args!("error" => e.to_string())
                )
            );
        }
    }
}

/// 预算清理的纯逻辑：输入按 mtime 新→旧排列的各产物体积，返回哪些保留（前 MAX_FILES 份且累计不超过 MAX_FILES_BYTES）
fn prune_keep(sizes: &[u64]) -> Vec<bool> {
    let mut keep = vec![false; sizes.len()];
    let mut total = 0u64;
    for (i, len) in sizes.iter().enumerate() {
        total = total.saturating_add(*len);
        if i < MAX_FILES && total <= MAX_FILES_BYTES {
            keep[i] = true;
        }
    }
    keep
}

/// 清理 logs/pgo/：只认 perf_*.data，按 mtime 新→旧保留限量产物，其余删除
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut found: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("perf_") || !name.ends_with(".data") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
        found.push((mtime, meta.len(), entry.path()));
    }
    // 新→旧：同 mtime 时按路径兜底，保证顺序稳定
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.2.cmp(&a.2)));
    let sizes: Vec<u64> = found.iter().map(|(_, len, _)| *len).collect();
    let keep = prune_keep(&sizes);
    for (i, (_, _, path)) in found.iter().enumerate() {
        if !keep[i] {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 毫秒级 epoch：产物名单文件名用，避免同秒两轮撞名
fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{MAX_FILES, MAX_FILES_BYTES, prune_keep};

    /// 份数上限：超过 MAX_FILES 的部分一律不保留
    #[test]
    fn prune_keep_caps_file_count() {
        let sizes = vec![1024u64; MAX_FILES + 4];
        let keep = prune_keep(&sizes);
        assert_eq!(keep.iter().filter(|k| **k).count(), MAX_FILES);
        assert!(!keep[MAX_FILES]);
    }

    /// 体积上限：新→旧累计超限，跨线的那份及更旧的都不保留
    #[test]
    fn prune_keep_caps_total_bytes() {
        let each = 2 * 1024 * 1024u64;
        let count = 20usize;
        let keep = prune_keep(&vec![each; count]);
        let expected = (MAX_FILES_BYTES / each) as usize;
        assert_eq!(keep.iter().filter(|k| **k).count(), expected);
        assert!(keep[expected - 1]);
        assert!(!keep[expected]);
    }

    /// 空目录与单份小产物：不误删
    #[test]
    fn prune_keep_keeps_small_sets() {
        assert!(prune_keep(&[]).is_empty());
        assert_eq!(prune_keep(&[10]), vec![true]);
    }
}
