//! main.rs: [daemonize] [env_init] [soc_check] [config_path] [lang_logger] [channels] [live_time]
//! [scheduler_start] [monitor_start] [suspend]

mod chiri;
mod common;
mod down;
pub mod fas_types;
pub mod i18n;
mod logger;
mod monitor;
mod notify;
mod remote_push;
mod rhine;
mod scheduler;
mod scripts;
pub mod utils;
mod webui_asset;
use crate::i18n::{load_language, t, t_with_args};
use crate::monitor::FasSignal;
use anyhow::Result;
use log::{debug, error, info, warn};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::thread;
// fluent_args 由 i18n.rs 的 #[macro_export] 注入 crate 根，main.rs 即 root 模块，不能再用 use crate::fluent_args 重复导入（E0255）

fn main() -> Result<()> {
    // [toolbox] 内置压缩：`chiri zstd <file>` 就地生成 `<file>.zst`，pack.sh 导出时调用（语义同 gzip -f）
    // zstd 走 vendored C 进程内压缩，级别：启动归档 2／导出 CLI 19，6 线程；必须在 daemonize 之前分发，CLI 需要完整 stdout/stderr 与退出码
    if let Some(cmd) = std::env::args().nth(1) {
        match cmd.as_str() {
            "zstd" => {
                let rest: Vec<String> = std::env::args().skip(2).collect();
                std::process::exit(crate::logger::compress_cli(&cmd, &rest));
            }
            // 其余 arg1 即 chdir 路径，走守护进程流程
            _ => {}
        }
    }

    // [daemonize]
    // 0. 进程自保：脱离派生方会话与管道，看门狗与 daemon 由管理器 su 会话派生，管理器被杀会连带终止调度
    // prctl(PR_SET_PDEATHSIG, 0) 归零父死信号；setsid() 自成会话；stdio 重定向 /dev/null（日志走 daemon.log，panic 由钩子落盘）
    #[cfg(unix)]
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, 0, 0, 0, 0);
        libc::setsid();
        // 经典 daemonize：先关 stdin，open /dev/null 复用 fd 0，再 dup2 覆盖 1/2，与派生方管道彻底解除关联
        libc::close(0);
        let null_fd = libc::open(b"/dev/null\0".as_ptr() as *const libc::c_char, libc::O_RDWR);
        if null_fd >= 0 {
            libc::dup2(null_fd, 0);
            libc::dup2(null_fd, 1);
            libc::dup2(null_fd, 2);
            if null_fd > 2 {
                libc::close(null_fd);
            }
        } else {
            // /dev/null 打不开（fd 耗尽）：关闭 stdout/stderr 解除管道关联，后续写入返回 EBADF，无 SIGPIPE 风险
            libc::close(1);
            libc::close(2);
        }
    }

    // [env_init] 1. 环境初始化
    let chdir_path = std::env::args().nth(1);
    if let Some(path) = &chdir_path {
        nix::unistd::chdir(path.as_str())?;
    }

    let root = common::get_module_root();

    // 单实例锁：flock 互斥必须先于日志归档，否则第二个实例会把首个实例的 logs/ 整体归档改名
    // 拿锁失败即退出（看门狗稍后重试）；锁文件放模块根而非 logs/（logs/ 每次启动被整体归档）
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if let Ok(lock_file) = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(root.join("daemon.lock"))
        {
            if unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                eprintln!("chiri: another daemon instance holds daemon.lock, exiting");
                std::process::exit(0);
            }
            // 故意泄漏 fd 持锁至进程退出，fd 关闭即释放锁
            std::mem::forget(lock_file);
        }
    }

    let log_dir = root.join("logs");
    // 启动归档：上一轮 logs/ 与 devimp/ 重命名后交子线程异步打包为 logd/<ts>.tar.zst 与 logd/devimp_<ts>.tar.zst
    // zstd 级别 2、6 线程，失败回落保留 .tar；完成后各自预算清理（>128MB 删最旧到<96MB）；必须在 logger::init 之前执行
    let (archived_zip, archived_devimp) = logger::archive_on_startup(&root);
    std::fs::create_dir_all(&log_dir)?;
    // devimp/ 不在这里创建（dev_record 关闭时不得产生任何数据），写入路径会自行 create_dir_all；此处只做容量清理兜底
    logger::diag_prepare();

    // [soc_check] 2. 检测到列表中的特定处理器时启用 Chiri 专用调度器
    let chiri_active = common::is_chiri_soc();

    // [config_path] 3. 解析配置路径：Chiri 目标 SoC 优先用 config/{soc}/meta.yaml，其余回退 config/meta.yaml（调优段 feature.yaml 仅存于二进制）
    let config_path = common::get_config_path();
    // [deferred_write_fails] 启动期（logger::init 之前）的对外文件写入失败先攒在这里：那时 log 会被静默丢弃，写失败只留旧值
    // 攒到 logger::init 之后统一补打，与归档结果 / rhine 启动期结果同一套延迟打点口径
    let mut deferred_write_fails: Vec<(&'static str, String)> = Vec::new();
    // 写生效 meta.yaml 相对 config 目录的路径，WebUI 拼接 `config/{相对路径}` 读取同一份文件
    let config_rel = config_path
        .strip_prefix(root.join("config"))
        .unwrap_or(&config_path);
    // as_encoded_bytes 未稳定化，用 to_string_lossy 兼容
    // [log] 三个对外文件写入必须用真实返回 Result 的 utils::write_to_file（try_write_file 恒 Ok），失败入 deferred 延后补打
    if let Err(e) = utils::write_to_file(
        root.join("active_config.chr"),
        config_rel.to_string_lossy().as_bytes(),
    ) {
        deferred_write_fails.push(("active_config.chr", e.to_string()));
    }
    // [pf_chr] 写本机是否启用**相对能效模型**（EEM，取代旧帕累托前沿）：`1` = 启用，`0` = 未启用。
    // 静态资产，启动写一次，只作 WebUI 只读接触点（文件名沿用 `pf.chr` 保持兼容）。
    // 判据与运行期一致：模型可用（三簇至少一簇有曲线）才算启用——只看 soc.yaml 有段
    // 不看数据齐不齐，会写 `1` 而实际走比例路径，WebUI 就显示错了。
    if let Err(e) = utils::write_to_file(
        root.join("pf.chr"),
        if chiri::energy_model::available() {
            b"1"
        } else {
            b"0"
        },
    ) {
        deferred_write_fails.push(("pf.chr", e.to_string()));
    }

    // meta.yaml 快照自愈：以编译期嵌入的 meta.yaml 为基准校验磁盘副本，非法字段只恢复该字段那一行（其余内容保留），缺失则重建
    // `nofix: true` 时跳过所有覆盖类操作，必须在自愈之前读（晚了开关本身先丢）
    let nofix = common::read_nofix_flag(&config_path);
    // 置进程级标志：热重载路径的自愈与资产还原都读它，避免各入口重复读磁盘
    common::set_nofix(nofix);
    if !nofix {
        common::sync_meta_snapshot(&config_path);
    }

    // 清理拆分前的遗留 config.yaml（内容已并入二进制 feature 段与 meta.yaml）
    let _ = std::fs::remove_file(root.join("config").join("config.yaml"));
    if let Ok(rd) = std::fs::read_dir(root.join("config")) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                let _ = std::fs::remove_file(e.path().join("config.yaml"));
            }
        }
    }

    // rules.yaml 快照复制：运行时一律读嵌入值，磁盘副本仅供展示（被篡改不影响调度，下次启动还原），nofix 时交还用户
    if !nofix {
        common::sync_rules_snapshot(&root.join("rules.yaml"));
    }

    // 导出内部特调白名单（编译期嵌入）供 WebUI 展示：每行 `包名:特调模式列表:优先回退模式`，只导出精确包名条目
    // daemon 不读磁盘，仅 Chiri 激活时导出；日志延后到 logger::init 之后输出
    let mut exported_special: Option<usize> = None;
    let mut exported_fas: Option<usize> = None;
    if chiri_active {
        let exact: Vec<&common::SpecialTunedEntry> = common::special_tuned_entries()
            .iter()
            .filter(|e| e.regex.is_none())
            .collect();
        let special_tuned_content: String = exact
            .iter()
            .map(|e| format!("{}:{}:{}\n", e.package, e.modes.join(","), e.fallback))
            .collect();
        // 用真实 Result 判成败：失败入 deferred（init 后补打），exported_special 仅真成功置位
        if let Err(e) = utils::write_to_file(
            root.join("special_tuned.yaml"),
            special_tuned_content.as_bytes(),
        ) {
            deferred_write_fails.push(("special_tuned.yaml", e.to_string()));
        } else {
            exported_special = Some(exact.len());
        }

        // 导出 FAS 白名单（编译期嵌入，WebUI 只读展示，daemon 不读磁盘）
        let fas_content: String = common::fas_whitelist()
            .iter()
            .map(|(pkg, cfg)| format!("{pkg}:{cfg}\n"))
            .collect();
        // 同上改真实成败入口：失败入 deferred，exported_fas 仅真成功置位
        if let Err(e) =
            utils::write_to_file(root.join("fas_whitelist.yaml"), fas_content.as_bytes())
        {
            deferred_write_fails.push(("fas_whitelist.yaml", e.to_string()));
        } else {
            exported_fas = Some(common::fas_whitelist().len());
        }
    }

    // [rhine] 实验室启动期处理：先按 rhine-back.chr 还原上次改动，再按 rhine.chr 决定是否重新套用
    // 必须排在首次 Config::load 之前（还原写的就是 meta.yaml）；只有 Chiri 走这条
    let rhine_report = if chiri_active {
        rhine::on_startup(&root, &config_path)
    } else {
        rhine::StartupReport::default()
    };

    // [lang_logger] 4. 加载语言与日志（meta 读取统一走 chiri::config，非 ChiRi 机型同样适用）
    let (language, loglevel) = {
        let cfg = chiri::config::Config::load(config_path.to_str().unwrap()).unwrap_or_default();
        (cfg.meta.language, cfg.meta.loglevel)
    };
    load_language(&language);
    logger::init(&loglevel)?;

    // [battery_unit] 启动期电流/电压倍数自检：必须排在 meta.yaml 自愈（sync_meta_snapshot）之后，
    // 且首次 Config::load 已把 divisor 同步到遥测层；异步执行，非放电态由 power_supply uevent 唤醒等待
    if let Err(e) = monitor::start_battery_unit_check(config_path.clone()) {
        error!(
            "{}",
            t_with_args(
                "main-battery-unit-spawn-failed",
                &fluent_args!("error" => e.to_string())
            )
        );
    }

    // [webui_restore] 内嵌 WebUI 资产还原：把 webui/dist 覆盖回模块 webroot/，被改/被删的文件回到出厂内容，nofix 时跳过
    if nofix {
        log::info!("{}", t("main-nofix-skip"));
    } else if !webui_asset::EMBEDDED {
        log::warn!("{}", t("main-webui-not-embedded"));
    } else {
        let restored = webui_asset::restore_webroot(&root);
        if restored > 0 {
            log::info!(
                "{}",
                t_with_args(
                    "main-webui-restored",
                    &fluent_args!("count" => restored.to_string())
                )
            );
        }
    }

    // [scripts_restore] 模块脚本启动覆盖 + 运行期篡改监听：WebUI 执行磁盘副本，daemon 走内嵌正文
    // ① 启动无条件覆盖一次；② 常驻 inotify 监听，被改/被删即用内嵌内容还原；nofix 时跳过
    if nofix {
        // 上面已打过 main-nofix-skip，这里不重复
    } else {
        let restored = scripts::restore_module_scripts(&root);
        if restored == 0 {
            log::warn!(
                "[Scripts] 启动覆盖未能写入任何脚本（{} 个）",
                scripts::MODULE_REL.len()
            );
        }
        // 监听线程独立于监控/调度线程，只做防护，故刻意不 join
        let stop = Arc::new(AtomicBool::new(false));
        let watch_root = root.clone();
        std::thread::Builder::new()
            .name("scripts-watch".to_string())
            .spawn(move || {
                scripts::watch_module_scripts(&watch_root, &stop);
            })
            .ok();
    }

    // [remote_push] 远程提醒线程：08~24 点每 2 小时拉一次 remindPush.json，命中 versionCode 就发 `ChiRi-Remote` 通知
    // 独立线程、不 join、失败只留 info 不重试，任何异常都不得影响调度与监控
    remote_push::spawn();

    // [power_avg] 启动清空 PowerAVG.chr：它是本次运行的输出记录，上次的值是过期数据，置空后回到 — 直到首个放电采样写回
    logger::power_avg_reset();

    // 全局 panic 钩子：任何线程 panic 都落盘 daemon.log；钩子内不得调用 i18n（可能死锁或二次 panic），只用纯格式化 + log::error!
    // 消息用英文格式，与 panic payload 一致
    {
        std::panic::set_hook(Box::new(|info| {
            let payload = info.payload();
            let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "<non-string panic payload>".to_string()
            };
            let loc = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_else(|| "<unknown>".to_string());
            let thread_name = std::thread::current()
                .name()
                .unwrap_or("<unnamed>")
                .to_string();
            log::error!("[PANIC] thread \"{thread_name}\" panicked: {msg} (at {loc})");
        }));
    }

    // 日志系统就绪后输出归档结果（归档线程在 init 之前已启动，不阻塞）
    if let Some(zip) = &archived_zip {
        info!(
            "{}",
            t_with_args(
                "main-log-archive-submitted",
                &fluent_args!("zip" => zip.clone())
            )
        );
    }
    if let Some(zip) = &archived_devimp {
        info!(
            "{}",
            t_with_args(
                "main-devimp-archive-submitted",
                &fluent_args!("zip" => zip.clone())
            )
        );
    }
    // 上一轮存活不足 30s（崩溃循环）：其日志已丢弃未打包，这里补一条说明
    if logger::short_session_discarded() {
        info!("{}", t("main-log-short-session-discarded"));
    }
    // 实验室启动期结果：同样发生在 init 之前，这里补打
    rhine::report_startup(&rhine_report);

    // [deferred_write_fails] 启动期对外文件写入失败统一补打（启动一次，非高频）
    for (name, err) in &deferred_write_fails {
        warn!("[Main] 启动期写入 {name} 失败，磁盘保留旧值: {err}");
    }

    // [fix-1017-c3] 首次 Config::load 早于 logger::init，其 error/warn 被静默丢弃，非 ChiRi 机型没有二次 load 重放
    // 这里取走暂存的一条补打（取走即清空）；两个 key 参数名不同（$error / $path），一并塞入由 fluent 自取
    if let Some((level, key, detail)) = chiri::config::drain_startup_issue() {
        let msg = t_with_args(
            key,
            &fluent_args!("error" => detail.clone(), "path" => detail.clone()),
        );
        if level == "error" {
            error!("{msg}");
        } else {
            warn!("{msg}");
        }
    }

    // 白名单导出结果（写入在 logger::init 之前完成，日志延后到此处）
    if let Some(count) = exported_special {
        info!(
            "{}",
            t_with_args(
                "main-special-tuned-exported",
                &fluent_args!("count" => count.to_string())
            )
        );
    }
    if let Some(count) = exported_fas {
        info!(
            "{}",
            t_with_args(
                "main-fas-whitelist-exported",
                &fluent_args!("count" => count.to_string())
            )
        );
    }

    // 日志系统就绪后再输出调试信息
    if let Some(path) = &chdir_path {
        debug!(
            "{}",
            t_with_args("main-chdir", &fluent_args!("dir" => path.as_str()))
        );
    }
    debug!(
        "{}",
        t_with_args(
            "main-module-root",
            &fluent_args!("path" => root.to_string_lossy().to_string())
        )
    );
    debug!(
        "{}",
        t_with_args(
            "main-config-loaded",
            &fluent_args!(
                "path" => config_path.to_string_lossy().to_string(),
                "loglevel" => loglevel,
                "language" => language
            )
        )
    );
    info!("{}", t("chiri-module-starting"));

    // [live_time] 心跳线程：每 15s 把本地时间（MM:SS）写模块根 LiveTime.chr，WebUI 比对差值（容差 20s）判定调度是否在跑
    // 语义是进程级存活：独立线程而非搭调度循环，进程被杀即停写；循环永不 panic、写失败静默
    thread::Builder::new()
        .name("live_time".to_string())
        .spawn(|| {
            loop {
                logger::write_live_time();
                thread::sleep(std::time::Duration::from_secs(
                    logger::LIVE_TIME_INTERVAL_SECS,
                ));
            }
        })?;

    // [channels] 5. 创建通信通道（有界容量 64，满时 send 阻塞形成背压）
    let (tx, rx) = mpsc::sync_channel::<common::DaemonEvent>(64);

    // 特调激活共享标志：TunedGovernor 接管/释放时置位，cpu_monitor 据此在 160ms 与特调 40ms 采样间隔间切换
    let ak_active = Arc::new(AtomicBool::new(false));

    // FAS 前台激活信号：FasManager 激活/去激活时置位并唤醒等待者，fps_monitor 据此门控 eBPF 探针
    // FAS 未激活时不建 tokio runtime、不加载 eBPF、不挂 uprobe；类型为 FasSignal 而非裸 AtomicBool，见 monitor 的 [fas_signal]
    let fas_signal = Arc::new(FasSignal::new(false));

    // [scheduler_start] 6. 启动 ChiRi 调度器（非 ChiRi SoC 不接管 CPU，只跑监控/WebUI/日志，事件通道无消费者直接丢弃）
    let start_result = if chiri_active {
        log::info!("{}", t("main-chiri-scheduler-selected"));
        let cfg = chiri::config::Config::load(config_path.to_str().unwrap()).unwrap_or_default();
        Some(chiri::start_scheduler_thread(
            rx,
            Arc::new(RwLock::new(cfg)),
            ak_active.clone(),
            fas_signal.clone(),
            // 启动期收敛后的实验室模式：监听线程据此判断文件没变就不重复套用
            rhine_report.enabled.clone(),
        ))
    } else {
        log::warn!("{}", t("main-no-chiri-scheduler"));
        // 非 ChiRi SoC 不接管 CPU，与停摆同态：只写 current_mode.chr 投影，不碰 down.chr
        down::project_unsupported(&root);
        log::info!("{}", t("down-unsupported-soc"));
        // devimp 解耦：非 ChiRi 无调度线程，devimp_main / aff_ 产出改由独立诊断线程承担（随 meta.dev_record 独立开关）
        match chiri::start_diag_thread(config_path.clone()) {
            Ok(()) => log::info!("{}", t("main-diag-thread-started")),
            Err(e) => error!(
                "{}",
                t_with_args(
                    "main-diag-thread-failed",
                    &fluent_args!("error" => e.to_string())
                )
            ),
        }
        drop(rx);
        None
    };
    if let Some(Err(e)) = start_result {
        error!(
            "{}",
            t_with_args(
                "scheduler-module-start-failed",
                &fluent_args!("error" => e.to_string())
            )
        );
        return Err(e);
    }
    if chiri_active {
        info!("{}", t("scheduler-module-started"));
    }

    // [monitor_start] 7. 启动 Monitor：Chiri 160ms，非 ChiRi（仅监控）200ms
    let sample_ms_normal: u64 = if chiri_active { 160 } else { 200 };
    let monitor_thread = thread::Builder::new()
        .name("monitor_core".to_string())
        .spawn(move || {
            if let Err(e) = monitor::start_monitor(tx, ak_active, sample_ms_normal, fas_signal) {
                error!(
                    "{}",
                    t_with_args(
                        "monitor-module-crashed",
                        &fluent_args!("error" => e.to_string())
                    )
                );
            }
        })?;

    info!("{}", t("monitor-module-started"));

    // [startup_notify] 启动完成通知：调度线程与监控线程都起来才算本次启动完成；标题/正文/tag 为字面量（`ChiRi已启动` / `ChiRi-Status`）
    // 不走 i18n、不经 FAS 拦截器；只在成功时打 notify-startup-posted（失败已由 notify 组件留痕）
    // [down-notify] DOWN 启动时正文换一条；不能只信 down::is_down()（on_startup 可能尚未置位，存在竞态），故补一次磁盘读取兜底
    let down_at_start = down::is_down() || down::read_down(&root);
    let body = if down_at_start {
        "ChiRi调度已启动，当前以DOWN模式运行，需要其他调度接管"
    } else {
        "ChiRi已启动"
    };
    if notify::post_notification("ChiRi已启动", "ChiRi-Status", body) {
        info!("{}", t("notify-startup-posted"));
    }

    // [suspend] 8. 挂起：join 监控线程
    monitor_thread.join().unwrap();

    Ok(())
}
