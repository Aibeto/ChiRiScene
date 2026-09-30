//! logger.rs: [level] [appender] [loglimit] [init] [status] [devimp_state] [devrow] [devimp_writer] [devimp_api]
//! [aff_writer] [aff_api] [archive]

use crate::common;
use crate::fluent_args;
use crate::i18n::t_with_args;
use anyhow::{Result, anyhow};
use log::{LevelFilter, Log, Metadata, Record};
use std::collections::HashMap;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

// [level]
fn parse_level(level_str: &str) -> LevelFilter {
    match level_str.to_uppercase().as_str() {
        "OFF" => LevelFilter::Off,
        "ERROR" => LevelFilter::Error,
        "WARN" => LevelFilter::Warn,
        "INFO" => LevelFilter::Info,
        "DEBUG" => LevelFilter::Debug,
        "TRACE" => LevelFilter::Trace,
        _ => LevelFilter::Info,
    }
}

/// 本模块 `daemon.log` 的相对路径（`logs/daemon.log`）
const LOG_REL_PATH: &str = "logs/daemon.log";
/// 单文件大小上限：达到后把 `daemon.log` 循环后移成 `daemon.{1,2,...}.log`
const LOG_MAX_BYTES: u64 = 50 * 1024 * 1024;
/// 保留的备份数量（`daemon.1.log` ~ `daemon.N.log`，N = LOG_KEEP_BACKUPS）
const LOG_KEEP_BACKUPS: u32 = 3;

/// 日志追加器：文件被删除也能自愈、且日志路径上绝不 panic常驻句柄 + 低频巡检：稳态每条日志一次 write，每写满 [`LOG_VERIFY_BYTES`]用一次 `metadata` 校正尺寸并发现「被外部删除
/// / 已轮转」（代价：外部删除后最多丢 `LOG_VERIFY_BYTES` 字节日志）；循环轮转全程无 panic（所有重命名/删除吞错，轮转前先丢句柄避免写已改名 inode）；锁毒化剥除 poison 继续用
/// 写失败只丢弃当条日志（句柄置空、下条重建），不影响进程存活
// [appender]
#[derive(Debug)]
struct SelfHealingAppender {
    path: PathBuf,
    max_bytes: u64,
    keep: u32,
    /// 常驻写状态（该 Mutex 同时充当原先的串行化锁）
    state: Mutex<AppendState>,
}

/// 追加状态：常驻句柄 + 记账尺寸
#[derive(Debug)]
struct AppendState {
    /// 常驻 append 句柄；轮转或被外部删除后置 None，下次写入重建
    file: Option<fs::File>,
    /// 当前文件尺寸：以本进程记账为主，每 `LOG_VERIFY_BYTES` 用 metadata 校正一次
    size: u64,
    /// 距上次真实尺寸校验已写入的字节数
    since_verify: u64,
}

/// 低频巡检间隔（字节）：外部删除 `daemon.log` 后最坏丢这么多日志（约 60 行）
const LOG_VERIFY_BYTES: u64 = 8 * 1024;

impl SelfHealingAppender {
    /// 备份名：`daemon.log` -> `daemon.1.log`（把扩展名前缀替换为 `.{n}.log`）
    fn archive_path(&self, n: u32) -> PathBuf {
        let stem = self
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parent = self.path.parent().unwrap_or(Path::new(""));
        parent.join(format!("{stem}.{n}.log"))
    }

    /// 低频巡检：用真实尺寸校正记账值；路径消失（被外部删除 / 已被轮转）则丢弃句柄
    fn append_verify(&self, st: &mut AppendState) {
        match fs::metadata(&self.path) {
            Ok(md) => st.size = md.len(),
            Err(_) => {
                st.file = None;
                st.size = 0;
            }
        }
        st.since_verify = 0;
    }

    /// 重建句柄：目录可能也被删，`create_dir_all` 只在这里做；打开后用一次 `metadata` 校正尺寸（覆盖「运行期新建 appender，但文件已有内容」的场景）
    fn append_open(&self, st: &mut AppendState) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        st.file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .ok();
        if st.file.is_some() {
            if let Ok(md) = fs::metadata(&self.path) {
                st.size = md.len();
            }
        }
    }

    /// 无 panic 的循环轮转：`daemon.log -> daemon.1.log -> ... -> daemon.keep.log`，最旧备份被删除；任何一步失败（如文件恰好不存在）都直接忽略
    fn rotate(&self) {
        let _ = fs::remove_file(self.archive_path(self.keep));
        for i in (1..self.keep).rev() {
            let from = self.archive_path(i);
            let to = self.archive_path(i + 1);
            let _ = fs::rename(&from, &to);
        }
        let _ = fs::rename(&self.path, self.archive_path(1));
    }
}

impl SelfHealingAppender {
    /// 落盘一行（已格式化字节）锁内 IO + 计数；`note_write` 达门限会经`log::info!` 重入本函数，调用方持本锁调用会对非重入 Mutex 死锁
    fn append_line(&self, line: &[u8]) {
        let bytes = line.len() as u64;
        {
            let mut st = match self.state.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            // 低频巡检：校正尺寸 / 发现被删
            if st.since_verify >= LOG_VERIFY_BYTES {
                self.append_verify(&mut st);
            }
            // 尺寸达到上限：先丢句柄再轮转（否则会继续写已改名的 inode）
            if st.size >= self.max_bytes {
                st.file = None;
                self.rotate();
                st.size = 0;
            }
            // 句柄缺失（首次 / 被删 / 轮转后）则重建
            if st.file.is_none() {
                self.append_open(&mut st);
            }
            if let Some(f) = st.file.as_mut() {
                if f.write_all(line).and_then(|_| f.flush()).is_ok() {
                    st.size += bytes;
                    st.since_verify += bytes;
                } else {
                    // 写失败（句柄失效 / 磁盘异常）：丢弃句柄，下一条重建
                    st.file = None;
                }
            }
        }
        // 记账：按编码长度计（与写成败无关），决定「日志目录预算 / 128MB 打包门限」触发点
        note_write(&LOGS_BYTES_WRITTEN, "logs/", bytes);
    }
}

// 日志打包门限（事件触发，零额外 syscall）：本会话写入 logs/ 与 devimp/ 的字节数
// 各自累计，任一目录 ≥ LOG_RESTART_THRESHOLD_BYTES（128MB）即退出进程——看门狗
// 3s 后拉起新进程，由 archive_on_startup 在 logger::init 前打包进 logd/（打包只在
// 启动路径发生）按写入字节而非遍历目录记账：写路径仅三条（daemon.log /
// status.csv / devimp 当前诊断文件），事件驱动零 stat，累计值即目录增长量；
// 每进程从 0 计起（启动归档已清空重建两目录）
// [loglimit]
/// logs/ 或 devimp/ 任一目录增长到该大小（128MB）即重启调度以打包日志
const LOG_RESTART_THRESHOLD_BYTES: u64 = 128 * 1024 * 1024;
/// logs/ 本会话累计写入字节（daemon.log + status.csv）
static LOGS_BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
/// devimp/ 本会话累计写入字节（当前诊断文件 main_* 与 aff_* 共用同一目录预算）
static DEVIMP_BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
/// 已进入重启流程：日志落盘走同一写路径，置位后不再记账/重入
static LOG_RESTARTING: AtomicBool = AtomicBool::new(false);

/// 记账并判定门限：`counter` 为本目录累计写入量，`dir` 为目录名（日志展示）。达到门限即退出进程，由看门狗 3s 后拉起走启动归档无看门狗（`watchdog_pid()` 为 None）
/// 不退出——退出后无人拉起、调度永久停止；此时计数清零并打 warn，等下一门限再判退出前打点也走本写路径，以`LOG_RESTARTING` 防重入；**调用方不得持有 appender 锁**（经 `log::info!
/// `重入 append 会对非重入 Mutex 死锁）
fn note_write(counter: &AtomicU64, dir: &str, bytes: u64) {
    // 仅 Chiri 调度启用（非 ChiRi 无调度接管，不做自动重启；devimp 本就不产生）
    if !common::is_chiri_soc() {
        return;
    }
    if LOG_RESTARTING.load(Ordering::Relaxed) {
        return;
    }
    let total = counter.fetch_add(bytes, Ordering::Relaxed) + bytes;
    if total < LOG_RESTART_THRESHOLD_BYTES {
        return;
    }
    if watchdog_pid().is_none() {
        // 抑制必须可见：静默清零零痕迹，无从排查「达到门限但永不重启」
        log::warn!(
            "{}",
            t_with_args(
                "logger-log-restart-suppressed",
                &fluent_args!(
                    "dir" => dir,
                    "mb" => (total / (1024 * 1024)).to_string()
                )
            )
        );
        counter.store(0, Ordering::Relaxed);
        return;
    }
    LOG_RESTARTING.store(true, Ordering::Relaxed);
    log::info!(
        "{}",
        t_with_args(
            "logger-log-restart-for-archive",
            &fluent_args!(
                "dir" => dir,
                "mb" => (total / (1024 * 1024)).to_string()
            )
        )
    );
    std::process::exit(0);
}

// [init]
/// 行格式化：`[YYYY-MM-DD HH:MM:SS] [LEVEL] [module] message\n`（与原 log4rs 编码器输出逐字节一致，本地时间）；
/// 仅剥掉模块路径里重复的 crate 名前缀（包名与子模块同名，前缀白占列宽），非本 crate 模块路径原样保留
fn format_line(record: &Record) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    out.push(b'[');
    write_local_timestamp(&mut out);
    out.extend_from_slice(b"] [");
    out.extend_from_slice(record.level().as_str().as_bytes());
    out.extend_from_slice(b"] [");
    let module = record.module_path().unwrap_or("");
    match module.strip_prefix(CRATE_PREFIX) {
        Some(rest) => out.extend_from_slice(rest.as_bytes()),
        None => out.extend_from_slice(module.as_bytes()),
    }
    out.extend_from_slice(b"] ");
    let _ = std::io::Write::write_fmt(&mut out, *record.args());
    out.push(b'\n');
    out
}

/// crate 名：daemon.log 模块路径里冗余的前缀（crate 自身日志才有）
const CRATE_PREFIX: &str = "chiri::";

/// 写入 `[YYYY-MM-DD HH:MM:SS]`（设备本地时间）。任一步失败都退化为 epoch 时间，绝不在日志路径上 panic
fn write_local_timestamp(out: &mut Vec<u8>) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        out.extend_from_slice(b"1970-01-01 00:00:00");
        return;
    }
    let mut buf = [0u8; 20];
    let n = unsafe {
        libc::strftime(
            buf.as_mut_ptr().cast(),
            buf.len(),
            c"%Y-%m-%d %H:%M:%S".as_ptr(),
            &tm,
        )
    };
    out.extend_from_slice(&buf[..n.min(buf.len())]);
}

/// 构造追加器（原为 log4rs `Config`）：路径 / 上限 / 备份数 + 空状态
fn make_appender() -> SelfHealingAppender {
    SelfHealingAppender {
        path: common::get_module_root().join(LOG_REL_PATH),
        max_bytes: LOG_MAX_BYTES,
        keep: LOG_KEEP_BACKUPS,
        state: Mutex::new(AppendState {
            file: None,
            size: 0,
            since_verify: 0,
        }),
    }
}

/// 自写 `log::Log` 实现（替代 log4rs）：仅保留按行格式化落盘与运行时调级，省去 log4rs 及其传递依赖
struct ChiriLogger {
    appender: SelfHealingAppender,
}

impl Log for ChiriLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        self.appender.append_line(&format_line(record));
    }

    fn flush(&self) {}
}

/// 初始化日志系统，启动时调用一次
pub fn init(level_str: &str) -> Result<()> {
    let level = parse_level(level_str);
    LOG_LEVEL.store(level as u8, Ordering::Release);
    log::set_boxed_logger(Box::new(ChiriLogger {
        appender: make_appender(),
    }))
    .map_err(|e| anyhow!("Logger already initialized: {e}"))?;
    log::set_max_level(level);
    // 启动即落一行模块版本：排查现场第一步即确认这份日志对应的版本
    log_module_version();
    Ok(())
}

/// 模块版本信息：module.prop 的 name / version / versionCode + SoC + kernel。 缺失字段一律 `-`，绝不因读不到文件而 panic（日志路径零 panic 是硬约束）
fn log_module_version() {
    let root = common::get_module_root();
    let (mut name, mut ver, mut code) = (String::from("-"), String::from("-"), String::from("-"));
    if let Ok(text) = fs::read_to_string(root.join("module.prop")) {
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("name=") {
                name = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("version=") {
                ver = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("versionCode=") {
                code = v.trim().to_string();
            }
        }
    }
    let soc = common::matched_soc_hint().unwrap_or("-");
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "-".to_string());
    log::info!(
        "{}",
        t_with_args(
            "main-module-version",
            &fluent_args!(
                "name" => name,
                "version" => ver,
                "code" => code,
                "soc" => soc.to_string(),
                "kernel" => kernel
            )
        )
    );
}

/// 当前生效的日志等级（`update_level` 判变化用；启动时由 init 写入）
static LOG_LEVEL: AtomicU8 = AtomicU8::new(LevelFilter::Info as u8);

/// 动态更新日志等级
pub fn update_level(level_str: &str) {
    let level = parse_level(level_str);
    let prev = LOG_LEVEL.swap(level as u8, Ordering::AcqRel);
    log::set_max_level(level);
    // 变化走 info（INFO 级别下可直接确认新等级已生效），未变化仅 debug 避免每次重载刷一行；注：框架按新 level 过滤，上调等级时这条 info 可能被自身过滤
    if prev != level as u8 {
        log::info!(
            "{}",
            t_with_args(
                "log-level-updated",
                &fluent_args!("level" => level.to_string())
            )
        );
    } else {
        log::debug!(
            "{}",
            t_with_args(
                "log-level-updated",
                &fluent_args!("level" => level.to_string())
            )
        );
    }
}

// 状态日志（logs/status.csv，CSV 宽表）：daemon.log 之外的唯一状态文件，
// 仅一种行类型——snap 行（1s 一条，chiri 调度线程）：遥测 + 热保护 + 模式状态
// + 前台包名 + 充放电状态，前台切换点由相邻行的 package 列变化体现
// 开销控制：Mutex 常驻 append 句柄（每次写入仅一次 write）；每 16 行巡检一次
// （轮转 8MB + 被删自愈）；写失败重开重试一次，全程不 unwrap、不阻塞调度线程
// [status]
/// 状态日志路径（CSV 宽表）
const STATUS_LOG_REL: &str = "logs/status.csv";
/// 单文件上限 8MB，保留 1 份备份；1s 一行，数小时轮转一次
const STATUS_LOG_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// 每 N 行巡检一次（轮转 + 被删自愈）：常驻句柄被外部删除后指向孤儿 inode、写入不报错，只能靠巡检发现；1s 一行时 16 行 ≈ 16s 自愈窗口，stat 开销可忽略
const STATUS_CHECK_EVERY: u64 = 16;

/// CSV 表头（25 列，列序由 status_log_snapshot 保证对齐，完整列名见下方STATUS_HEADER 字符串）fps 为**预留列**，schema 恒定存在：
/// 仅 FAS 激活且帧窗口有样本时为实测值，其余一律 "-"；列只在末尾追加，避免打乱既有列索引。
/// 末尾两列 daemon_utime_ms / daemon_stime_ms = daemon 自身累计用户态/内核态 CPU 时间（ms，自测量基线），
/// 与同行的 batt_power_w 并排可反推「观测者自身开销」占比
const STATUS_HEADER: &str = "timestamp,type,mode,package,charge,screen_on,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,batt_voltage_v,batt_current_ma,batt_power_w,wakeups,migrations,freq_trans,fps,screen_prop,daemon_utime_ms,daemon_stime_ms";

/// 常驻写入器：append 句柄 + 巡检计数
struct StatusWriter {
    file: Option<fs::File>,
    since_check: u64,
}

/// 进程级单例（Mutex::new 为 const，可静态初始化）
static STATUS_WRITER: Mutex<StatusWriter> = Mutex::new(StatusWriter {
    file: None,
    since_check: 0,
});

/// 打开（或重建）状态日志：create+append；空文件补表头。返回 None 表示打开失败（调用方下次写入时再试）
fn status_open() -> Option<fs::File> {
    let path = common::get_module_root().join(STATUS_LOG_REL);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    if f.metadata().map(|m| m.len()).unwrap_or(1) == 0 {
        let _ = f.write_all(STATUS_HEADER.as_bytes());
        let _ = f.write_all(b"\n");
    }
    Some(f)
}

/// 巡检：轮转（超 8MB）与被删自愈（外部删除后 metadata 报错 → 重开重建）
fn status_check(w: &mut StatusWriter) {
    let path = common::get_module_root().join(STATUS_LOG_REL);
    match fs::metadata(&path) {
        Ok(m) if m.len() < STATUS_LOG_MAX_BYTES => return,
        Ok(_) => {
            // 轮转：path -> path.1（旧备份覆盖删除）
            let bak = common::get_module_root().join(format!("{}.1", STATUS_LOG_REL));
            let _ = fs::remove_file(&bak);
            let _ = fs::rename(&path, &bak);
        }
        Err(_) => {} // 文件被删：丢弃旧句柄（fd 仍指向孤儿 inode），下方重开重建
    }
    w.file = status_open();
}

/// 写一行到 status.csv（调用方保证 fields 与 STATUS_HEADER 列数对齐）
fn status_write_line(fields: &[&str]) {
    let line = fields.join(",");
    let mut w = STATUS_WRITER.lock().unwrap_or_else(|p| p.into_inner());
    if w.file.is_none() {
        w.file = status_open();
    }
    let write_ok = match w.file.as_mut() {
        Some(f) => f
            .write_all(line.as_bytes())
            .and_then(|_| f.write_all(b"\n"))
            .is_ok(),
        None => false,
    };
    if !write_ok {
        // 写失败（磁盘/句柄异常）：重开重试一次，仍失败则丢弃本行
        w.file = status_open();
        if let Some(f) = w.file.as_mut() {
            let _ = f.write_all(line.as_bytes());
            let _ = f.write_all(b"\n");
        }
    }
    w.since_check += 1;
    if w.since_check >= STATUS_CHECK_EVERY {
        w.since_check = 0;
        status_check(&mut w);
    }
    drop(w);
    // 记账（含换行）：写入即事件触发，logs/ 累计增长达 128MB 触发重启打包
    note_write(&LOGS_BYTES_WRITTEN, "logs/", line.len() as u64 + 1);
}

// [power_avg]
/// PowerAVG 输出文件（模块根，对外暴露、供 WebUI 只读展示）
pub const POWER_AVG_CHR: &str = "PowerAVG.chr";

/// PowerAVG 递推状态：（当前留存值, 留存次数）。进程级静态——调度线程 panic 重启不丢；daemon 进程重启从零开始（按契约设计：文件只是输出记录，不读回）
static POWER_AVG: Mutex<(f32, u64)> = Mutex::new((0.0, 0));

/// 参考模式权重：上次留存值 : 新采样 = 10 : 1——单次异常读数只拉动 1/11，避免跟着尖峰跳
const REFERENCE_WEIGHT: f32 = 10.0;

/// 启动清空 `PowerAVG.chr`（保留文件、内容置空）：文件是本次运行的输出记录、不读回，上次运行遗留值对 WebUI 是过期读数；属运行时输出，不受 `nofix` 约束
pub fn power_avg_reset() {
    let path = common::get_module_root().join(POWER_AVG_CHR);
    let _ = common::write_file_no_panic(&path, b"");
}

/// 更新 PowerAVG 并写模块根 `PowerAVG.chr`：**在 status.csv 写入流程里顺序调用**（每 1s 采样一次）参考模式（默认）：value = (旧值 × 10 + 新值) / 11，偏历史、
/// 读数稳（单次异常采样只占 1/11）；平均模式：value = (旧值 × 次数 + 新值) /(次数 + 1)，等权全史留存次数两模式都累计（切到平均模式即有历史次数可用）功耗缺测（None/非法）时跳过本次、
/// 不写文件；**取样口径由调用方保证**：仅电池放电；平均模式排除息屏样本（息屏功耗低但占时长，等权会拉低亮屏读数），参考值保留息屏；被跳过样本不推进留存次数、不改动文件返回**当前留存值**（W）：接受则为新值，
/// 跳过沿用上次值，从未采到为 None （调用方据此展示，不必回读文件）
pub fn power_avg_update(power_w: Option<f32>, use_average: bool) -> Option<f32> {
    let current = || {
        let state = POWER_AVG.lock().unwrap_or_else(|e| e.into_inner());
        let (value, count) = *state;
        (count > 0).then_some(value)
    };
    let Some(p) = power_w.filter(|v| v.is_finite() && *v >= 0.0) else {
        return current(); // 本次跳过（缺测/非放电/息屏进均值）：沿用上次留存值
    };
    let mut state = POWER_AVG.lock().unwrap_or_else(|e| e.into_inner());
    let (value, count) = *state;
    let next = if count == 0 {
        p
    } else if use_average {
        (value * count as f32 + p) / (count as f32 + 1.0)
    } else {
        (value * REFERENCE_WEIGHT + p) / (REFERENCE_WEIGHT + 1.0)
    };
    *state = (next, count + 1);
    drop(state);
    // 原子写（tmp+rename）：WebUI 每秒读它展示，绝不能读到半截内容
    let path = common::get_module_root().join(POWER_AVG_CHR);
    let _ = common::write_file_no_panic(&path, format!("{:.2}\n", next).as_bytes());
    Some(next)
}

// [live_time]
/// 心跳文件（模块根，对外暴露、供 WebUI 只读）：每 [`LIVE_TIME_INTERVAL_SECS`]秒写一次本地时间 `MM:SS`WebUI 读它与本机时间比对，差值超容差（20s）
/// 即判调度已关闭（文件缺失/内容非法同判）只写分秒：差值按 1 小时取模判新鲜度，间隔 15s + 容差 20s 远小于半小时，取模不会把旧值误判为新鲜；写文件原子替换
pub const LIVE_TIME_CHR: &str = "LiveTime.chr";

/// 心跳间隔（秒）：与 WebUI 侧容差常量配套（容差必须 > 它，留满一轮余量）
pub const LIVE_TIME_INTERVAL_SECS: u64 = 15;

/// 写一次心跳（`MM:SS` + 换行，本地时间）。失败静默：模块根不可写时 WebUI 自然判定为已关闭，不需要额外告警刷日志
pub fn write_live_time() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let (_, m, s) = local_hms(now.as_secs() as i64).unwrap_or_else(|| {
        let total = now.as_secs() % 86400;
        (
            (total / 3600) as u32,
            ((total % 3600) / 60) as u32,
            (total % 60) as u32,
        )
    });
    let path = common::get_module_root().join(LIVE_TIME_CHR);
    let _ = common::write_file_no_panic(&path, format!("{m:02}:{s:02}\n").as_bytes());
}

/// 缺失数值的占位
const NA: &str = "-";

/// 数值格式化（None → "-"）全精度写入：取整是显示层（WebUI toFixed(1)）的职责
fn fmt_num(v: Option<f32>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| NA.to_string())
}

/// 写 snapshot 行（1s 一条，chiri 调度线程）：遥测 + 热保护 + 模式状态 + 前台包名（切换点由相邻行变化体现）+ 充放电状态（未知为 "-"）+ FAS 实测帧率（`fps` 预留列，
/// 见 STATUS_HEADER）+ `screen_prop`（debug.tracing.screen_state 原始值，属性缺失为 "-"）
#[allow(clippy::too_many_arguments)]
pub fn status_log_snapshot(
    mode: &str,
    package: &str,
    charge: &str,
    screen_on: bool,
    batt_temp: Option<f32>,
    cpu_temp: Option<f32>,
    thermal_cap: &str,
    thermal_free: &str,
    clg_active: bool,
    psi_cpu: &str,
    psi_io: &str,
    psi_mem: &str,
    gpu_busy: &str,
    batt_v: &str,
    batt_i: &str,
    batt_p: &str,
    wakeups: u32,
    migrations: u32,
    freq_trans: u32,
    fps: Option<f32>,
    screen_prop: &str,
    daemon_utime_ms: u64,
    daemon_stime_ms: u64,
) {
    // frozen（待春归）：status.csv 属于该模式要停的「额外开销」；闸口集中在此，所有写入路径过同一道
    if mode == "frozen" {
        return;
    }
    status_write_line(&[
        &format_now(),
        "snap",
        mode,
        package,
        charge,
        if screen_on { "1" } else { "0" },
        &fmt_num(batt_temp),
        &fmt_num(cpu_temp),
        thermal_cap,
        thermal_free,
        if clg_active { "1" } else { "0" },
        psi_cpu,
        psi_io,
        psi_mem,
        gpu_busy,
        batt_v,
        batt_i,
        batt_p,
        &wakeups.to_string(),
        &migrations.to_string(),
        &freq_trans.to_string(),
        &fmt_num(fps),
        screen_prop,
        &daemon_utime_ms.to_string(),
        &daemon_stime_ms.to_string(),
    ]);
}

/// daemon 自身累计 CPU 时间（ms）：读 `/proc/self/stat` 的 utime/stime（第 14/15 字段，单位
/// USER_HZ=100 → ×10 得 ms）；读/解析失败返回 (0,0)。自测量基线用（与 status.csv 同行落盘）
pub fn self_cpu_ms() -> (u64, u64) {
    let Ok(text) = fs::read_to_string("/proc/self/stat") else {
        return (0, 0);
    };
    // comm 可能含空格与括号：从**最后一个** ')' 之后切分，字段序才稳定（'(' 亦可用 rsplit 兜住）
    let Some((_, rest)) = text.rsplit_once(')') else {
        return (0, 0);
    };
    let mut it = rest.split_whitespace();
    // rest 起始于整体第 3 字段(state)：索引 11 = utime(14)，索引 12 = stime(15)
    let utime = it.nth(11).and_then(|v| v.parse::<u64>().ok());
    let stime = it.next().and_then(|v| v.parse::<u64>().ok());
    match (utime, stime) {
        (Some(u), Some(s)) => (u.saturating_mul(10), s.saturating_mul(10)),
        _ => (0, 0),
    }
}

/// HH:MM:SS.mmm 格式**设备本地时间**（避免引入 chrono 依赖）；status.csv /devimp/ / daemon.log 时区统一为本地时间，离线对齐无需人工换算
/// 经 libc localtime_r 走系统时区；不可用时回退 UTC（仅丢时区正确性）
fn format_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let (h, m, s) = local_hms(now.as_secs() as i64).unwrap_or_else(|| {
        let total = now.as_secs() % 86400;
        (
            (total / 3600) as u32,
            ((total % 3600) / 60) as u32,
            (total % 60) as u32,
        )
    });
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, now.subsec_millis())
}

/// epoch 秒 → 本地 (时, 分, 秒)。仅 unix（bionic/glibc 均有 localtime_r）；非 unix 主机恒 None（回退 UTC），不影响 Android 目标
#[cfg(unix)]
fn local_hms(epoch: i64) -> Option<(u32, u32, u32)> {
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t: libc::time_t = epoch as libc::time_t;
        if libc::localtime_r(&t, &mut tm).is_null() {
            return None;
        }
        Some((tm.tm_hour as u32, tm.tm_min as u32, tm.tm_sec as u32))
    }
}

#[cfg(not(unix))]
fn local_hms(_epoch: i64) -> Option<(u32, u32, u32)> {
    None
}

// 开发诊断日志（devimp/ 目录下双文件：main_<前台包名>_<MMDD-HHmmss>.log +
// aff_<MMDD-HHmmss>.log），供离线分析改善调度的按核诊断数据，与 status.csv 分离：
// - 独立目录 `devimp/`（模块根，与 logs/ 平级），启动时随 logs/ 一起归档到
// logd/devimp_<MMDD-HHmmss>.tar.lz4；归档后**不预建空目录**——由首次写入的
// main_open / aff_open 惰性 create_dir_all（dev_record 关闭时不得留痕，含空目录）；
// - **main_ 按前台包名分组**：scheduler_ipc 每秒经 set_diag_package 同步包名，
// 包名变化即下次写入以新包名 + 当前时间戳开新文件（同应用聚合同文件，同秒
// 重开以 -N 后缀去重）；**aff_ 单滚动文件不分包**（线程动作跨包、快照全系统
// 视角，分文件会把流切碎），帧格式见 AFF_HEADER；
// - 文件头 `#` 元信息注释行：模块名/版本、SoC、机型、Android 版本、内核版本
// （diag_meta，两文件共用）；
// - 无包名场景不触发切文件，尚无任何包名时包名段为 `nopkg`；
// - 单文件软上限 128MB：触顶自动换新时间戳文件继续写（不静默停写），不丢数据
// 不 panic；写入巡检（每 256 行）换新文件时清理，仅保留最近 DEVIMP_KEEP_FILES
// 份（main_/aff_ 合并计数，按 mtime 排序，活跃文件不清；旧 devimp_*.log 不迁移）；
// - 总开关 DIAG_ACTIVE 由 scheduler_ipc 按 Config.meta.dev_record 同步（WebUI
// 开关 + 热重载），各写入点统一过 diag_active() 闸门，关闭时零 IO 零分配
// main_ CSV 宽表 + `type` 列（44 列，main_ 只承载决策/状态/事件三类行，
// 线程相关行已迁至 aff_ 帧流）：
// - tick（每决策 tick × 每核心组，**决策签名变化才写 + 2s 心跳**）：调频决策
// 轨迹，package 列自动填充前台包名
// - snap（1s）：环境上下文（PSI/GPU/电池/温度/热压制），关联决策与功耗
// - event：decision(事件名)/reason —— 模式/屏幕/热/配置/触摸等状态变化

/// 开发记录总开关（scheduler_ipc 按 Config.meta.dev_record 同步）
// [devimp_state]
static DIAG_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 当前生效模式名（scheduler_ipc 在启动/模式切换/周期刷新时同步），诊断各行 mode 列自动填充，写入点无需感知模式
static DIAG_MODE: Mutex<String> = Mutex::new(String::new());

/// 当前前台包名（set_diag_package 每秒同步，原始未清洗值），诊断各行 package 列自动填充；行主体有更精确包名时（event）由调用方覆盖。空 = 尚未检测到前台应用，package 列保持 "-"
static DIAG_FG_PKG: Mutex<String> = Mutex::new(String::new());

/// 设置开发记录总开关
pub fn set_diag_active(on: bool) {
    DIAG_ACTIVE.store(on, Ordering::Relaxed);
}

/// 同步当前模式名（诊断行 mode 列填充用）
pub fn set_diag_mode(mode: &str) {
    // frozen 标记顺带维护给 diag_active() 高频读取（见那里的注释）
    DIAG_FROZEN.store(mode == "frozen", Ordering::Relaxed);
    if let Ok(mut m) = DIAG_MODE.lock() {
        *m = mode.to_string();
    }
}

/// 同步前台包名并按需切换 main_ 文件（scheduler_ipc 每秒调用，内部去重）：包名变化即关闭当前文件、下次写入以「新包名 + 当前时间戳」开新文件（同应用聚合同文件）；空包名不触发切换；
/// aff_ 单滚动文件不分包不受影响顺序敏感：先切文件（MAIN_WRITER）后更新 DIAG_FG_PKG（行 package 列数据源），保证切换边界处「行的 package 列」与「所在文件」永不错位
pub fn set_diag_package(pkg: &str) {
    // dev_record 关闭时直接返回：两个消费者（package 列填充、按包名切文件）都只在诊断路径上，关时不得产生任何数据，省掉每秒两把锁与 String 分配；重开后下一秒照常切文件，
    // DIAG_FG_PKG 陈旧值无人读取（读取方都在 diag_active 门控下）
    if !diag_active() {
        return;
    }
    let p = pkg.trim();
    if p.is_empty() {
        return;
    }
    // 包名 → 文件名安全段：只保留字母数字与 . _ -（Android 包名合法字符），异常字符替换丢弃，超长截断，全被过滤时退化为 nopkg
    let seg: String = p
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(64)
        .collect();
    let seg = if seg.is_empty() {
        "nopkg"
    } else {
        seg.as_str()
    };
    // ① 先切换文件（对比写入器当前归属的包名段）。WRITER 临界区内只做
    // writer 自身状态修改（文件 IO 与无锁操作），不获取任何其他锁——
    // 锁序约定见 MAIN_WRITER 定义处
    let pkg_switched = {
        let mut w = MAIN_WRITER.lock().unwrap_or_else(|p| p.into_inner());
        if w.pkg_seg == seg {
            false
        } else {
            w.pkg_seg = seg.to_string();
            // 关闭当前文件（cur_name 一并清空）：下次写入按新包名 + 新时间戳开新文件
            w.file = None;
            w.cur_name = None;
            true
        }
    };
    // tick 节流状态清零在 WRITER 锁释放后执行（新文件首 tick 即记录，不留心跳空窗）
    if pkg_switched {
        main_tick_state_clear();
    }
    // ② 最后更新 DIAG_FG_PKG：先切文件后更新，避免切换边界处读到新包名却写进
    // 旧文件的 1~2 行归属错位（顺序敏感详见函数 doc）
    if let Ok(mut g) = DIAG_FG_PKG.lock() {
        *g = p.to_string();
    }
}

/// frozen（待春归）标记：由 `set_diag_mode`（低频）维护，供 `diag_active()` 高频读取。单独存一份原子量，是为了让那条闸门不抢 `DIAG_MODE` 的锁（见函数内注释）
static DIAG_FROZEN: AtomicBool = AtomicBool::new(false);

/// 开发记录是否开启（各写入点检查；关闭时不产生任何 IO/分配）
pub fn diag_active() -> bool {
    if !DIAG_ACTIVE.load(Ordering::Relaxed) {
        return false;
    }
    // frozen：诊断记录是 40ms/tick 级高频写入，属该模式要停的「额外开销」，按当前模式二次判定；本函数是每条行/帧写入前的闸门，只读原子量、不抢 DIAG_MODE 锁
    if DIAG_FROZEN.load(Ordering::Relaxed) {
        return false;
    }
    true
}

/// devimp 目录相对模块根的路径（目录/归档/记账派生物保留 devimp 名，main_* 与 aff_* 两种诊断文件同住此目录）
const DEVIMP_DIR_REL: &str = "devimp";
/// 保留的历史文件数（按文件 mtime 从旧到新删除，当前活跃文件除外）；按包名分组后单轮会话可能产生多份文件，较旧版（一进程一文件）放宽。 main_*.log 与 aff_*.log 合并计数
const DEVIMP_KEEP_FILES: usize = 20;
/// 单文件软上限：触顶换新时间戳文件继续写（不静默停写），main_ / aff_ 同口径
const DEVIMP_MAX_BYTES: u64 = 128 * 1024 * 1024;
/// 每 N 行巡检一次（触顶换文件 + 被删自愈），main_ / aff_ 同口径
const DEVIMP_CHECK_EVERY: u64 = 256;
/// tick 行无变化时的心跳间隔（决策签名不变时每 2s 仍写一条，保证时间轴连续）
const MAIN_TICK_HEARTBEAT: Duration = Duration::from_secs(2);

/// tick 行节流状态：cluster 名 → (上次写入的决策签名, 上次写入时刻)。签名只含决策结果字段（decision/tgt_perf/cur_freq/max_freq/thermal/touch/防抖进度），
/// util/over/under 等逐 tick 抖动的观测值不参与——稳态不写，过渡期逐 tick 记录
static MAIN_TICK_STATE: OnceLock<Mutex<HashMap<String, (String, Instant)>>> = OnceLock::new();

fn main_tick_state() -> &'static Mutex<HashMap<String, (String, Instant)>> {
    MAIN_TICK_STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 清空 tick 节流状态（包名切换/触顶换新文件时调用：新文件首 tick 即记录，不留心跳空窗）
fn main_tick_state_clear() {
    main_tick_state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clear();
}

/// CSV 表头（列 schema，**44 列**，行类型无关——tick/snap/event 只是 type 列取值，各写既有列子集、其余留 "-"，禁止按行类型增删列）；
/// 列序由 main_tick /main_snap / main_event 的写入保证对齐
// [devrow]
/// 列 36-43：CPU 各 policy 与 GPU 各节点的**实际**频率/上下限/调速器（追加在末尾，不改动既有列索引）多 policy（节点）用 `;` 分隔，每项 `policy<id>:
/// <值>`（GPU 为 `<设备名>:<值>`），读不到写 `-`；只在 snap 行填充，其余行类型留 "-"区别于 cur_freq_khz / max_freq_khz：
/// 那两列是 TunedGovernor/CLG 的**决策值**，这几列是**内核当前实际值**
const MAIN_HEADER: &str = "ts,type,mode,screen_on,pid,package,tid,comm,cluster,core,max_util,over_cores,under_cores,cur_perf,tgt_perf,cur_freq_khz,max_freq_khz,decision,deb_up,deb_down,reason,thermal_cap_pct,touch,psi_cpu,psi_io,psi_mem,gpu_busy,batt_v,batt_i,batt_p,wakeups,migrations,freq_trans,batt_temp,cpu_temp,clg_active,cpu_cur_khz,cpu_max_khz,cpu_min_khz,cpu_governor,gpu_cur_khz,gpu_max_khz,gpu_min_khz,gpu_governor";

// 列索引常量（MainRow.set 用，调用方按列语义取用；48→44 列重排，保持其余列相对顺序不变）
const DM_TS: usize = 0;
const DM_TYPE: usize = 1;
const DM_MODE: usize = 2;
const DM_SCREEN: usize = 3;
/// pid/tid/comm/core 四列（列位保留）：place/aff/core 行迁出后暂无写入方，常量保留供列位对齐与后续行类型使用
#[allow(dead_code)]
const DM_PID: usize = 4;
const DM_PKG: usize = 5;
#[allow(dead_code)]
const DM_TID: usize = 6;
#[allow(dead_code)]
const DM_COMM: usize = 7;
const DM_CLUSTER: usize = 8;
#[allow(dead_code)]
const DM_CORE: usize = 9;
const DM_MAXUTIL: usize = 10;
const DM_OVER: usize = 11;
const DM_UNDER: usize = 12;
const DM_CURPERF: usize = 13;
const DM_TGTPERF: usize = 14;
const DM_CURFREQ: usize = 15;
const DM_MAXFREQ: usize = 16;
const DM_DECISION: usize = 17;
const DM_DEBUP: usize = 18;
const DM_DEBDOWN: usize = 19;
const DM_REASON: usize = 20;
const DM_THERMAL: usize = 21;
const DM_TOUCH: usize = 22;
const DM_PSICPU: usize = 23;
const DM_PSIIIO: usize = 24;
const DM_PSIMEM: usize = 25;
const DM_GPU: usize = 26;
const DM_BATTV: usize = 27;
const DM_BATTI: usize = 28;
const DM_BATTP: usize = 29;
const DM_WAKEUPS: usize = 30;
const DM_MIGR: usize = 31;
const DM_FREQT: usize = 32;
const DM_BATTTEMP: usize = 33;
const DM_CPUTEMP: usize = 34;
const DM_CLGACT: usize = 35;
// 36-43：CPU/GPU 实际频率与调速器（原 48 列版的 40-47，见 MAIN_HEADER 上方注释，仅 snap 行填充）
const DM_CPUCUR: usize = 36;
const DM_CPUMAX: usize = 37;
const DM_CPUMIN: usize = 38;
const DM_CPUGOV: usize = 39;
const DM_GPUCUR: usize = 40;
const DM_GPUMAX: usize = 41;
const DM_GPUMIN: usize = 42;
const DM_GPUGOV: usize = 43;

/// 一行诊断记录（固定 44 列，未用列填 "-"），由各 main_* 函数填充
struct MainRow([String; 44]);

impl MainRow {
    /// 新建一行：ts/type/mode/package 已填（mode/package 读全局同步值，无前台包名时 package 留 "-"），其余置 "-"
    fn new(kind: &str) -> Self {
        let mut row = MainRow(std::array::from_fn(|_| NA.to_string()));
        row.0[DM_TS] = format_now();
        row.0[DM_TYPE] = kind.to_string();
        if let Ok(m) = DIAG_MODE.lock() {
            row.0[DM_MODE] = m.clone();
        }
        if let Ok(g) = DIAG_FG_PKG.lock() {
            if !g.is_empty() {
                row.0[DM_PKG] = g.clone();
            }
        }
        row
    }

    fn set(&mut self, idx: usize, v: impl Into<String>) -> &mut Self {
        self.0[idx] = v.into();
        self
    }

    /// 覆盖 package 列：仅当调用方携带有效包名（非空且非 "-"）时覆盖自动填充值；空/"-" 视为未提供，保留前台包名（如 event 行与具体包名无关）
    fn set_pkg(&mut self, pkg: &str) -> &mut Self {
        if !pkg.is_empty() && pkg != NA {
            self.0[DM_PKG] = pkg.to_string();
        }
        self
    }
}

/// 常驻写入器：append 句柄 + 当前文件名 + 当前包名段 + 巡检计数
/// `cur_name` 为 None 时，下次写入按 `pkg_seg` + 当前时间戳确定新文件名
/// （包名切换与 128MB 触顶续写都走这条路径）
/// 锁序约定（防死锁，诊断子系统五把锁：`DIAG_MODE`/`DIAG_FG_PKG`/
/// `MAIN_TICK_STATE`/`MAIN_WRITER`/`AFF_WRITER`）：
/// 1. 各锁均为短临界区，**不嵌套持有**（获取下一个锁前必须释放上一个）；
/// 2. WRITER 临界区内只允许文件 IO 与 writer 自身状态修改，不获取任何其他锁
/// ——它是写路径汇合点（scheduler_ipc / CLG Worker / 亲和线程都写入），嵌套最易成环；
/// 3. 两个写入器互不同时持有，也不与其他任何锁同时持有（两文件独立写入，无跨文件原子性需求）
/// 违反任一条都有死锁风险（如反向先取 WRITER 再取 FG_PKG）
// [devimp_writer]
struct MainWriter {
    file: Option<fs::File>,
    /// 当前文件名（含包名段与文件创建时间戳）
    cur_name: Option<String>,
    /// 当前文件名的包名段（空串 = 尚无前台包名，命名退化为 nopkg）
    pkg_seg: String,
    since_check: u64,
}

static MAIN_WRITER: Mutex<MainWriter> = Mutex::new(MainWriter {
    file: None,
    cur_name: None,
    pkg_seg: String::new(),
    since_check: 0,
});

/// 本地时间文件名时间戳：`MMDD-HHmmss`（人眼可辨）经 libc::localtime_r 取设备本地时区；localtime_r 失败回退 epoch 秒的十六进制
fn filename_ts() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::localtime_r(&secs, &mut tm) };
    if ok.is_null() {
        return format!("t{secs:x}");
    }
    format!(
        "{:02}{:02}-{:02}{:02}{:02}",
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// 生成新文件名：`main_<包名段>_<MMDD-HHmmss>.log`（包名段空 → nopkg）。时间戳在每次开新文件时取当前本地时刻，同一包名触顶续写也会得到新文件名。同秒内重开（极端：包名快速抖动）
/// 由 main_open 的存在性检测补 -N 后缀去重
fn main_new_name(pkg_seg: &str) -> String {
    if pkg_seg.is_empty() {
        format!("main_nopkg_{}.log", filename_ts())
    } else {
        format!("main_{pkg_seg}_{}.log", filename_ts())
    }
}

/// 诊断文件头元信息（`#` 注释行，解析跳过，main_ / aff_ 两文件共用）：处理器型号、系统版本、模块版本等，方便多设备/多版本日志离线比对。进程内只收集一次
fn diag_meta() -> &'static String {
    static META: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    META.get_or_init(|| {
        let root = common::get_module_root();
        // module.prop：name / version / versionCode
        let (mut m_name, mut m_ver, mut m_code) =
            (String::from("-"), String::from("-"), String::from("-"));
        if let Ok(text) = fs::read_to_string(root.join("module.prop")) {
            for line in text.lines() {
                if let Some(v) = line.strip_prefix("name=") {
                    m_name = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("version=") {
                    m_ver = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("versionCode=") {
                    m_code = v.trim().to_string();
                }
            }
        }
        // /system/build.prop：机型 / 平台 / Android 版本
        let (mut model, mut board, mut release, mut sdk) = (
            String::from("-"),
            String::from("-"),
            String::from("-"),
            String::from("-"),
        );
        if let Ok(text) = fs::read_to_string("/system/build.prop") {
            for line in text.lines() {
                if let Some(v) = line.strip_prefix("ro.product.model=") {
                    model = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("ro.board.platform=") {
                    board = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("ro.build.version.release=") {
                    release = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("ro.build.version.sdk=") {
                    sdk = v.trim().to_string();
                }
            }
        }
        // getprop 拿跨分区属性的合并视图：ro.product.model 等常不在 /system/build.prop （Android 10+ 属 /product、/vendor 分区）；
        // getprop 为空时保留上面解析值兜底
        let gp = |k: &str| crate::common::getprop(k);
        let m = gp("ro.product.model");
        if !m.is_empty() {
            model = m;
        }
        let b = gp("ro.board.platform");
        if !b.is_empty() {
            board = b;
        }
        let r = gp("ro.build.version.release");
        if !r.is_empty() {
            release = r;
        }
        let s = gp("ro.build.version.sdk");
        if !s.is_empty() {
            sdk = s;
        }
        let soc = common::matched_soc_hint().unwrap_or("-");
        let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "-".to_string());
        format!(
            "# module={m_name} {m_ver} (versionCode {m_code})\n\
             # soc={soc} board={board} model={model}\n\
             # android={release} (sdk {sdk}) kernel={kernel}\n\
             # ts-column=local format_now\n"
        )
    })
}

/// 诊断文件头（schema 行 + `#` 元信息注释行）拼接，main_ 与 aff_ 共用（`schema` 分别传 [`MAIN_HEADER`] / [`AFF_HEADER`]；元信息进程内只收集一次）
fn diag_file_head(schema: &str) -> Vec<u8> {
    let mut head = Vec::with_capacity(schema.len() + diag_meta().len() + 1);
    head.extend_from_slice(schema.as_bytes());
    head.push(b'\n');
    head.extend_from_slice(diag_meta().as_bytes());
    head
}

/// aff_ 文件头：帧格式说明 + 元信息。元信息的 ts 说明行按文件改写—— main_ 的 ts 列是 format_now（HH:MM:SS.mmm），aff_ 帧 `ts=` 是 MMDD-HHmmss（与文件名同款），
/// 两文件不能共用同一句 ts 口径
fn aff_file_head() -> Vec<u8> {
    let meta = diag_meta().replace(
        "# ts-column=local format_now",
        "# ts-column=local MMDD-HHmmss（帧 ts 字段，与文件名同款）",
    );
    let mut head = Vec::with_capacity(AFF_HEADER.len() + meta.len() + 1);
    head.extend_from_slice(AFF_HEADER.as_bytes());
    head.push(b'\n');
    head.extend_from_slice(meta.as_bytes());
    head
}

/// 打开（或重建）main_ 诊断日志：create+append；空文件整块写入文件头（CSV 表头 + `#` 元信息注释行）`cur_name` 为空则按包名段 + 当前本地时间戳确定新文件名并记入写入器；
/// 同秒重开以 -N 后缀去重返回 None 表示打开失败（调用方下次写入时再试）
fn main_open(w: &mut MainWriter) -> Option<fs::File> {
    let dir = common::get_module_root().join(DEVIMP_DIR_REL);
    // devimp/ 目录的唯一创建者（含启动期）：本函数只在 diag_active() 门控的写入路径上被调用，故目录的创建天然受 dev_record 开关约束（diag_prepare 不再代创建）
    let _ = fs::create_dir_all(&dir);
    let name = match w.cur_name.clone() {
        Some(n) => n,
        None => {
            let base = main_new_name(&w.pkg_seg);
            let stem = base.strip_suffix(".log").unwrap_or(&base);
            let mut name = base.clone();
            let mut found = false;
            for n in 1..100u32 {
                let candidate = if n == 1 {
                    base.clone()
                } else {
                    format!("{stem}-{n}.log")
                };
                if !dir.join(&candidate).exists() {
                    name = candidate;
                    found = true;
                    break;
                }
                name = candidate;
            }
            if !found {
                // 99 个候选全部存在（同秒理论极限）：绝不复用已有文件追加（会不写文件头混流），用 pid 做最后唯一化
                name = format!("{stem}-{}.log", std::process::id());
            }
            w.cur_name = Some(name.clone());
            name
        }
    };
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(&name))
        .ok()?;
    if f.metadata().map(|m| m.len()).unwrap_or(1) == 0 {
        let _ = f.write_all(&diag_file_head(MAIN_HEADER));
    }
    Some(f)
}

/// 巡检：触顶换新时间戳文件继续写（修复旧版触顶静默停写）、被删自愈（丢弃句柄后同名重建）、触顶换新文件后做容量清理。返回是否发生触顶换文件（调用方在 WRITER 锁释放后据此清 tick 节流状态，
/// 避免 WRITER 临界区内嵌套获取 TICK_STATE 锁）
fn main_check(w: &mut MainWriter) -> bool {
    let mut rotated = false;
    if let Some(name) = w.cur_name.clone() {
        let path = common::get_module_root().join(DEVIMP_DIR_REL).join(&name);
        match fs::metadata(&path) {
            // 触顶：关闭当前文件，换新时间戳文件继续写（不丢数据）
            Ok(m) if m.len() >= DEVIMP_MAX_BYTES => {
                w.file = None;
                w.cur_name = None;
                rotated = true;
            }
            Ok(_) => {}
            // 文件被删：丢弃旧句柄（fd 指向孤儿 inode），下方同名重建
            Err(_) => w.file = None,
        }
    }
    if w.file.is_none() {
        let f = main_open(w);
        w.file = f;
        if rotated {
            diag_prune(w.cur_name.as_deref());
        }
    }
    rotated
}

/// 容量清理：仅保留最近 DEVIMP_KEEP_FILES 份诊断文件（`main_*.log` 与 `aff_*.log` 两种前缀合并计数），从旧到新删除；`current` 为当前活跃文件名，不参与清理
/// 旧 `devimp_*.log` 不在过滤前缀内（不迁移，随启动归档自然过期淘汰）。文件名含包名段，字典序不再等于时间序，因此按文件 mtime 排序
fn diag_prune(current: Option<&str>) {
    let dir = common::get_module_root().join(DEVIMP_DIR_REL);
    let mut files: Vec<(std::time::SystemTime, String)> = fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if !(name.starts_with("main_") || name.starts_with("aff_"))
                        || !name.ends_with(".log")
                    {
                        return None;
                    }
                    if Some(name.as_str()) == current {
                        return None;
                    }
                    let mtime = e.metadata().ok()?.modified().ok()?;
                    Some((mtime, name))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    while files.len() > DEVIMP_KEEP_FILES {
        let (_, old) = files.remove(0);
        let _ = fs::remove_file(dir.join(old));
    }
}

// main_ 行拼接缓冲（**线程局部复用**）：devimp 行由同一线程高频写出，复用缓冲避免每行一次
// `row.0.join(",")` 的 String 分配；仅在持 MAIN_WRITER 锁写出期间借用，`with` 返回前归还
thread_local! {
    static MAIN_LINE_BUF: std::cell::RefCell<String> =
        std::cell::RefCell::new(String::with_capacity(512));
}

/// 写一行到 main_ 诊断日志（未开启开关时不产生任何 IO）
fn main_write_line(row: MainRow) {
    if !diag_active() {
        return;
    }
    // 拼接 + 写入都在 MAIN_LINE_BUF.with 内完成，写出用的正是复用缓冲（不再每次分配 String）
    let (rotated, line_len) = MAIN_LINE_BUF.with(|buf| {
        let mut line = buf.borrow_mut();
        line.clear();
        for (i, f) in row.0.iter().enumerate() {
            if i > 0 {
                line.push(',');
            }
            line.push_str(f);
        }
        // WRITER 临界区内只做写入与巡检（文件 IO），不获取任何其他锁；触顶换文件后的 tick 节流清零移到锁释放之后（锁序约定见 MAIN_WRITER）
        let rotated = {
            let mut w = MAIN_WRITER.lock().unwrap_or_else(|p| p.into_inner());
            if w.file.is_none() {
                let f = main_open(&mut w);
                w.file = f;
            }
            let write_ok = match w.file.as_mut() {
                Some(f) => f
                    .write_all(line.as_bytes())
                    .and_then(|_| f.write_all(b"\n"))
                    .is_ok(),
                None => false,
            };
            if !write_ok {
                // 写失败（磁盘/句柄异常）：重开重试一次，仍失败则丢弃本行
                let f = main_open(&mut w);
                w.file = f;
                if let Some(f) = w.file.as_mut() {
                    let _ = f.write_all(line.as_bytes());
                    let _ = f.write_all(b"\n");
                }
            }
            w.since_check += 1;
            if w.since_check >= DEVIMP_CHECK_EVERY {
                w.since_check = 0;
                main_check(&mut w)
            } else {
                false
            }
        };
        (rotated, line.len())
    });
    if rotated {
        // 触顶换新文件：清 tick 节流状态（新文件首 tick 即记录，不留心跳空窗）
        main_tick_state_clear();
        // 触顶即新增了一个 128MB 级文件：顺手执行目录预算清理（logd/ 与 devimp/ 各自独立计量，各自超 256MB 才从本目录最旧文件删到 <200MB）
        enforce_dir_limits(&common::get_module_root());
    }
    // 记账（含换行）：写入即事件触发，devimp/ 累计增长达 128MB 触发重启打包（在 WRITER 锁外调用，遵守锁序约定）
    note_write(&DEVIMP_BYTES_WRITTEN, "devimp/", line_len as u64 + 1);
}

/// 启动兜底清理：仅保留最近 DEVIMP_KEEP_FILES 份历史诊断文件（main_/aff_ 合并
/// 计数，按 mtime 从旧到新删）main.rs 启动时调用一次；正常路径 devimp/ 已被
/// 启动归档整体 rename 走，此函数仅作归档 rename 失败时的兜底
/// **目录不存在即早退、绝不代创建**：devimp/ 整体属 dev_record 产物，关闭时不得
/// 留痕（含空目录）；目录唯一创建者是 diag_active 门控下的 main_open / aff_open
// [devimp_api]
pub fn diag_prepare() {
    let dir = common::get_module_root().join(DEVIMP_DIR_REL);
    // 早退：devimp/ 不存在 → 无历史文件可清理，且绝不代 dev_record 创建目录
    if !dir.exists() {
        return;
    }
    diag_prune(None);
}

/// 看门狗 PID（daemon 由看门狗 sh 前台拉起，`getppid()` 即其 PID）
/// 判据两条满足其一：
/// 1. **pidfile 铁证**：`logs/watchdog.pid` 内容 == `getppid()`（看门狗启动时
/// `echo $$` 自写、归档时复制回、缺失由 `ensure_watchdog_pid_file` 自愈）；
/// 2. **脱管 shell**（`detached_shell_parent`）：父进程是被 init 收养的 shell
/// （看门狗 setsid+disown 后必为孤儿；调试直跑的交互 shell 祖字段 ≠ 1）
fn watchdog_pid() -> Option<i32> {
    let ppid = unsafe { libc::getppid() };
    if ppid <= 1 {
        return None;
    }
    let pid_path = common::get_module_root().join("logs/watchdog.pid");
    if let Ok(s) = fs::read_to_string(&pid_path) {
        if s.trim().parse::<i32>() == Ok(ppid) {
            return Some(ppid);
        }
    }
    detached_shell_parent()
}

/// 父进程是否为「脱管的 shell」（comm 属 shell 家族且已被 init 收养）。/proc/<ppid>/stat 第 4 字段 = 其父 pid：
/// ==1 即孤儿态（setsid + disown 的看门狗必然如此），交互终端链完好的调试直跑不会命中
fn detached_shell_parent() -> Option<i32> {
    let ppid = unsafe { libc::getppid() };
    if ppid <= 1 {
        return None;
    }
    let comm = fs::read_to_string(format!("/proc/{ppid}/comm")).unwrap_or_default();
    if !matches!(
        comm.trim(),
        "sh" | "mksh" | "ash" | "bash" | "dash" | "busybox" | "toybox"
    ) {
        return None;
    }
    // stat 的 comm 字段可含空格/括号，从最后一个 ')' 之后取：state ppid …
    let stat = fs::read_to_string(format!("/proc/{ppid}/stat")).unwrap_or_default();
    let grand = stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(1))
        .and_then(|s| s.parse::<i32>().ok());
    if grand == Some(1) { Some(ppid) } else { None }
}

/// logs/watchdog.pid 运行时自愈：logs/ 被外部删除时该文件随目录消失，WebUI 「关闭调度」靠它定位并终止看门狗——缺失会导致 stopScheduler 杀不死看门狗、
/// 看门狗 3s 后再把 daemon 拉起，最终新旧实例并行、各路日志各写两份由 chiri scheduler_ipc 的 5s 周期块调用；文件存在时零开销直返
pub fn ensure_watchdog_pid_file() {
    let root = common::get_module_root();
    let pid_path = root.join("logs/watchdog.pid");
    if pid_path.exists() {
        return;
    }
    if fs::create_dir_all(root.join("logs")).is_err() {
        return;
    }
    // 非看门狗拉起（调试直跑/孤儿态）不写，防 stopScheduler 误杀无关进程。重建判定用脱管 shell 判据（文件正是缺失状态，pidfile 无从匹配）
    let Some(ppid) = detached_shell_parent() else {
        return;
    };
    let _ = fs::write(&pid_path, ppid.to_string());
}

/// tick 行：CLG/akmode 调频决策轨迹（每决策 tick × 每核心组一行）。写入量控制：按 cluster 节流——决策签名变化即写，无变化时每 2s 心跳一条；
/// util/over/under 等逐 tick 抖动的观测值不触发写入；防抖与升降过渡期仍逐 tick 记录
#[allow(clippy::too_many_arguments)]
pub fn main_tick(
    cluster: &str,
    max_util: &str,
    over: u32,
    under: u32,
    cur_perf: &str,
    tgt_perf: &str,
    cur_freq_khz: &str,
    max_freq_khz: &str,
    decision: &str,
    deb_up: u32,
    deb_down: u32,
    thermal_cap_pct: &str,
    touch_active: bool,
) {
    if !diag_active() {
        return;
    }
    let sig = format!(
        "{decision}|{tgt_perf}|{cur_freq_khz}|{max_freq_khz}|{thermal_cap_pct}|{touch_active}|{deb_up}|{deb_down}"
    );
    let now = Instant::now();
    let should_write = {
        let mut st = main_tick_state().lock().unwrap_or_else(|p| p.into_inner());
        match st.get_mut(cluster) {
            Some((last_sig, last_t)) => {
                if *last_sig == sig && now.duration_since(*last_t) < MAIN_TICK_HEARTBEAT {
                    false
                } else {
                    *last_sig = sig;
                    *last_t = now;
                    true
                }
            }
            None => {
                st.insert(cluster.to_string(), (sig, now));
                true
            }
        }
    };
    if !should_write {
        return;
    }
    let mut r = MainRow::new("tick");
    r.set(DM_CLUSTER, cluster)
        .set(DM_MAXUTIL, max_util)
        .set(DM_OVER, over.to_string())
        .set(DM_UNDER, under.to_string())
        .set(DM_CURPERF, cur_perf)
        .set(DM_TGTPERF, tgt_perf)
        .set(DM_CURFREQ, cur_freq_khz)
        .set(DM_MAXFREQ, max_freq_khz)
        .set(DM_DECISION, decision)
        .set(DM_DEBUP, deb_up.to_string())
        .set(DM_DEBDOWN, deb_down.to_string())
        .set(DM_THERMAL, thermal_cap_pct)
        .set(DM_TOUCH, if touch_active { "1" } else { "0" });
    main_write_line(r);
}

/// snap 行：1s 环境上下文（遥测 + 热保护；前台包名由 MainRow 自动填充，scheduler_ipc 每秒 set_diag_package 与此处同值）
#[allow(clippy::too_many_arguments)]
pub fn main_snap(
    screen_on: bool,
    batt_temp: &str,
    cpu_temp: &str,
    thermal_cap_pct: &str,
    clg_active: bool,
    psi_cpu: &str,
    psi_io: &str,
    psi_mem: &str,
    gpu_busy: &str,
    batt_v: &str,
    batt_i: &str,
    batt_p: &str,
    wakeups: u32,
    migrations: u32,
    freq_trans: u32,
    cpu_cur: &str,
    cpu_max: &str,
    cpu_min: &str,
    cpu_gov: &str,
    gpu_cur: &str,
    gpu_max: &str,
    gpu_min: &str,
    gpu_gov: &str,
) {
    let mut r = MainRow::new("snap");
    r.set(DM_SCREEN, if screen_on { "1" } else { "0" })
        .set(DM_BATTTEMP, batt_temp)
        .set(DM_CPUTEMP, cpu_temp)
        .set(DM_THERMAL, thermal_cap_pct)
        .set(DM_CLGACT, if clg_active { "1" } else { "0" })
        .set(DM_PSICPU, psi_cpu)
        .set(DM_PSIIIO, psi_io)
        .set(DM_PSIMEM, psi_mem)
        .set(DM_GPU, gpu_busy)
        .set(DM_BATTV, batt_v)
        .set(DM_BATTI, batt_i)
        .set(DM_BATTP, batt_p)
        .set(DM_WAKEUPS, wakeups.to_string())
        .set(DM_MIGR, migrations.to_string())
        .set(DM_FREQT, freq_trans.to_string())
        // CPU/GPU 实际频率与调速器：内核当前值，区别于 DM_CURFREQ/DM_MAXFREQ 的决策值
        .set(DM_CPUCUR, cpu_cur)
        .set(DM_CPUMAX, cpu_max)
        .set(DM_CPUMIN, cpu_min)
        .set(DM_CPUGOV, cpu_gov)
        .set(DM_GPUCUR, gpu_cur)
        .set(DM_GPUMAX, gpu_max)
        .set(DM_GPUMIN, gpu_min)
        .set(DM_GPUGOV, gpu_gov);
    main_write_line(r);
}

/// event 行：状态变化（decision 列记事件名：mode_change/screen/thermal_change/config_reload/touch/ak_cooldown、overload_hold 等；
/// reason 记详情）。pkg 有效时覆盖自动填充（mode_change 携带触发包名；screen/thermal 等系统事件保留前台包名上下文）
pub fn main_event(kind: &str, pkg: &str, reason: &str) {
    let mut r = MainRow::new("event");
    r.set(DM_DECISION, kind).set_pkg(pkg).set(DM_REASON, reason);
    main_write_line(r);
}

// [aff_writer]
/// aff_ 帧格式说明（文件头 schema，`#` 注释行解析跳过；正文按行首字符定界，帧格式详见下方 AFF_HEADER 字符串：@A 单行动作帧、@S 快照帧按帧头计数定界）
const AFF_HEADER: &str = "# aff_ 线程数据文件（帧式文本，2026-09-28 字段级差分版）\n\
# @A ts=<MMDD-HHmmss> act=<pin|restore|move_group|cpuset_cpus|uclamp|corectl|bind_release|self_pin|self_unpin|elf32|fdp> pid=<i32> tid=<i32> pkg=<str> comm=<str> dst=<str> value=<str> result=<ok|e{errno}> reason=<str>\n\
# @S ts=<MMDD-HHmmss> ntop=<进程行数> nfg=<线程行数>[ full=1]\n\
# p <rank> <pid> <pkg|comm> u=<整数util%> mask=<允许核hex> home=<核|-1>\n\
# t <pid> <tid>[ comm][ u=][ core=][ home=][ pin=][ uclamp=]\n\
# t 行槽序固定 comm/u/core/home/pin/uclamp，pid/tid 是行标识恒写：与上次落盘值相同的槽写 -，\n\
#   尾部连续未变的槽整段省略 —— 只承载「本次变化的那部分」，缺省槽 = 沿用上次值（勿把 - 当字段值）。\n\
#   槽真值本身就是 - 时改写 NaN（只有 comm 取得到 -；数值槽是整数），读者还原为 -。\n\
# t 行级差分：任一槽变化才落行，**整行全未变则不落该行**（缺失 tid = 与上次落盘值完全相同）。\n\
# 刷新帧：@S 帧头带 full=1（首帧与每 30 帧一次）——所有存活候选 tid 全量落行，\n\
#   下游据此重建存活集合（连续两次刷新帧都不出现的 tid 视为已退出，粒度 = 刷新间隔）。\n\
#   长尾行的 u 是自上次落盘（最多 30s）窗口的平均 util%，其余行仍是 1s 窗口值。\n\
# 帧边界：@A 单行自帧；@S 按帧头 ntop+nfg 计数定界，末尾截断帧直接丢弃。\n\
# 行首字符 \\x01 保留给将来的二进制帧（本版不实现）。全字段空白/控制字符净化为 _。\n\
# t 行 pid=0 表示归属未知（后台候选建档线程）；uclamp 槽本版恒不变（预留字段），故除刷新帧外不出现。";

struct AffWriter {
    file: Option<fs::File>,
    /// 当前文件名（含创建时间戳；单滚动文件不分包）
    cur_name: Option<String>,
    since_check: u64,
}

static AFF_WRITER: Mutex<AffWriter> = Mutex::new(AffWriter {
    file: None,
    cur_name: None,
    since_check: 0,
});

/// 生成新文件名：`aff_<MMDD-HHmmss>.log`（单滚动不分包；同秒重开由 aff_open 的存在性检测补 -N 后缀去重）
fn aff_new_name() -> String {
    format!("aff_{}.log", filename_ts())
}

/// 打开（或重建）aff_ 线程数据文件：create+append；空文件整块写入文件头（帧格式说明 + `#` 元信息注释行）。返回 None 表示打开失败（下次写入再试）
fn aff_open(w: &mut AffWriter) -> Option<fs::File> {
    let dir = common::get_module_root().join(DEVIMP_DIR_REL);
    // devimp/ 目录的唯一创建者（含启动期）：同 main_open，本函数只在 diag_active()门控的写入路径上被调用
    let _ = fs::create_dir_all(&dir);
    let name = match w.cur_name.clone() {
        Some(n) => n,
        None => {
            let base = aff_new_name();
            let stem = base.strip_suffix(".log").unwrap_or(&base);
            let mut name = base.clone();
            for n in 1..100u32 {
                let candidate = if n == 1 {
                    base.clone()
                } else {
                    format!("{stem}-{n}.log")
                };
                if !dir.join(&candidate).exists() {
                    name = candidate;
                    break;
                }
                name = candidate;
            }
            w.cur_name = Some(name.clone());
            name
        }
    };
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(&name))
        .ok()?;
    if f.metadata().map(|m| m.len()).unwrap_or(1) == 0 {
        let _ = f.write_all(&aff_file_head());
    }
    Some(f)
}

/// 巡检：触顶换新时间戳文件继续写、被删自愈（同名重建）、换新后容量清理。与 main_check 同口径（aff_ 无 tick 节流，返回值仅用于调用方触发目录预算清理）
fn aff_check(w: &mut AffWriter) -> bool {
    let mut rotated = false;
    if let Some(name) = w.cur_name.clone() {
        let path = common::get_module_root().join(DEVIMP_DIR_REL).join(&name);
        match fs::metadata(&path) {
            Ok(m) if m.len() >= DEVIMP_MAX_BYTES => {
                w.file = None;
                w.cur_name = None;
                rotated = true;
            }
            Ok(_) => {}
            Err(_) => w.file = None,
        }
    }
    if w.file.is_none() {
        let f = aff_open(w);
        w.file = f;
        if rotated {
            diag_prune(w.cur_name.as_deref());
        }
    }
    rotated
}

/// 写一个帧块（帧头 + payload 行，`block` 已含换行）到 aff_。`lines` 为块内行数（巡检计数用）。WRITER 临界区内只做文件 IO （锁序约定见 MAIN_WRITER 定义处）；
/// 写失败重开重试一次，仍失败丢块
fn aff_write_block(block: &str, lines: u64) {
    if !diag_active() {
        return;
    }
    let rotated = {
        let mut w = AFF_WRITER.lock().unwrap_or_else(|p| p.into_inner());
        if w.file.is_none() {
            let f = aff_open(&mut w);
            w.file = f;
        }
        let write_ok = match w.file.as_mut() {
            Some(f) => f.write_all(block.as_bytes()).is_ok(),
            None => false,
        };
        if !write_ok {
            // write_all 失败可能已落残块（不回滚）：弃用当前文件、换新时间戳文件重写整块，绝不把残块与重试块写进同一文件（防 @S 计数定界错位）
            w.file = None;
            w.cur_name = None;
            let f = aff_open(&mut w);
            w.file = f;
            if let Some(f) = w.file.as_mut() {
                let _ = f.write_all(block.as_bytes());
            }
        }
        w.since_check += lines;
        if w.since_check >= DEVIMP_CHECK_EVERY {
            w.since_check = 0;
            aff_check(&mut w)
        } else {
            false
        }
    };
    if rotated {
        // 触顶即新增一个 128MB 级文件：顺手执行目录预算清理（logd/ 与 devimp/ 各自独立计量，与 main_write_line 同口径）
        enforce_dir_limits(&common::get_module_root());
    }
    // 记账（WRITER 锁外，锁序约定）：devimp/ 目录预算与 128MB 归档门限共用
    note_write(&DEVIMP_BYTES_WRITTEN, "devimp/", block.len() as u64);
}

/// 帧字段净化：空白/控制字符（含换行）替换为 `_`，防字段值破坏单行帧边界（comm 可能含空格；reason 为末字段且调用方传短 token，一并净化无损）
pub(crate) fn aff_token(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_whitespace() || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

// [aff_api]
/// `@A` 动作帧：线程/内核节点写入动作 + 结果（`result` 形如 `ok` 或 `e{errno}`）
/// act 枚举见 [`AFF_HEADER`]；pid/tid 节点级动作填 0，pkg/comm 无主体填 "-"
/// 关闭诊断（diag_active=false）时零分配直接返回
#[allow(clippy::too_many_arguments)]
pub fn aff_action(
    act: &str,
    pid: i32,
    tid: i32,
    pkg: &str,
    comm: &str,
    dst: &str,
    value: &str,
    result: &str,
    reason: &str,
) {
    if !diag_active() {
        return;
    }
    let line = format!(
        "@A ts={} act={} pid={pid} tid={tid} pkg={} comm={} dst={} value={} result={} reason={}\n",
        filename_ts(),
        aff_token(act),
        aff_token(pkg),
        aff_token(comm),
        aff_token(dst),
        aff_token(value),
        aff_token(result),
        aff_token(reason),
    );
    aff_write_block(&line, 1);
}

/// `@S` 每秒快照帧：帧头计数由 `rows` 实际行反推（首字符 `p`/`t` 分别计入
/// ntop/nfg，帧头与行数恒等）；rows 为调用方拼好的 p/t 行（不含换行）
/// **`t` 行是槽级差分行**（见 `chiri::build_aff_snapshot` 文档）：只含本次变化的槽，
/// 未变槽 `-`、尾部未变槽省略；**整行全未变则不落行**（缺失 tid = 与上次落盘相同）
/// `full` 为刷新帧标记（首帧与每 30 帧一次，由调用方给）：帧头追加 ` full=1`，
/// 下游据此重建存活集合并全量合并该帧的 t 行
/// 帧头**每秒恒落**（即使本秒一行都没变化，`ntop=0 nfg=0`）：34 B/帧换「1 帧 = 1 秒」
/// 这个既有假设不破——帧号不再等于秒数会让离线轨迹与帧距判读全部漂移，不值那点字节。
/// 非 p/t 行不写入
pub fn aff_snapshot(rows: &[String], full: bool) {
    if !diag_active() {
        return;
    }
    let mut ntop = 0u64;
    let mut nfg = 0u64;
    for r in rows {
        match r.as_bytes().first() {
            Some(b'p') => ntop += 1,
            Some(b't') => nfg += 1,
            _ => {}
        }
    }
    let mut block = if full {
        format!("@S ts={} ntop={ntop} nfg={nfg} full=1\n", filename_ts())
    } else {
        format!("@S ts={} ntop={ntop} nfg={nfg}\n", filename_ts())
    };
    for r in rows {
        // 非 p/t 行（调用方错误）不写入：保持帧头计数与实际行数恒等
        if !matches!(r.as_bytes().first(), Some(b'p') | Some(b't')) {
            continue;
        }
        block.push_str(r);
        block.push('\n');
    }
    aff_write_block(&block, ntop + nfg + 1);
}

// 启动归档：logs/ → logd/<ts>.tar.lz4、devimp/ → logd/devimp_<ts>.tar.lz4
// （一次性子线程串行打包），打包完成后执行 logd/ 与 devimp/ 的**各自独立**预算清理
// 流程（由 main.rs 在 logger::init 之前调用，保证新旧日志文件分离）：
// 1. logs/ 与 devimp/ 分别原子 rename 为同级 `ziped_<ts>` / `ziped_devimp_<ts>`
// 临时目录（`ziped_` 是历史命名、遗留扫描按它认目录，保留不动）；logs/ 新建
// 空目录接住本进程新写入；devimp/ 不预建空目录（诊断写入路径惰性创建，
// dev_record 关闭时不得留痕，含空目录）；
// 2. **复制回 watchdog.pid** 到新建 logs/——WebUI stopScheduler 靠它终止看门狗；
// 3. 单个一次性子线程串行打包：外部脚本 scripts/pack.sh（对外稳定接口，构建
// 流程不得修改；优先模块自带 core/bin/tar，回退系统 tar）打**无压缩 tar**，
// 再用内置 lz4_flex 压成 `.tar.lz4` 并删中间 tar（不依赖设备 lz4 二进制；
// lz4 失败回落保留 .tar）；成功删临时目录，失败保留并写 ARCHIVE_FAILED.txt
// 留痕（此时 logger 尚未 init，无法打点）；
// 4. 打包完成后目录预算清理：logd/ 与 devimp/ 各自超过 LOGD_MAX_BYTES /
// DEVIMP_DIR_MAX_BYTES 时，只删本目录内最旧文件到低于对应 target；
// 5. rename 失败或原目录为空时跳过对应归档，不影响启动

// [archive]
/// logd/ 归档目录自身预算：超过后只清本目录内最旧归档（256MB，2026-09-24 由 128MB 扩容）
const LOGD_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// logd/ 清理目标：从最旧归档删到低于该值（200MB，滞回防每次归档都触发清理）
const LOGD_TARGET_BYTES: u64 = 200 * 1024 * 1024;
/// devimp/ 目录自身预算，与 logd/ **独立计量**（256MB）
const DEVIMP_DIR_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// devimp/ 清理目标（200MB）
const DEVIMP_DIR_TARGET_BYTES: u64 = 200 * 1024 * 1024;

/// 归档 staging 目录名前缀（logs/ → `ziped_<ts>`，devimp/ → `ziped_devimp_<ts>`）
const STAGING_PREFIX: &str = "ziped_";

/// 唯一化目标路径：`stem.ext` 已存在则依次试 `stem-1.ext` / `stem-2.ext` …
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let base = dir.join(format!("{stem}.{ext}"));
    if !base.exists() {
        return base;
    }
    for i in 1..100 {
        let p = dir.join(format!("{stem}-{i}.{ext}"));
        if !p.exists() {
            return p;
        }
    }
    base
}

/// 唯一化 staging 目录名（同目录 rename 目标必须不存在，否则 ENOTEMPTY 会让
/// **本次完全不归档**、日志滞留在模块根）
fn unique_staging(root: &Path, base: &str) -> PathBuf {
    let p = root.join(base);
    if !p.exists() {
        return p;
    }
    for i in 1..100 {
        let q = root.join(format!("{base}-{i}"));
        if !q.exists() {
            return q;
        }
    }
    p
}

/// 回收上一轮遗留的 staging 目录：进程在打包完成前退出（崩溃 / 日志门限`exit(0)` / 手动 kill）时，`ziped_*` 永久留在模块根——不在 logd/ 也不在devimp/，预算清理扫不到、
/// 后续启动也不再认领，日志永远进不了 logd在 rename 本轮目录**之前**扫描（避免把本轮刚建目录也算成遗留）
fn collect_staging_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(STAGING_PREFIX))
                    .unwrap_or(false)
        })
        .collect();
    out.sort();
    out
}

/// 短会话判定阈值：上一轮 daemon 启动距本次启动不足该秒数 → 丢弃其日志、不打包（防崩溃循环把 logd/ 塞满几 KB 空壳归档、淹没真正的崩溃现场）
const SHORT_SESSION_SECS: i64 = 30;

/// 本轮是否因「上一轮为短会话」而丢弃了日志（供 main 在 logger::init 之后打点，归档发生在 init 之前，当时无日志可打；不打点则日志凭空消失无从解释）
static SHORT_SESSION_DISCARDED: AtomicBool = AtomicBool::new(false);

/// 本轮是否丢弃了上一轮短会话日志（main 在 logger::init 后查询打点）
pub fn short_session_discarded() -> bool {
    SHORT_SESSION_DISCARDED.load(Ordering::Acquire)
}

/// 当前 epoch 秒（与 mktime 结果同基准）
fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 解析 daemon.log 行首的 `[YYYY-MM-DD HH:MM:SS]` → epoch 秒
/// **时区口径**：daemon.log 时间戳由 `libc::localtime_r` 生成的**本地时间**，
/// 必须用 `mktime`（本地时区）而非 `timegm`（UTC）解析，否则东八区偏 -8h、
/// 30s 短会话判定彻底失效`tm_isdst = -1` 交给 libc 按 DST 规则判定
#[cfg(unix)]
fn parse_log_ts_secs(line: &str) -> Option<i64> {
    let s = line.trim_start().strip_prefix('[')?;
    let ts = &s[..s.find(']')?];
    let (date, time) = ts.split_once(' ')?;
    let (y, rest) = date.split_once('-')?;
    let (mo, d) = rest.split_once('-')?;
    let (h, rest) = time.split_once(':')?;
    let (mi, se) = rest.split_once(':')?;
    let (y, mo, d, h, mi, se): (i32, i32, i32, i32, i32, i32) = (
        y.parse().ok()?,
        mo.parse().ok()?,
        d.parse().ok()?,
        h.parse().ok()?,
        mi.parse().ok()?,
        se.parse().ok()?,
    );
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = y - 1900;
        tm.tm_mon = mo - 1;
        tm.tm_mday = d;
        tm.tm_hour = h;
        tm.tm_min = mi;
        tm.tm_sec = se;
        tm.tm_isdst = -1;
        let t = libc::mktime(&mut tm);
        if t < 0 { None } else { Some(t) }
    }
}

#[cfg(not(unix))]
fn parse_log_ts_secs(_line: &str) -> Option<i64> {
    None
}

/// 读 `logs/daemon.log` 首行时间戳（epoch 秒）。daemon.log 单文件可达 50MB，
/// **只读前 256 字节**（首行必然完整），绝不整读
fn first_log_line_secs(path: &Path) -> Option<i64> {
    let mut f = fs::File::open(path).ok()?;
    let mut buf = [0u8; 256];
    let n = f.read(&mut buf).ok()?;
    if n == 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&buf[..n]);
    parse_log_ts_secs(text.lines().next()?)
}

/// 上一轮是否为短会话（存活 < SHORT_SESSION_SECS）。读不到/解析失败时返回 false —— 保守走正常归档，绝不因判定失败而丢日志
fn is_short_session(daemon_log: &Path) -> bool {
    let Some(start) = first_log_line_secs(daemon_log) else {
        return false;
    };
    // 时钟回拨（用户改时间/NTP 校正）时差值为负：不当作短会话，避免误删
    (0..SHORT_SESSION_SECS).contains(&now_epoch_secs().saturating_sub(start))
}

/// 清空目录内容（**保留 `keep` 里的文件名**，目录本身不删）
fn clear_dir_keep(dir: &Path, keep: &[&str]) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if keep.contains(&name) {
            continue;
        }
        if p.is_dir() {
            let _ = fs::remove_dir_all(&p);
        } else {
            let _ = fs::remove_file(&p);
        }
    }
}

/// 串行打包一批 staging 目录：每个目录打成 `logd/<去 ziped_ 前缀名>.tar.lz4`（沿用其原始时间戳），成功删目录、失败留痕（pack_or_keep）；空目录直接丢弃
fn pack_staging_dirs(root: &Path, logd: &Path, dirs: Vec<PathBuf>) {
    for dir in dirs {
        let mut files = Vec::new();
        if collect_files(&dir, &mut files).is_ok() && files.is_empty() {
            let _ = fs::remove_dir_all(&dir);
            continue;
        }
        let stem = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| STAGING_PREFIX.to_string());
        let stem = stem
            .strip_prefix(STAGING_PREFIX)
            .unwrap_or(&stem)
            .to_string();
        let tar_path = unique_path(logd, &stem, "tar.lz4");
        pack_or_keep(root, &dir, &tar_path);
    }
}

/// 启动归档入口。返回 (logs 归档 tar 名, devimp 归档 tar 名)（logger::init 后供 main info 打点）；未归档（首次安装 / 空目录 / rename 失败 / 短会话丢弃）
/// 对应项为 None
pub fn archive_on_startup(root: &Path) -> (Option<String>, Option<String>) {
    // 归档命名用本地时间 MMDD-HHmmss（人眼可辨）；同秒内两次启动会重名，由 unique_staging 补 -N 后缀（重名会让 rename 失败、本次整个不归档）
    let ts = filename_ts();

    // 先把上一轮遗留的 staging 目录收进本轮打包队列（必须在 rename 之前扫描）
    let orphans = collect_staging_dirs(root);

    let src_logs = root.join("logs");
    let src_devimp = root.join("devimp");

    // ── 短会话丢弃：上一轮存活不足 30s 直接清空两目录、不打包 ──判据取自 logs/daemon.log 首行（上一轮进程的首条日志 ≈ 其启动时刻，本地时间）。必须在 rename 之前判定：
    // rename 后 logs/ 已被换成空目录，就读不到上一轮日志了
    if is_short_session(&src_logs.join("daemon.log")) {
        // watchdog.pid 必须保留：看门狗先于 daemon 启动、WebUI stopScheduler 靠它终止看门狗，清掉会导致「关闭调度」失效（与归档路径同口径）
        clear_dir_keep(&src_logs, &["watchdog.pid"]);
        clear_dir_keep(&src_devimp, &[]);
        // devimp/ 清空后若已成空则连目录一并删：该目录整体属于 dev_record 产物，开关关闭时不得留痕（含空目录）。remove_dir 仅在目录存在且为空时成功，
        // 有残留/不存在时静默失败——开关开启时后续写入自会 create_dir_all 重建
        let _ = fs::remove_dir(&src_devimp);
        SHORT_SESSION_DISCARDED.store(true, Ordering::Release);
        // 遗留 staging 目录仍照常回收打包——它们属于更早的会话，不是本轮垃圾，且在崩溃循环里正是最有价值的那份现场
        if !orphans.is_empty() {
            let logd = root.join("logd");
            let _ = fs::create_dir_all(&logd);
            let root = root.to_path_buf();
            let _ = std::thread::Builder::new()
                .name("log_archiver".to_string())
                .spawn(move || {
                    pack_staging_dirs(&root, &logd, orphans);
                    enforce_dir_limits(&root);
                });
        }
        return (None, None);
    }

    // ── logs/：rename → 新建 logs/ → 复制回 watchdog.pid ──
    let mut logs_tmp: Option<PathBuf> = None;
    let logs_has_entries = fs::read_dir(&src_logs)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    if logs_has_entries {
        let tmp = unique_staging(root, &format!("ziped_{}", ts));
        if fs::rename(&src_logs, &tmp).is_ok() {
            let new_logs = root.join("logs");
            let _ = fs::create_dir_all(&new_logs);
            let pid_src = tmp.join("watchdog.pid");
            if pid_src.exists() {
                let _ = fs::copy(&pid_src, new_logs.join("watchdog.pid"));
            }
            logs_tmp = Some(tmp);
        }
    }

    // ── devimp/：整体 rename 归档（**不新建目录**——交由 diag 写入路径 main_open / aff_open 惰性 create_dir_all，开关关闭时不得留痕，含空目录）──
    let mut devimp_tmp: Option<PathBuf> = None;
    let devimp_has_entries = fs::read_dir(&src_devimp)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    if devimp_has_entries {
        let tmp = unique_staging(root, &format!("ziped_devimp_{}", ts));
        if fs::rename(&src_devimp, &tmp).is_ok() {
            devimp_tmp = Some(tmp);
        }
    }

    // ── 单个一次性子线程：串行打包（遗留优先）+ 预算清理 ──tar 名沿用各自 staging 目录名的时间戳段（保留原始时间戳），不再统一用本次 ts；`ziped_` 前缀是历史命名（遗留扫描按它认目录，
    // 保留不动），产物名剥掉它
    fn tar_name_of(dir: &Path) -> String {
        let stem = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| STAGING_PREFIX.to_string());
        let stem = stem
            .strip_prefix(STAGING_PREFIX)
            .unwrap_or(&stem)
            .to_string();
        format!("{stem}.tar.lz4")
    }
    let logs_tar = logs_tmp.as_deref().map(tar_name_of);
    let devimp_tar = devimp_tmp.as_deref().map(tar_name_of);
    if !orphans.is_empty() || logs_tmp.is_some() || devimp_tmp.is_some() {
        let logd = root.join("logd");
        let _ = fs::create_dir_all(&logd);
        let root = root.to_path_buf();
        let logs_tar_name = logs_tar.clone().unwrap_or_default();
        let devimp_tar_name = devimp_tar.clone().unwrap_or_default();
        let _ = std::thread::Builder::new()
            .name("log_archiver".to_string())
            .spawn(move || {
                // 遗留目录先打：本轮数据即便再次被打断，下次启动还能回收
                pack_staging_dirs(&root, &logd, orphans);
                if let Some(dir) = logs_tmp {
                    pack_or_keep(&root, &dir, &logd.join(&logs_tar_name));
                }
                if let Some(dir) = devimp_tmp {
                    pack_or_keep(&root, &dir, &logd.join(&devimp_tar_name));
                }
                // 两个归档均已落盘后才清点预算：此时总大小才包含新归档
                enforce_dir_limits(&root);
            });
    }

    (logs_tar, devimp_tar)
}

/// logd/ 与 devimp/ 目录预算清理（**各自独立计量**，各 256MB→清到 200MB）：任一目录超过自己的 MAX 时收到 TARGET 以下**清理语义不同**：logd/ 按「归档批次」
/// 原子删（`enforce_logd_limit`），devimp/ 按单文件 mtime 删（`enforce_dir_limit`，被删的活跃文件由写入器巡检自愈）调用时机：
/// 启动归档打包完成后 + devimp 触顶换文件时（运行中触发频率极低，全目录 metadata 扫描开销可忽略）
fn enforce_dir_limits(root: &Path) {
    enforce_logd_limit(&root.join("logd"));
    enforce_dir_limit(
        &root.join(DEVIMP_DIR_REL),
        DEVIMP_DIR_MAX_BYTES,
        DEVIMP_DIR_TARGET_BYTES,
    );
}

/// 两个诊断写入器的当前活跃文件名（两把 WRITER 锁顺序短取、不嵌套——锁序约定；仅在未持任何写入器锁的上下文调用）
fn active_diag_names() -> Vec<String> {
    let mut out = Vec::with_capacity(2);
    if let Ok(w) = MAIN_WRITER.lock() {
        if let Some(n) = &w.cur_name {
            out.push(n.clone());
        }
    }
    if let Ok(w) = AFF_WRITER.lock() {
        if let Some(n) = &w.cur_name {
            out.push(n.clone());
        }
    }
    out
}

/// devimp/ 单目录预算清理：`dir` 总大小超过 `max` 时按 mtime 从最旧文件逐个删除，直到低于 `target`
/// **活跃文件与最新的一个文件永不删除**——单个文件本身超过 target 时（如一个 100MB+ 的 aff_），删完其余文件后仍会留下它，避免目录被清空（清理目标不可达时以「至少留活跃+一份」为准）
/// logd/ 不走本函数（见 `enforce_logd_limit` 的批次原子语义）
fn enforce_dir_limit(dir: &Path, max: u64, target: u64) {
    let mut paths: Vec<PathBuf> = Vec::new();
    if collect_files(dir, &mut paths).is_err() {
        return;
    }
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    let mut total: u64 = 0;
    for p in paths {
        let Ok(meta) = fs::metadata(&p) else {
            continue;
        };
        let len = meta.len();
        total = total.saturating_add(len);
        let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        files.push((mtime, len, p));
    }
    if total <= max {
        return;
    }
    files.sort();
    // 保留两个写入器的活跃文件（main_/aff_ 并存后另一活跃文件未必是最新——它正在被写，删掉会让后续写入落进孤儿 inode 直到巡检自愈，期间丢数据）
    let active = active_diag_names();
    files.retain(|(_, _, p)| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map_or(true, |n| !active.iter().any(|a| a == n))
    });
    files.pop(); // 再保留最新一份
    for (_, len, path) in &files {
        if total < target {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(*len);
        }
    }
}

/// 归档批次键（`logd/` 预算清理的原子单位）：同一次启动归档产出的`<MMDD-HHmmss>.tar.lz4`（logs 侧）与 `devimp_<MMDD-HHmmss>.tar.lz4`（devimp 侧）
/// 共享 `<MMDD-HHmmss>` 一段；`unique_path` 去重的 `-N` 后缀剥掉。兼容 lz4 回落的历史 `<ts>.tar` 产物；形态不符的外来文件按整名成组（等价单文件批次），不会被误并组
fn logd_batch_key(name: &str) -> String {
    let is_ts = |s: &str| {
        let b = s.as_bytes();
        b.len() == 11
            && b[4] == b'-'
            && b.iter()
                .enumerate()
                .all(|(i, c)| i == 4 || c.is_ascii_digit())
    };
    let Some(stem) = name
        .strip_suffix(".tar.lz4")
        .or_else(|| name.strip_suffix(".tar"))
    else {
        return name.to_string();
    };
    let stem = stem.strip_prefix("devimp_").unwrap_or(stem);
    if is_ts(stem) {
        return stem.to_string();
    }
    if let Some((base, idx)) = stem.rsplit_once('-') {
        if is_ts(base) && !idx.is_empty() && idx.bytes().all(|c| c.is_ascii_digit()) {
            return base.to_string();
        }
    }
    name.to_string()
}

/// logd/ 预算清理：**按归档批次原子删**按单文件 mtime 删会把同批的 logs 侧
/// `<ts>.tar`（daemon.log / status.csv 唯一载体）与 `devimp_*.tar` 拆散，导出包
/// 出现 daemon.log/status.csv 凭空消失现语义：
/// 1. **批次原子**：`<ts>.tar` 与 `devimp_<ts>.tar` 同进退——批次是一次会话的
/// 完整记录，拆掉任何一半都不可再生；
/// 2. **最新批次永不删**：每次导出必含最近一次会话的 daemon.log/status.csv；
/// 3. **按批龄旧→新整批删**，删到 < `LOGD_TARGET_BYTES`；
/// 4. **目标不可达**（最新批次自身 ≥ target）时退化为「收到 MAX 即停」，
/// 预算内的旧批次不再为凑 target 而陪葬
fn enforce_logd_limit(dir: &Path) {
    let mut paths: Vec<PathBuf> = Vec::new();
    if collect_files(dir, &mut paths).is_err() {
        return;
    }
    // 批次键 → (组内最新 mtime, [(len, path)])
    let mut groups: std::collections::BTreeMap<
        String,
        (std::time::SystemTime, Vec<(u64, PathBuf)>),
    > = std::collections::BTreeMap::new();
    let mut total: u64 = 0;
    for p in paths {
        let Ok(meta) = fs::metadata(&p) else {
            continue;
        };
        let len = meta.len();
        total = total.saturating_add(len);
        let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        let key = p
            .file_name()
            .map(|n| logd_batch_key(&n.to_string_lossy()))
            .unwrap_or_default();
        let g = groups.entry(key).or_insert((mtime, Vec::new()));
        g.0 = g.0.max(mtime);
        g.1.push((len, p));
    }
    if total <= LOGD_MAX_BYTES {
        return;
    }
    // 最新批次 = 含最新 mtime 文件的那组（本轮 batch 的 devimp tar 最后落盘）
    let newest_key = groups
        .iter()
        .max_by_key(|(_, g)| g.0)
        .map(|(k, _)| k.clone());
    let floor = newest_key
        .as_ref()
        .and_then(|k| groups.get(k))
        .map(|g| g.1.iter().map(|(l, _)| *l).sum::<u64>())
        .unwrap_or(0);
    let budget = if floor >= LOGD_TARGET_BYTES {
        LOGD_MAX_BYTES
    } else {
        LOGD_TARGET_BYTES
    };
    let mut batches: Vec<(std::time::SystemTime, String, Vec<(u64, PathBuf)>)> =
        groups.into_iter().map(|(k, (m, v))| (m, k, v)).collect();
    batches.sort(); // 批龄旧→新（mtime 优先，键名兜底破平）
    for (_, key, members) in &batches {
        if Some(key) == newest_key.as_ref() {
            continue;
        }
        if total < budget {
            break;
        }
        for (len, p) in members {
            if fs::remove_file(p).is_ok() {
                total = total.saturating_sub(*len);
            }
        }
    }
}

/// 递归收集目录下全部普通文件（logs/ 实际为平铺结构，递归仅作防御）
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() {
            collect_files(&p, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

/// 调用外部打包脚本 `scripts/pack.sh`（对外暴露的稳定接口，构建流程不得修改）把 staging 目录打成**无压缩 tar**（优先模块自带 core/bin/tar，回退系统 tar；
/// lz4 压缩由调用方完成）成功判据：退出码 0 且目标文件存在；失败 → false，调用方保留 staging 并留痕
fn pack_dir_tar(root: &Path, dir: &Path, out_tar: &Path) -> bool {
    let script = root.join("scripts/pack.sh");
    let ok = std::process::Command::new("/system/bin/sh")
        .arg(&script)
        .arg("archive")
        .arg(dir)
        .arg(out_tar)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    ok && out_tar.exists()
}

/// Rust 内置流式压缩（flate2 gzip / lz4_flex，纯 Rust，全部 `io::copy` 不整读进内存——归档可达 100MB+）失败删除半截目标文件
fn compress_gzip(src: &Path, dst: &Path) -> bool {
    match (fs::File::open(src), fs::File::create(dst)) {
        (Ok(mut fin), Ok(fout)) => {
            let mut enc = flate2::write::GzEncoder::new(fout, flate2::Compression::new(6));
            let ok = std::io::copy(&mut fin, &mut enc)
                .and_then(|_| enc.finish().map(|_| ()))
                .is_ok();
            if ok {
                true
            } else {
                let _ = fs::remove_file(dst);
                false
            }
        }
        _ => false,
    }
}

fn compress_lz4(src: &Path, dst: &Path) -> bool {
    match (fs::File::open(src), fs::File::create(dst)) {
        (Ok(mut fin), Ok(fout)) => {
            let mut enc = lz4_flex::frame::FrameEncoder::new(fout);
            // finish(self) 返回 lz4_flex 自家的 frame::Error，不与 io::Error 同型，不能链进 io::Result，分开判
            let ok = match std::io::copy(&mut fin, &mut enc) {
                Ok(_) => enc.finish().is_ok(),
                Err(_) => false,
            };
            if ok {
                true
            } else {
                let _ = fs::remove_file(dst);
                false
            }
        }
        _ => false,
    }
}

/// 内置压缩工具入口（main.rs [toolbox] 分发）：`chiri gzip <file>` 生成 `<file>.gz` 并删除源文件（语义与 `gzip -f` 一致，供 pack.sh export 调用）；
/// `chiri lz4 <src> <dst>` 压缩到指定目标（备用入口，当前启动归档直接进程内调 `compress_lz4`，不走本入口）。返回进程退出码：0 成功、1 压缩失败、2 用法错误
pub fn compress_cli(cmd: &str, args: &[String]) -> i32 {
    match cmd {
        "gzip" => match args.first() {
            Some(s) if !s.is_empty() => {
                let src = PathBuf::from(s);
                let Some(dst) = src.to_str().map(|p| PathBuf::from(format!("{p}.gz"))) else {
                    return 2;
                };
                if compress_gzip(&src, &dst) {
                    let _ = fs::remove_file(&src);
                    0
                } else {
                    eprintln!("chiri gzip: compress {s} failed");
                    1
                }
            }
            _ => {
                eprintln!("usage: chiri gzip <file>");
                2
            }
        },
        "lz4" => match (args.first(), args.get(1)) {
            (Some(s), Some(d)) if !s.is_empty() && !d.is_empty() => {
                if compress_lz4(Path::new(s), Path::new(d)) {
                    0
                } else {
                    eprintln!("chiri lz4: compress {s} -> {d} failed");
                    1
                }
            }
            _ => {
                eprintln!("usage: chiri lz4 <src> <dst>");
                2
            }
        },
        _ => 2,
    }
}

/// 打包单个 staging 目录：pack.sh 先打无压缩 tar（目标名去掉 `.lz4`），再用内置 lz4_flex 压成 `out_final`（.tar.lz4）并删除中间 tar；
/// lz4 落盘失败（纯 I/O 错误，无环境依赖）回落保留 `.tar`。tar 打包失败写 ARCHIVE_FAILED.txt 保留待查（此时 logger 尚未 init，无法打点）
fn pack_or_keep(root: &Path, dir: &Path, out_final: &Path) {
    let Some(tar_str) = out_final.to_str().and_then(|s| s.strip_suffix(".lz4")) else {
        return;
    };
    let tar_path = PathBuf::from(tar_str);
    if pack_dir_tar(root, dir, &tar_path) {
        if compress_lz4(&tar_path, out_final) {
            let _ = fs::remove_file(&tar_path);
        }
        // lz4 失败：保留 .tar 作为最终产物（回落，见上）
        let _ = fs::remove_dir_all(dir);
    } else {
        let _ = fs::write(
            dir.join("ARCHIVE_FAILED.txt"),
            "tar packing failed; this directory was kept for inspection\n",
        );
    }
}
