//! utils.rs: [io] [temp_probe] [batt_temp] [sys_path] [fast_writer] [fast_reader] [misc]

use anyhow::Result;
use inotify::{Inotify, WatchMask};
use log;
use nix::unistd::{AccessFlags, access};
use serde::de::DeserializeOwned;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use crate::fluent_args;
use crate::i18n::t_with_args;

// [io]
/// 写文件：已存在则先置 0o664 可写，写完置 0o444 只读
pub fn write_to_file<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, content: C) -> Result<()> {
    let path = path.as_ref();

    // 尝试修改权限以便写入
    if path.exists() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o664));
    }

    fs::write(path, content)?;

    // 写完后设为只读
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o444));
    Ok(())
}

// 写文件，失败仅记日志不抛错
pub fn try_write_file<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, content: C) -> Result<()> {
    if let Err(e) = write_to_file(path.as_ref(), content) {
// ENOENT = 机型/内核没有该调优节点，非写入失败：降 debug，避免周期 apply 每轮刷 warn
        let not_found = e
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
        if not_found {
            log::debug!("write skipped (node missing): {}", path.as_ref().display());
        } else {
            log::warn!("Failed to write to {}: {}.", path.as_ref().display(), e);
        }
    }
    Ok(())
}

// [nodes]
/// 多节点写入的失效告警去重表：`what` 进入「全部节点不可用」态记一条，任一节点写成功即移除（重新武装），防 1~2s 热路径刷屏
static NODE_FAIL_WARNED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// 多节点写入：同一功能写到多个候选节点（不同机型可用节点不同——cgroup 组、cpufreq policy、`/sys/block` 设备等）日志口径：单节点失败 → debug（机型无该节点是常态）；
/// 全部节点失败 →warn，每个 `what` 仅进入失效态时一条（写成功即复位，见 [`NODE_FAIL_WARNED`]）；非「节点缺失」类错误（权限/IO）就地 warn返回成功写入的节点路径（空 = 全部失败）
/// 刻意不做存在性预判：写入失败才是「节点不可用」的权威证据路径/值按 `AsRef<str>` 接收，String/&str 元组皆可直传
pub fn write_nodes<P: AsRef<str>, V: AsRef<str>>(
    items: &[(P, V)],
    what: &'static str,
) -> Vec<String> {
    let mut written: Vec<String> = Vec::new();
    let mut last_err: Option<String> = None;
    for (path, value) in items {
        let path = path.as_ref();
        match write_to_file(path, value.as_ref()) {
            Ok(()) => written.push(path.to_string()),
            Err(e) => {
                if is_node_missing(&e) {
                    log::debug!("[{what}] node missing, skipped: {path}");
                } else {
                    log::warn!("[{what}] write failed: {path} — {e}");
                }
                last_err = Some(e.to_string());
            }
        }
    }
    if !written.is_empty() {
        clear_node_fail_warned(what);
    } else if let Some((first, _)) = items.first() {
        warn_all_nodes_failed(what, items.len(), first.as_ref(), last_err.as_deref());
    }
    written
}

/// 节点/目录不存在（ENOENT / ENOTDIR）：属「这台机型没有该节点」，按 debug 记
fn is_node_missing(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>().is_some_and(|io| {
        io.kind() == std::io::ErrorKind::NotFound || io.raw_os_error() == Some(20)
    })
}

fn warn_all_nodes_failed(what: &'static str, count: usize, first: &str, last_err: Option<&str>) {
    let mut warned = NODE_FAIL_WARNED.lock().unwrap_or_else(|e| e.into_inner());
    if warned.contains(&what) {
        return;
    }
    warned.push(what);
    log::warn!(
        "[{what}] all {count} node(s) unavailable (first: {first}, last error: {})",
        last_err.unwrap_or("unknown")
    );
}

fn clear_node_fail_warned(what: &'static str) {
    let mut warned = NODE_FAIL_WARNED.lock().unwrap_or_else(|e| e.into_inner());
    warned.retain(|k| *k != what);
}

pub fn enable_perm<P: AsRef<Path>>(path: P) -> Result<()> {
    let path = path.as_ref();
    if path.exists() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o664))?;
    }
    Ok(())
}

/// 目录内单个文件的变更监听（配置热重载用）两条硬约束：
/// 1. 跨重载复用同一 inotify 实例：每轮 init→drop 间隙无 watch 在册，其间事件被内核丢弃
/// （WebUI 保存是 tmp 写入 + mv 覆盖，真 MOVED_TO 恰落间隙即丢——表现为日志显示重载成功但级别仍旧值）；
/// 2. 按文件名过滤：目录级 watch 会收到目录内所有文件事件，不过滤会在 tmp 落盘时提前重载
pub struct DirWatcher {
    inotify: Inotify,
    buffer: [u8; 1024],
}

/// 命中事件后的静默合并窗口
const WATCH_SETTLE: Duration = Duration::from_millis(100);

impl DirWatcher {
    /// 监听 `dir` 目录（不递归）。目录不存在/无权限时返回 Err，由调用方退避重试
    pub fn new(dir: &Path) -> Result<Self> {
// CLOSE_WRITE 覆盖直接写入；MOVED_TO 覆盖原子替换（WebUI 用临时文件 + mv 保存，只有 MOVED_TO 能感知）
        Self::new_with_mask(dir, WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO)
    }

/// 自定义事件掩码仅当「删掉文件也算状态变更」时才需要更多事件：配置/实验室链路文件被删会自愈补建（仍走 CLOSE_WRITE），多给掩码反而会因误删触发重载
    pub fn new_with_mask(dir: &Path, mask: WatchMask) -> Result<Self> {
        let inotify = Inotify::init()?;
        inotify.watches().add(dir, mask)?;
        Ok(Self {
            inotify,
            buffer: [0u8; 1024],
        })
    }

    /// 阻塞等待 `file_name` 发生变更；目录内其它文件的事件一律忽略并继续等待
    pub fn wait_change(&mut self, file_name: &str) -> Result<()> {
        let target = std::ffi::OsStr::new(file_name);
        loop {
            let hit = {
                let events = self.inotify.read_events_blocking(&mut self.buffer)?;
                events
                    .into_iter()
                    .any(|ev| ev.name.as_deref() == Some(target))
            };
            if hit {
                break;
            }
        }
// 静默窗口 + 清空积压：连续写入合并为一次重载（fd 非阻塞，无事件时 WouldBlock 结束循环）
        thread::sleep(WATCH_SETTLE);
        while let Ok(events) = self.inotify.read_events(&mut self.buffer) {
            if events.peekable().peek().is_none() {
                break;
            }
        }
        Ok(())
    }
}

// 读取文件内容解析为 f64
pub fn read_f64_from_file(path: &str) -> Result<f64> {
    let mut content = String::new();
    File::open(path)?.read_to_string(&mut content)?;
    let val: f64 = content.trim().parse()?;
    Ok(val)
}

// 辅助函数：读取文件内容为 String
pub fn read_file_content(path: &str) -> Result<String> {
    let mut content = String::new();
    File::open(path)?.read_to_string(&mut content)?;
    Ok(content.trim().to_string())
}

// [temp_probe]
/// CPU 温度 zone type 内置默认名单（高通 soc_max/cpuss + MTK mtktscpu/cpu-1-/cpu-0-0-usr）feature.yaml `Thermal
/// cpu_temp_zone_types` 可覆盖（缺省一律用内置名单）soc_max = virtual-sensor 聚合温区（多传感器取 max，可能含非 CPU 传感器，作 cpu_temp 偏保守）；
/// cpuss = cluster 级 tsens 单传感器，soc_max 缺失时按子串回退命中唯一来源：chiri/config.rs 的 ThermalGuardConfig serde default 直接引用
pub(crate) fn default_cpu_temp_zone_types() -> Vec<String> {
    ["soc_max", "cpuss", "mtktscpu", "cpu-1-", "cpu-0-0-usr"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// 运行时生效的 zone 名单（Config::load 从 Thermal 配置同步；加载前/失败用内置默认）名单来自编译期嵌入的 feature.yaml，进程内恒定，set 仅首次生效、热重载重复调用无副作用
static CPU_TEMP_ZONE_TYPES: OnceLock<Vec<String>> = OnceLock::new();

/// utils 无法直接访问 chiri Config，走「加载时 set + 静态缓存」范式；OnceLock 首胜即上面的进程内恒定语义若未来名单改为磁盘可调，须换 RwLock/Mutex 存量真可重设
pub fn set_cpu_temp_zone_types(types: Vec<String>) {
    let _ = CPU_TEMP_ZONE_TYPES.set(types);
}

fn cpu_temp_zone_types() -> &'static [String] {
    static DEFAULT: OnceLock<Vec<String>> = OnceLock::new();
    CPU_TEMP_ZONE_TYPES
        .get()
        .unwrap_or_else(|| DEFAULT.get_or_init(default_cpu_temp_zone_types))
}

// 查找 CPU 温度传感器路径
pub fn find_cpu_temp_path() -> Result<String> {
    let thermal_path = "/sys/class/thermal";
    let thermal_dir = Path::new(thermal_path);

    if !thermal_dir.exists() {
        return Err(anyhow::anyhow!("Thermal directory not found"));
    }

// [名单序] 外层遍历名单项、内层 zones，首个命中项胜出，保证 soc_max 优先于 cpuss 回退（zones 少，双循环无性能顾虑）
    for item in cpu_temp_zone_types() {
        for entry in fs::read_dir(thermal_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(dir_name) = path.file_name().and_then(|s| s.to_str()) {
                if !dir_name.starts_with("thermal_zone") {
                    continue;
                }
                let type_path = path.join("type");
                if let Ok(type_content) =
                    read_file_content(type_path.to_str().unwrap_or_default())
                {
                    if type_content.contains(item.as_str()) {
                        let temp_path = path.join("temp");
                        if temp_path.exists() {
                            return Ok(temp_path.to_str().unwrap().to_string());
                        }
                    }
                }
            }
        }
    }
    Err(anyhow::anyhow!("Valid CPU thermal zone not found"))
}

/// 电池温度节点路径原始刻度因内核/厂商而异不可硬编码换算，读取前先经 `battery_temp_scale()` 预识别
/// None = 节点不存在（依赖它的功能自动失效，不影响其余功能）
pub fn find_battery_temp_path() -> Option<&'static str> {
    const BATT_TEMP: &str = "/sys/class/power_supply/battery/temp";
    std::path::Path::new(BATT_TEMP)
        .exists()
        .then_some(BATT_TEMP)
}

// 电池温度刻度预识别 `/sys/class/power_supply/battery/temp` 的单位因内核/厂商而异（0.1°C / 毫摄氏度 / 直读 °C 三种），硬编码除数会有 10×/100× 偏差，
// 故运行时预识别一次并缓存，热保护与温度护栏共用同一结论，避免口径漂移

// [batt_temp]
/// 电池温度节点的原始刻度
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BatteryTempScale {
    /// 0.1°C（内核 power_supply 标准）：raw 400 = 40.0°C
    TenthsCelsius,
    /// 毫摄氏度：raw 40000 = 40.0°C
    MilliCelsius,
    /// 摄氏度（少数厂商直读）：raw 40 = 40.0°C
    Celsius,
}

impl BatteryTempScale {
    /// 原始读数换算为 °C 的除数
    pub fn divisor(self) -> f64 {
        match self {
            BatteryTempScale::TenthsCelsius => 10.0,
            BatteryTempScale::MilliCelsius => 1000.0,
            BatteryTempScale::Celsius => 1.0,
        }
    }

    /// 日志用刻度名
    pub fn label(self) -> &'static str {
        match self {
            BatteryTempScale::TenthsCelsius => "0.1C",
            BatteryTempScale::MilliCelsius => "milliC",
            BatteryTempScale::Celsius => "C",
        }
    }
}

/// 合理电池温度窗口（°C）：在多种单位解释里挑唯一合理者下界 5（取 0 会令 tenths 解释失去区分度）、上界 80（容纳偶发高温，离谱值由热保护 TempFilter 物理范围门兜底）；
/// 窗口下 tenths 与 milli 解释不互相碰撞
const BATT_TEMP_PLAUSIBLE_MIN_C: f64 = 5.0;
const BATT_TEMP_PLAUSIBLE_MAX_C: f64 = 80.0;

/// 预识别结果缓存：仅在探测得出确定结论时写入，未就绪/读数异常留空待下次重试（避免固化开机早期占位值）
static BATTERY_TEMP_SCALE: OnceLock<BatteryTempScale> = OnceLock::new();

/// 读一次电池温度原始值（不换算）
pub fn read_battery_temp_raw() -> Option<f64> {
    std::fs::read_to_string(find_battery_temp_path()?)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
}

/// 预识别刻度：读一次原始值，按「唯一落进合理窗口」定档，多个解释同时合理时按内核标准优先（tenths > milli > Celsius）；读数 0/节点缺失返回 None，
/// 均不合理时回退 tenths（离谱值由 TempFilter 兜底）
pub fn detect_battery_temp_scale() -> Option<BatteryTempScale> {
    let raw = read_battery_temp_raw()?;
    if raw == 0.0 {
        return None;
    }
    [
        BatteryTempScale::TenthsCelsius,
        BatteryTempScale::MilliCelsius,
        BatteryTempScale::Celsius,
    ]
    .into_iter()
    .find(|s| {
        let c = raw / s.divisor();
        (BATT_TEMP_PLAUSIBLE_MIN_C..=BATT_TEMP_PLAUSIBLE_MAX_C).contains(&c)
    })
    .or(Some(BatteryTempScale::TenthsCelsius))
}

/// 取（必要时预识别并缓存）电池温度刻度；None = 无法确定（调用方退化为仅 CPU 温度）
pub fn battery_temp_scale() -> Option<BatteryTempScale> {
    if let Some(s) = BATTERY_TEMP_SCALE.get() {
        return Some(*s);
    }
    let detected = detect_battery_temp_scale()?;
    // 竞态下可能已被其它线程写入（值相同，忽略结果）
    let _ = BATTERY_TEMP_SCALE.set(detected);
    BATTERY_TEMP_SCALE.get().copied()
}

/// 电池温度原始读数的换算除数（供持有 (路径, 除数) 的调用方复用同一结论）
pub fn battery_temp_divisor() -> Option<f64> {
    battery_temp_scale().map(|s| s.divisor())
}

/// 读电池温度并按预识别刻度换算为 °C；节点缺失/读数异常返回 None
pub fn read_battery_temp_celsius() -> Option<f32> {
    let scale = battery_temp_scale()?;
    Some((read_battery_temp_raw()? / scale.divisor()) as f32)
}

// [sys_path]
pub struct SysPathExist {
    pub qcom_feas_exist: bool,
    pub mtk_feas_exist: bool,
    pub walt_exist: bool,
    pub stune_exist: bool,
    pub hi6220_ufs_exist: bool,
    pub cpuctl_top_app_exist: bool,
    pub cpuctl_foreground_exist: bool,
    pub cpuctl_background_exist: bool,
    pub cpuset_top_app_exist: bool,
    pub cpuset_foreground_exist: bool,
    pub cpuset_background_exist: bool,
    pub cpuset_system_background_exist: bool,
    pub cpuset_restricted_exist: bool,
    pub cpuset_root_exist: bool,
    pub cpuidle_governor_exist: bool,
    pub sda_scheduler_exist: bool,
}

impl SysPathExist {
    pub fn new() -> Self {
        Self {
            qcom_feas_exist: Self::path_exists("/sys/module/perfmgr/parameters/perfmgr_enable"),
            mtk_feas_exist: Self::path_exists("/sys/module/mtk_fpsgo/parameters/perfmgr_enable"),
            walt_exist: Self::path_exists("/proc/sys/walt"),
            stune_exist: Self::path_exists("/dev/stune"),
            hi6220_ufs_exist: Self::path_exists(
                "/sys/bus/platform/devices/hi6220-ufs/ufs_clk_gate_disable",
            ),
            cpuctl_top_app_exist: Self::path_exists("/dev/cpuctl/top-app"),
            cpuctl_foreground_exist: Self::path_exists("/dev/cpuctl/foreground"),
            cpuctl_background_exist: Self::path_exists("/dev/cpuctl/background"),
            cpuset_top_app_exist: Self::path_exists("/dev/cpuset/top-app"),
            cpuset_foreground_exist: Self::path_exists("/dev/cpuset/foreground"),
            cpuset_background_exist: Self::path_exists("/dev/cpuset/background"),
            cpuset_system_background_exist: Self::path_exists("/dev/cpuset/system-background"),
            cpuset_restricted_exist: Self::path_exists("/dev/cpuset/restricted"),
            cpuset_root_exist: Self::path_exists("/dev/cpuset"),
            cpuidle_governor_exist: Self::path_exists(
                "/sys/devices/system/cpu/cpuidle/current_governor",
            ),
            sda_scheduler_exist: Self::path_exists("/sys/block/sda/queue/scheduler"),
        }
    }

    fn path_exists(path: &str) -> bool {
        access(path, AccessFlags::F_OK).is_ok()
    }
}

// [fast_writer]
// FastWriter — 带去重 + unmount 的 sysfs 写入器

pub struct FastWriter {
    file: Option<File>,
    buf: [u8; 20],
    path: PathBuf,
    /// 本次生命周期内是否已尝试过 umount（最多一次，避免反复 detach 合法挂载）
    unmounted: bool,
}

impl FastWriter {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let path_ref = path.as_ref();
        let _ = enable_perm(path_ref);
        let mut w = Self {
            file: Self::open_file(path_ref),
            buf: [0u8; 20],
            path: path_ref.to_path_buf(),
            unmounted: false,
        };
// 惰性卸载：仅当直接打开失败（挂载写保护/权限封装等异常态）时 umount 重开，避免对正常节点无条件detach（可能拆掉合法挂载）。写被拒（EACCES/EROFS）时也会走一次卸载重试（见 do_write）
        if w.file.is_none() {
            w.unmount_and_reopen();
        }
        w
    }

    fn open_file(path: &Path) -> Option<File> {
        OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| {
                log::error!(
                    "{}",
                    t_with_args(
                        "sysfs-open-failed",
                        &fluent_args!("path" => path.display().to_string(), "error" => e.to_string())
                    )
                )
            })
            .ok()
    }

    fn unmount_and_reopen(&mut self) {
        Self::try_unmount(&self.path);
        self.unmounted = true;
        // 卸载后强制重新打开：旧 fd 可能仍指向被卸载的挂载背衬，需换到真实节点
        self.file = Self::open_file(&self.path);
    }

    fn try_unmount(path: &Path) {
        if let Some(path_str) = path.to_str() {
            if let Ok(cpath) = std::ffi::CString::new(path_str) {
                let ret = unsafe { libc::umount2(cpath.as_ptr(), libc::MNT_DETACH) };
                if ret != 0 {
                    let errno = std::io::Error::last_os_error();
                    if errno.raw_os_error() != Some(libc::EINVAL)
                        && errno.raw_os_error() != Some(libc::ENOENT)
                    {
                        log::debug!(
                            "{}",
                            t_with_args(
                                "sysfs-umount2-failed",
                                &fluent_args!("path" => path_str, "error" => errno.to_string())
                            )
                        );
                    }
                }
            }
        }
    }

    pub fn re_unmount(&self) {
        Self::try_unmount(&self.path);
    }

    pub fn write_value_force(&mut self, value: u32) -> bool {
        self.do_write(value)
    }

    pub fn is_valid(&self) -> bool {
        self.file.is_some()
    }

    fn do_write(&mut self, value: u32) -> bool {
        let len = Self::u32_to_buf(value, &mut self.buf);
        let write_result = if let Some(file) = &mut self.file {
            let _ = file.seek(SeekFrom::Start(0));
            file.write_all(&self.buf[..len])
        } else {
            return false;
        };
        match write_result {
            Ok(()) => true,
            Err(e) => {
                match e.raw_os_error() {
// EINVAL(22): 内核拒绝该频率（热限频/范围收窄）；EBUSY(16): 节点短暂被占用均为预期内瞬态错误：降级 debug 且不缓存，下次 tick 自动重试
                    Some(libc::EINVAL) | Some(libc::EBUSY) => {
                        log::debug!("write freq {} to {:?} skipped: {}", value, self.path, e);
                    }
                    // EACCES/EROFS: 多为挂载写保护/权限封装（异常态），卸载后重试一次
                    Some(libc::EACCES) | Some(libc::EROFS) if !self.unmounted => {
                        log::warn!(
                            "{}",
                            t_with_args(
                                "sysfs-write-freq-failed",
                                &fluent_args!("freq" => value.to_string(), "error" => e.to_string())
                            )
                        );
                        self.unmount_and_reopen();
                        return self.do_write(value);
                    }
                    _ => {
                        log::warn!(
                            "{}",
                            t_with_args(
                                "sysfs-write-freq-failed",
                                &fluent_args!("freq" => value.to_string(), "error" => e.to_string())
                            )
                        );
                    }
                }
// 写入失败返回 false，不更新任何缓存值，下次 tick 由调用方重试
                false
            }
        }
    }

    fn u32_to_buf(mut v: u32, buf: &mut [u8; 20]) -> usize {
        if v == 0 {
            buf[0] = b'0';
            buf[1] = b'\n';
            return 2;
        }
        let mut pos = 18;
        while v > 0 {
            buf[pos] = b'0' + (v % 10) as u8;
            v /= 10;
            pos -= 1;
        }
        let start = pos + 1;
        let digit_len = 19 - start;
        buf.copy_within(start..19, 0);
        buf[digit_len] = b'\n';
        digit_len + 1
    }
}

// [fast_reader]
// FastReader — keep-open + 可复用 buf 的稳定节点读取器（镜像 FastWriter 做法）

/// 最近一次读取的三态：需区分「节点缺失」与「读失败」时查 [`FastReader::state`]；「正常空值」以 Some("") 表达（对应 fs::read_to_string 的 Ok("")）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReadState {
    /// 读取成功（内容可能为空串）
    Ok,
    /// 节点不存在（ENOENT/ENOTDIR）：机型没有该节点，属合法缺失
    Missing,
    /// 其他读取失败（权限/IO/非法 UTF-8）：多为瞬态，下次调用自动重开重试
    Failed,
}

/// 稳定 sysfs/procfs 节点的 keep-open 读取器：fd 常驻 + seek(0) 重读入可复用 buf，消除每 tick 分配与 open-close只收常驻节点；
/// per-pid `/proc/<pid>/*` 随进程退出失效，保持每次 open读取三态与`fs::read_to_string` 链对齐（None = 读失败/缺失、Some("") = 正常空值）；
/// 读错误/EOF 重开重试一次
pub struct FastReader {
    file: Option<File>,
    /// 读入复用 buf：每次 seek(0) 后从头覆盖写入，跨调用不重新分配
    buf: Vec<u8>,
    /// 节点路径（构造期缓存，调用点免再 format! 拼路径）
    path: PathBuf,
    state: ReadState,
}

impl FastReader {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let mut r = Self {
            file: None,
            buf: Vec::new(),
            path: path.as_ref().to_path_buf(),
            state: ReadState::Missing,
        };
        r.reopen();
        r
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 最近一次读取的三态结果（read_raw/read_u32 返回 None 时区分缺失与失败）
    // 接口面：本轮落点未全部需要三态区分，保留查询能力
    #[allow(dead_code)]
    pub fn state(&self) -> ReadState {
        self.state
    }

    /// 打开 fd；失败按 errno 记 Missing/Failed（不告警：机型没有该节点是常态）
    fn reopen(&mut self) -> bool {
        match File::open(&self.path) {
            Ok(f) => {
                self.file = Some(f);
                true
            }
            Err(e) => {
                self.state = Self::err_state(&e);
                false
            }
        }
    }

    /// errno → 三态（口径同 utils 节点缺失判定：ENOTDIR(20) 与 ENOENT 同计缺失）
    fn err_state(e: &std::io::Error) -> ReadState {
        if e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(20) {
            ReadState::Missing
        } else {
            ReadState::Failed
        }
    }

    /// 读入内部 buf（seek(0) + read_to_end）；纯 IO，UTF-8 校验在 read_raw
    fn read_into_buf(&mut self) -> std::io::Result<()> {
        self.buf.clear();
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::from_raw_os_error(libc::ENOENT))?;
        file.seek(SeekFrom::Start(0))?;
        file.read_to_end(&mut self.buf)?;
        Ok(())
    }

    /// 原文读取（内容比较型调用点用，不归一化）：`Some(&str)` = 读到内容（空为 `Some("")`）、`None` = 读失败/缺失，与 `fs::read_to_string(..).ok()
    /// ` 同口径（非法 UTF-8 计失败）；借用内部 buf，下次 read_* 即失效
    pub fn read_raw(&mut self) -> Option<&str> {
        // 构造期未就绪的节点在这里惰性补打开
        if self.file.is_none() && !self.reopen() {
            return None;
        }
        let mut res = self.read_into_buf();
        if res.is_err() {
            // 读错误/EOF 异常：丢弃 fd 重开重试一次（umount/热插拔/节点重建后旧 fd 会持续报错）
            self.file = None;
            if !self.reopen() {
                return None;
            }
            res = self.read_into_buf();
        }
        match res {
            Ok(()) => match std::str::from_utf8(&self.buf) {
                Ok(s) => {
                    self.state = ReadState::Ok;
                    Some(s)
                }
                Err(_) => {
                    self.state = ReadState::Failed;
                    self.file = None;
                    None
                }
            },
            Err(e) => {
                self.state = Self::err_state(&e);
                self.file = None;
                None
            }
        }
    }

    /// 读 u32（trim + parse，不经 String 分配）：`None` = 读失败/节点缺失/内容非 u32，与`read_to_string(..)?.trim().parse().ok()
    /// ` 等价（parse 失败时 `state()` 仍为 Ok，可与读失败区分）
    // 接口面：本轮落点无整型读，后续 tick 读侧接入时启用
    #[allow(dead_code)]
    pub fn read_u32(&mut self) -> Option<u32> {
        self.read_raw().and_then(|s| s.trim().parse().ok())
    }
}

// [misc]
// 通用跨模块工具函数
/// Serde 默认值辅助函数：始终返回 true
pub fn default_true() -> bool {
    true
}

/// Serde 默认值辅助函数：耗电读数满量程（W）。meta.yaml 的 power_max_w 缺省/非法时用此值（WebUI 仪表盘据此换算进度）
pub fn default_power_max_w() -> f32 {
    12.0
}

/// 电池读数单位校准默认值（标准 Android ABI：µV / µA）：节点原始值 ÷ 该值 = V / A，读取层不做换算，batt_power_w 按安培 × 伏特得瓦OPlus 私有节点（bcc_parms）
/// 报 mV / mA 需填 1000，安装脚本检测到时自动写入
pub const DEFAULT_UNIT_DIVISOR: f32 = 1_000_000.0;

/// Serde 默认值辅助函数：单位校准除数（meta.yaml 的 `unit_divisor` 缺省即 [`DEFAULT_UNIT_DIVISOR`]）
pub fn default_unit_divisor() -> f32 {
    DEFAULT_UNIT_DIVISOR
}

/// 读取文件内容并解析为 i32
pub fn read_i32_from_file(path: &str) -> Result<i32> {
    let mut content = String::new();
    File::open(path)?.read_to_string(&mut content)?;
    Ok(content.trim().parse()?)
}

/// 获取与 BPF ktime_get_ns() 绝对对齐的单调时钟时间 (纳秒)
pub fn get_ktime_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
}

/// Serde 反序列化辅助：从 YAML 文件读取配置，解析失败时返回 Default
pub fn read_config<T, P>(path: P) -> Result<T>
where
    T: DeserializeOwned + Default,
    P: AsRef<Path>,
{
    let path_ref = path.as_ref();
    match File::open(path_ref) {
        Ok(mut file) => {
            let mut s = String::new();
            file.read_to_string(&mut s)?;
            serde_yaml::from_str(&s).or_else(|e| {
                log::warn!(
                    "[Config] Parse error {}: {}. Default.",
                    path_ref.display(),
                    e
                );
                Ok(T::default())
            })
        }
        Err(_) => {
            log::warn!("[Config] Not found: {}. Default.", path_ref.display());
            Ok(T::default())
        }
    }
}
