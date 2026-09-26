//! mod.rs: [mods] [guard] [fas_signal] [start] [init] [watchers] [ebpf] [detect]

use log::error;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

// [mods]
pub mod app_detect;
pub mod config;
pub mod cpu_monitor;
pub mod fps_monitor;
pub mod screen_detect;
pub mod telemetry;

use crate::common::DaemonEvent;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// [guard]
/// 受护子线程入口：监控子线程 panic 时线程级死亡看门狗感知不到（它只监控进程存活），该监控
/// 会永久缺失（如 fps 死后 FAS 无帧）。此处落盘后 `exit(1)`，由看门狗 3s 后按进程级拉起全新
/// daemon（main 启动初始化会重建全部监控）。仅 `Err`（panic）触发退出，正常返回行为不变；
/// 消息刻意不走 i18n：运行在 panic 收尾路径，纯英文便于检索且避免碰 i18n 锁。
fn spawn_guarded<F: FnOnce() + Send + 'static>(name: &'static str, f: F) -> std::io::Result<()> {
    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
                log::error!("[PANIC] monitor thread \"{name}\" died, exiting for watchdog restart");
                std::process::exit(1);
            }
        })?;
    Ok(())
}

/// CPU hotplug 事件标记：netlink uevent 收到 `cpu` 子系统事件（cpuN/online 变更）时置位，
/// affinity 消费方见到即刷新在线核位图（不必等 8s 周期）；周期兜底保留，不广播 uevent 的机型行为不变。
pub static CPU_HOTPLUG_DIRTY: AtomicBool = AtomicBool::new(false);

// [fas_signal]
/// FAS 前台激活信号：**FAS 激活瞬间本身可等待**。生产方是 chiri 的 `FasManager`（activate/
/// 续期 `set(true)`、`deactivate_active` `set(false)`），消费方是 fps 探针待机线程。
/// 等待对象必须是「激活」而非前台 PID：FAS 会在 PID 未变时重新激活（息屏释放回前台、15s
/// 冷却期结束调度线程自行 activate），等 PID 的线程会漏掉这类激活、FAS 收不到帧。
/// **store 与 notify 必须同锁**：读标志与入睡拆开必有丢唤醒窗口（读到 false → 置位方 notify
/// 时队列为空通知丢失 → 等待方入睡后漏唤醒）；同锁后仅剩两种不丢唤醒的交错，勿拆。
pub struct FasSignal {
    /// 状态本体。热路径只读原子量，与改造前的裸 `AtomicBool` 同价。
    flag: AtomicBool,
    /// 保护 `flag` 的谓词判定（wait_until_active）与置位+通知（set）的互斥锁；无哨兵值仅互斥用，勿删（见类型注释丢唤醒说明）。
    lock: Mutex<()>,
    /// 配合 `lock` 唤醒待机线程；`set` 与等待谓词同锁，故不存在丢唤醒。
    cvar: Condvar,
}

impl FasSignal {
    /// `active` 为初值（daemon 启动时 false = FAS 未激活）。
    pub const fn new(active: bool) -> Self {
        Self {
            flag: AtomicBool::new(active),
            lock: Mutex::new(()),
            cvar: Condvar::new(),
        }
    }

    /// 只读当前状态：等价裸原子量 load(Acquire)，供热路径零成本判定（fps 探针门控、待机线程唤醒后二次判定）。
    pub fn is_active(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    /// 置位/清零并唤醒全部等待者：store 与 notify_all 必须同锁（拆开即丢唤醒，见类型注释）；值未变也照常通知——幂等且无副作用。
    pub fn set(&self, active: bool) {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.flag.store(active, Ordering::Release);
        self.cvar.notify_all();
    }

    /// 阻塞直到 `is_active()` 为真（事件驱动待机，**无周期超时兜底**）；可能虚假唤醒或被
    /// set(false) 唤醒，故循环重判谓词。调用方语义：等待前完成待机收尾（摘除 uprobe），唤醒后自行补挂当前前台 PID。
    pub fn wait_until_active(&self) {
        let mut guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        while !self.flag.load(Ordering::Acquire) {
            guard = self.cvar.wait(guard).unwrap_or_else(|e| e.into_inner());
        }
    }
}

// [start]
/// `ak_active`：特调（akmode）激活标志，cpu_monitor 据此在常规与 40ms 采样间切换。
/// `sample_ms_normal`：常规采样间隔（main.rs 按 SoC 传入：ChiRi 160ms / 非 ChiRi 200ms）。
/// `fas_signal`：FAS 前台激活信号（见 `[fas_signal]`），fps_monitor 据此门控 eBPF 探针加载与 uprobe 挂载。
pub fn start_monitor(
    tx: SyncSender<DaemonEvent>,
    ak_active: Arc<AtomicBool>,
    sample_ms_normal: u64,
    fas_signal: Arc<FasSignal>,
) -> Result<(), Box<dyn Error>> {
    log::debug!("{}", t("monitor-starting"));

    // [init]
    // 解除内核 eBPF Map 内存锁定限制
    unsafe {
        let rlim = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: libc::RLIM_INFINITY,
        };
        if libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) != 0 {
            log::warn!("{}", t("monitor-rlimit-memlock-failed"));
        }
    }

    // --- 初始化配置 ---
    // 嵌入 rules.yaml 为唯一规则来源（编译期打包防篡改）；磁盘文件仅是启动时复制的展示副本（main.rs::sync_rules_snapshot），不参与运行时读取
    let initial_config = crate::common::embedded_rules();

    let config_arc = Arc::new(Mutex::new(initial_config));
    let config_arc_clone_for_watcher = Arc::clone(&config_arc);

    // --- 初始化共享的屏幕状态 ---
    let screen_state_arc = Arc::new(Mutex::new(true));
    let screen_state_clone_for_watcher = Arc::clone(&screen_state_arc);
    let screen_state_clone_for_app_detect = Arc::clone(&screen_state_arc);

    // 初始化共享的强制刷新标志
    let force_refresh_arc = Arc::new(AtomicBool::new(false));
    let force_refresh_clone_for_watcher = Arc::clone(&force_refresh_arc);

    // [watchers]
    // 屏幕状态监控线程：debug.tracing.screen_state 属性轮询（见 screen_detect.rs [prop]），
    // 变化即直推 ScreenStateChange 事件；app_detect 的轮询转发退化为 verify 自愈兜底（双发由消费端去重）。
    // [PAUSED] 原 uevent sysfs 投票直推已暂停（screen_detect.rs [source]）。
    log::debug!("{}", t("monitor-thread-start-screen"));
    let tx_screen = tx.clone();
    spawn_guarded("screen_watcher", move || {
        screen_detect::monitor_screen_state_property(screen_state_clone_for_watcher, tx_screen);
    })?;

    // 3b. uevent 线程：现仅服务 CPU hotplug（置 CPU_HOTPLUG_DIRTY，2s 周期块刷新核位图）；
    //     屏幕分支已 [PAUSED]，见 screen_detect.rs [uevent]。
    let screen_state_clone_for_uevent = Arc::clone(&screen_state_arc);
    let tx_uevent = tx.clone();
    spawn_guarded("uevent_watcher", move || {
        if let Err(e) =
            screen_detect::monitor_screen_state_uevent(screen_state_clone_for_uevent, tx_uevent)
        {
            error!(
                "{}",
                t_with_args(
                    "monitor-screen-watcher-failed",
                    &fluent_args!("error" => e.to_string())
                )
            );
        }
    })?;

    // 4. 启动配置监控线程
    log::debug!("{}", t("monitor-thread-start-config-watch"));
    let tx_config = tx.clone();
    spawn_guarded("config_watcher", move || {
        if let Err(e) = app_detect::watch_config_file(
            config_arc_clone_for_watcher,
            force_refresh_clone_for_watcher,
            tx_config,
        ) {
            error!(
                "{}",
                t_with_args(
                    "monitor-config-watcher-failed",
                    &fluent_args!("error" => e.to_string())
                )
            );
        }
    })?;

    // 前台 PID 统一广播源：FPS/CPU 两个 eBPF 监控共享同一 watch 通道，替代各自 500ms 的重复轮询。
    // 推送源在 app_detect（`CURRENT_PID` 唯一写入点 set_current_package，变化即发）；原 pid_watcher
    // 线程（500ms 空转转发）已删除。fps_monitor 在下方启动块 clone Receiver 消费同一广播。
    let initial_pid = app_detect::get_current_pid();
    let (tx_pid, rx_pid_cpu) = tokio::sync::watch::channel(initial_pid as u32);

    // [ebpf]
    // eBPF FPS 监控线程（独立 Tokio runtime）：仅服务 FAS 调频，仅 ChiRi 且 FAS 可用时启动
    // （空 uprobe attach + RingBuf 轮询是纯开销，非 ChiRi 零开销）。PID 复用上方共享广播。
    // 反偷跑门控：FasManager 激活前线程以**信号等待**待机（见 `[fas_signal]`）——tokio runtime /
    // eBPF 加载 / uprobe 挂载推迟到首个 FAS 应用进前台；稳态阻塞在条件变量 0 次周期唤醒（改造前 1s 轮询）。
    if crate::common::is_chiri_soc() && crate::common::fas_available() {
        log::debug!("{}", t("monitor-thread-start-fps"));
        let tx_fps = tx.clone();
        let rx_pid_fps = rx_pid_cpu.clone();
        spawn_guarded("fps_monitor_ebpf", move || {
            // 激活前待机：等待 FAS 激活信号（零周期唤醒），不建 runtime 不加载 eBPF
            fas_signal.wait_until_active();
            if let Ok(rt) = tokio::runtime::Runtime::new() {
                rt.block_on(async {
                    if let Err(e) =
                        fps_monitor::start_fps_loop(tx_fps, rx_pid_fps, fas_signal).await
                    {
                        error!(
                            "{}",
                            t_with_args(
                                "monitor-fps-crashed",
                                &fluent_args!("error" => e.to_string())
                            )
                        );
                    }
                });
            } else {
                error!("{}", t("monitor-fps-tokio-failed"));
            }
        })?;
    }

    // 7. 启动 eBPF CPU 负载监控线程
    log::debug!("{}", t("monitor-thread-start-cpu"));
    let tx_cpu = tx.clone();
    let ak_active_cpu = ak_active.clone();
    spawn_guarded("cpu_monitor_ebpf", move || {
        if let Ok(rt) = tokio::runtime::Runtime::new() {
            rt.block_on(async {
                if let Err(e) =
                    cpu_monitor::start_cpu_loop(tx_cpu, rx_pid_cpu, ak_active_cpu, sample_ms_normal)
                        .await
                {
                    error!(
                        "{}",
                        t_with_args(
                            "monitor-cpu-crashed",
                            &fluent_args!("error" => e.to_string())
                        )
                    );
                }
            });
        } else {
            error!("{}", t("monitor-cpu-tokio-failed"));
        }
    })?;

    // ChiRi 专属遥测线程：1s 轮询 PSI / GPU busy% / 电池电流电压，写进程级共享原子量
    // （telemetry()）供 chiri 调度层消费与落盘；仅 ChiRi SoC 启动，非 ChiRi 零开销。
    if crate::common::is_chiri_soc() {
        log::debug!("{}", t("monitor-thread-start-telemetry"));
        spawn_guarded("telemetry_monitor", telemetry::telemetry_loop)?;
    }

    // [detect]
    // 8. 启动应用检测主循环 (阻塞)
    log::debug!("{}", t("monitor-thread-start-app-detect"));
    app_detect::app_detection_loop(
        config_arc,
        screen_state_clone_for_app_detect,
        force_refresh_arc,
        tx,
        tx_pid,
    )?;

    Ok(())
}
