//! screen_detect.rs: [update] [prop] [verify] [uevent]（[source]/[select]/[read] 已 [PAUSED]）

use kobject_uevent::UEvent;
use log::{debug, info, trace, warn};
use netlink_sys::{Socket, SocketAddr, protocols::NETLINK_KOBJECT_UEVENT};
use std::error::Error;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::common::DaemonEvent;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// [update]
/// 更新共享屏幕状态，返回是否发生状态变化变化转发：uevent 线程直推 DaemonEvent::ScreenStateChange （零轮询延迟），verify 自愈路径的变化由 app_detect 主循环兜底转发
/// [PAUSED] 原「亮→息翻转先全节点投票（多数票）再确认」的仲裁已暂停，现役判定源是 debug.tracing.screen_state 属性（见 [prop]）；
/// 恢复投票 = 解开 [source]/[select]/[read] 区块并还原本函数投票块与 VETO_WARNED
/// 两次状态翻转之间的最小间隔：任何来源（属性轮询 / verify 自愈 / 未来的 uevent）在此时长内提出的
/// 第二次翻转一律**延后**（不是丢弃——下一次轮询/verify 会再次尝试）。人手按电源键不可能快于 200ms，
/// 故真实操作不受影响，但能吃掉「同一秒内息屏→亮屏」这类抖动（每次抖动都要走一整轮 doze 切换 +
/// CLG 重建）。2026-09-28 加：属性轮询线程的去抖挡不住 verify 路径（它每秒独立采样并直写共享状态）
const SCREEN_MIN_FLIP_INTERVAL: Duration = Duration::from_millis(200);
static LAST_FLIP_AT: Mutex<Option<Instant>> = Mutex::new(None);

fn update_state_if_changed(state_arc: &Arc<Mutex<bool>>, new_state: bool, source: &str) -> bool {
    let mut state_lock = state_arc.lock().unwrap();
    if *state_lock == new_state {
        return false;
    }
    // 锁序：state_arc → LAST_FLIP_AT（全仓只有此处同时持两把，无反向嵌套）
    {
        let mut last = LAST_FLIP_AT.lock().unwrap_or_else(|e| e.into_inner());
        if last.map(|t| t.elapsed() < SCREEN_MIN_FLIP_INTERVAL).unwrap_or(false) {
            return false;
        }
        *last = Some(Instant::now());
    }
    debug!(
        "{}",
        t_with_args(
            "screen-state-detect-detail",
            &fluent_args!(
                "source" => source,
                "old" => state_lock.to_string(),
                "new" => new_state.to_string()
            )
        )
    );
    debug!(
        "{}",
        t_with_args(
            "screen-state-change-detected",
            &fluent_args!("source" => source)
        )
    );
    *state_lock = new_state;
    let state_str = if new_state { "ON" } else { "OFF" };
    debug!(
        "{}",
        t_with_args(
            "screen-state-changed-value",
            &fluent_args!("state" => state_str)
        )
    );
    true
}

/* [PAUSED] sysfs 节点投票检测整体暂停：亮灭屏判定改用 debug.tracing.screen_state 属性（见 [prop]）
   恢复 = 移除本块注释开头标记与 [read] 末尾配对结束标记，再还原 [update]/[uevent] 的 [PAUSED] 投票块与屏幕分支
   // [source]
   /// 屏幕状态检测源类别：不同机型暴露的屏幕状态节点不同（QCOM/通用走 backlight class、MTK 走 leds、
   /// 老内核 fbdev blank、三星/海思部分仅 LCD class lcd_power），按可靠性优先级依次探测，找到第一个可用源即锁定缓存
   #[derive(Clone, Copy, PartialEq, Eq)]
   enum ScreenSourceKind {
   /// backlight class：bl_power/actual_brightness/brightness 判定
   Backlight,
   /// leds class（名字含 backlight 的节点）：Android 息屏背光写 0，brightness > 0 即面板在发光
   Leds,
   /// fbdev blank：0 = unblank（亮），非 0 = 灭
   FbBlank,
   /// LCD class lcd_power：FB_BLANK 口径，0 = UNBLANK（亮）、4 = POWERDOWN（灭）
   LcdPower,
   /// DRM 内屏 connector 的 `enabled`："enabled" = 亮、"disabled" = 灭（内核 DPMS 口径）；外接 HDMI/DP 不计
   DrmEnabled,
   /// DRM connector 的 `dpms`（`enabled` 缺失时的老内核兜底）："On" = 亮，"Off"/"Standby"/"Suspend" = 灭
   DrmDpms,
   }
   impl ScreenSourceKind {
   fn as_str(self) -> &'static str {
   match self {
   ScreenSourceKind::Backlight => "backlight",
   ScreenSourceKind::Leds => "leds",
   ScreenSourceKind::FbBlank => "fb-blank",
   ScreenSourceKind::LcdPower => "lcd-power",
   ScreenSourceKind::DrmEnabled => "drm-enabled",
   ScreenSourceKind::DrmDpms => "drm-dpms",
   }
   }
   }
   /// leds class 背光节点名关键字：厂商命名差异极大（MTK/海思 `lcd-backlight`、高通 `panel-backlight`、
   /// Awinic `aw22xxx-backlight`、展锐 `sprd-backlight`、wled/`disp-backlight 等），只认命中关键字的亮度节点
   const BACKLIGHT_LED_KEYWORDS: [&str; 5] = ["backlight", "lcd", "panel", "wled", "disp"];
   /// 非面板 LED 关键字豁免：键盘背光/按键灯/充电/通知灯/闪光灯/RGB 灯即便含上表关键字也不是屏幕背光，
   /// 这类灯随充电/通知亮灭，计入息屏仲裁会持续投亮屏票（永远进不了息屏）
   const NON_PANEL_LED_KEYWORDS: [&str; 14] = [
   "keyboard",
   "keypad",
   "button",
   "keys",
   "charging",
   "charge",
   "battery",
   "notification",
   "notify",
   "torch",
   "flash",
   "indicator",
   "breath",
   "rgb",
   ];
   /// 判断 leds 节点名是否屏幕背光：命中背光关键字且不命中非面板关键字枚举与 uevent 两路共用
   /// （uevent 收到通知灯/按键灯事件时不按背光处理）
   fn is_backlight_led_name(name: &str) -> bool {
   let n = name.to_lowercase();
   if !BACKLIGHT_LED_KEYWORDS.iter().any(|k| n.contains(k)) {
   return false;
   }
   !NON_PANEL_LED_KEYWORDS.iter().any(|k| n.contains(k))
   }
   /// 诊断打点去重：检测源发现 / 无源告警各只打一条（verify 高频调用防刷屏）
   static SOURCE_FOUND_LOGGED: AtomicBool = AtomicBool::new(false);
   static NO_SOURCE_LOGGED: AtomicBool = AtomicBool::new(false);
   /// 亮屏否决告警去重：一次被否决的息屏 episode 只 warn 一条，ON 读数到达后复位（防每轮 verify 刷屏）
   static VETO_WARNED: AtomicBool = AtomicBool::new(false);
   // [select]
   /// 候选节点清单缓存：候选由内核/驱动启动期创建、拓扑运行期基本不变，而自愈校验每轮全量枚举
   /// （最多 5 次 read_dir + 每候选 1~3 次 Path::exists()），故缓存清单而非屏幕状态——每轮仍逐节点
   /// 新鲜读，投票口径不变两条刷新路径：1) TTL 到期（SCREEN_NODES_TTL，10 分钟）：开机早期残缺
   /// 清单最迟 10 分钟补全；2) 全不可读即失效（invalidate_screen_nodes）：可能权限变化/模块加载
   /// 枚举为空不写缓存（无源机型每轮重试）；TTL 判定惰性，只在取清单调用点顺带判，无独立刷新线程
   static SCREEN_NODES: LazyLock<Mutex<Option<(Instant, Arc<Vec<(PathBuf, ScreenSourceKind)>>)>>> =
   LazyLock::new(|| Mutex::new(None));
   /// 节点清单缓存 TTL = 10 分钟：拓扑变化集中在开机早期（驱动加载/模块开关），足够稀疏又能及时补全残缺清单
   const SCREEN_NODES_TTL: Duration = Duration::from_secs(600);
   /// 取候选节点清单（TTL 内命中缓存只做一次引用计数递增 + 一次 `Instant` 时钟读取，零分配零 syscall）
   /// 返回 `Arc` 快照并立刻释放缓存锁：投票要逐个读节点文件，持锁会把 uevent 线程的翻转投票堵在锁上
   /// TTL 到期当作未命中：重新枚举并刷新创建时刻（判定就在本调用点顺带做，无独立刷新线程）
   fn screen_nodes() -> Arc<Vec<(PathBuf, ScreenSourceKind)>> {
   {
   let guard = SCREEN_NODES.lock().unwrap();
   if let Some((created, nodes)) = guard.as_ref() {
   if created.elapsed() <= SCREEN_NODES_TTL {
   return Arc::clone(nodes);
   }
   // TTL 到期：落到底下重新枚举（本块末尾释放锁，枚举不持锁）
   }
   }
   let nodes = Arc::new(enumerate_screen_nodes());
   if !nodes.is_empty() {
   SCREEN_NODES.lock().unwrap() = Some((Instant::now(), Arc::clone(&nodes)));
   }
   nodes
   }
   /// 丢弃节点清单缓存，下一轮重新枚举（投票一个有效读数都没有时调用；另一刷新路径是 TTL 到期）
   fn invalidate_screen_nodes() {
   SCREEN_NODES.lock().unwrap() = None;
   }
   /// 按可靠性优先级枚举全部候选屏幕状态节点（有序）：
   /// 1. `/sys/class/backlight`：具备 bl_power/actual_brightness/brightness 的设备（QCOM/通用内核）；
   /// 2. `/sys/class/leds` 背光节点（MTK lcd-backlight、panel/wled/aw22xxx/sprd-backlight 等）——backlight
   ///    缺失机型的主要修复路径（非面板 LED 不计，见 is_backlight_led_name）；
   /// 3. `/sys/class/graphics/fb<编号>/blank`（fbdev 旧接口兜底，fb0/fb1/… 全部计入）；
   /// 4. `/sys/class/lcd/<设备>/lcd_power`（LCD class，三星 Exynos `panel/lcd_power` 等唯一可用节点，FB_BLANK 口径）；
   /// 5. `/sys/class/drm` 内屏 connector（dsi/edp/lvds）的 `enabled` 节点，缺 enabled 退 dpms——backlight/leds 全缺机型的兜底
   fn enumerate_screen_nodes() -> Vec<(PathBuf, ScreenSourceKind)> {
   let mut nodes = Vec::new();
   if let Ok(entries) = fs::read_dir("/sys/class/backlight") {
   for entry in entries.flatten() {
   let dev = entry.path();
   // brightness 兜底：部分驱动未实现 get_brightness（actual_brightness 读不到）、也无 bl_power，只能看驱动存的亮度值
   if dev.join("bl_power").exists()
   || dev.join("actual_brightness").exists()
   || dev.join("brightness").exists()
   {
   nodes.push((dev, ScreenSourceKind::Backlight));
   }
   }
   }
   if let Ok(entries) = fs::read_dir("/sys/class/leds") {
   for entry in entries.flatten() {
   // 只认面板背光 leds，跳过通知灯/按键灯/键盘背光/充电灯/闪光灯
   if !is_backlight_led_name(&entry.file_name().to_string_lossy()) {
   continue;
   }
   let dev = entry.path();
   if dev.join("brightness").exists() {
   nodes.push((dev, ScreenSourceKind::Leds));
   }
   }
   }
   if let Ok(entries) = fs::read_dir("/sys/class/graphics") {
   for entry in entries.flatten() {
   let dev = entry.path();
   if dev.join("blank").exists() {
   nodes.push((dev, ScreenSourceKind::FbBlank));
   }
   }
   }
   if let Ok(entries) = fs::read_dir("/sys/class/lcd") {
   for entry in entries.flatten() {
   let dev = entry.path();
   // 内核 LCD class 属性为 lcd_power（FB_BLANK_*）；`power` 是每个设备都有的运行时 PM 目录，故用 is_file 判定真正的属性文件
   if dev.join("lcd_power").exists() || dev.join("power").is_file() {
   nodes.push((dev, ScreenSourceKind::LcdPower));
   }
   }
   }
   if let Ok(entries) = fs::read_dir("/sys/class/drm") {
   for entry in entries.flatten() {
   let name = entry.file_name();
   // 只认内屏 connector（DSI/eDP/LVDS），跳过 HDMI/DP 外接屏——外接屏亮灭与本机面板息屏无关，误计票会干扰息屏仲裁
   let name = name.to_string_lossy().to_lowercase();
   if !(name.contains("dsi") || name.contains("edp") || name.contains("lvds")) {
   continue;
   }
   let dev = entry.path();
   if dev.join("enabled").exists() {
   nodes.push((dev, ScreenSourceKind::DrmEnabled));
   } else if dev.join("dpms").exists() {
   nodes.push((dev, ScreenSourceKind::DrmDpms));
   }
   }
   }
   nodes
   }
   // [read]
   /// 息屏仲裁票数：只统计**有效读数**。读不到、不存在的节点不计票，也不否决
   struct ScreenVotes {
   off: usize,
   on: usize,
   /// 第一个报 ON 的节点：打点用，说明为什么没确认息屏
   on_node: Option<String>,
   }
   /// 全节点投票：对候选清单逐个用 read_screen_state 读（各源读取口径只此一处，避免两套逻辑漂移），
   /// 不可读的直接跳过；清单由 screen_nodes 提供（缓存枚举结果），本函数不做任何枚举
   fn tally_screen_nodes(nodes: &[(PathBuf, ScreenSourceKind)]) -> ScreenVotes {
   let mut votes = ScreenVotes {
   off: 0,
   on: 0,
   on_node: None,
   };
   for (dev, kind) in nodes {
   match read_screen_state(*kind, dev) {
   Some(true) => {
   votes.on += 1;
   if votes.on_node.is_none() {
   votes.on_node = Some(dev.display().to_string());
   }
   }
   Some(false) => votes.off += 1,
   None => debug!("screen-detect: node unreadable, skipped: {}", dev.display()),
   }
   }
   debug!("screen-detect: votes off={} on={}", votes.off, votes.on);
   votes
   }
   /// 是否确认息屏：OFF 票超过有效票数一半（多数票）；单节点机型仍按那 1 票算；无任何有效读数不确认——没有证据就不改判
   fn screen_off_confirmed(votes: &ScreenVotes) -> bool {
   let valid = votes.off + votes.on;
   valid > 0 && votes.off * 2 > valid
   }
   /// 读取检测源的屏幕开关状态；None = 不可读（调用方静默跳过）
   fn read_screen_state(kind: ScreenSourceKind, dev: &Path) -> Option<bool> {
   match kind {
   ScreenSourceKind::Backlight => read_backlight_state(dev),
   ScreenSourceKind::Leds => {
   crate::utils::read_i32_from_file(&dev.join("brightness").to_string_lossy())
   .ok()
   .map(|v| v > 0)
   }
   ScreenSourceKind::FbBlank => {
   crate::utils::read_i32_from_file(&dev.join("blank").to_string_lossy())
   .ok()
   .map(|v| v == 0)
   }
   ScreenSourceKind::LcdPower => {
   // LCD class：lcd_power（少数驱动写作 power）= FB_BLANK_*，0 = UNBLANK（亮），其余（4 = POWERDOWN 等）= 灭
   let p = dev.join("lcd_power");
   let p = if p.exists() { p } else { dev.join("power") };
   crate::utils::read_i32_from_file(&p.to_string_lossy())
   .ok()
   .map(|v| v == 0)
   }
   ScreenSourceKind::DrmEnabled => {
   let raw = fs::read_to_string(dev.join("enabled")).ok()?;
   match raw.trim() {
   "enabled" => Some(true),
   "disabled" => Some(false),
   _ => None,
   }
   }
   ScreenSourceKind::DrmDpms => {
   let raw = fs::read_to_string(dev.join("dpms")).ok()?;
   match raw.trim() {
   "On" => Some(true),
   "Off" | "Standby" | "Suspend" => Some(false),
   _ => None,
   }
   }
   }
   }
   /// 读取 backlight class 设备的屏幕开关状态：`bl_power == 0` → 亮（权威信号）；`bl_power != 0` → 不可信
   /// （部分 DRM 面板驱动息屏写 FB_BLANK 后亮屏路径不清零，当权威信号会让 verify 把状态钉死在 false），
   /// 以 `actual_brightness` 为准（Android 息屏背光写 0，>0 即面板在发光）；`bl_power` 不可读 → 回退
   /// `actual_brightness` > 0，也不可读 → 回退 `brightness > 0`（驱动存的目标亮度）；全部不可读 → None
   fn read_backlight_state(dev: &Path) -> Option<bool> {
   let bl_power = dev.join("bl_power");
   let actual = dev.join("actual_brightness");
   let target = dev.join("brightness");
   let bl = crate::utils::read_i32_from_file(&bl_power.to_string_lossy()).ok();
   let act = crate::utils::read_i32_from_file(&actual.to_string_lossy()).ok();
   let tgt = crate::utils::read_i32_from_file(&target.to_string_lossy()).ok();
   match (bl, act, tgt) {
   (Some(0), _, _) => Some(true),
   (Some(_), Some(a), _) => Some(a > 0),
   (Some(_), None, Some(b)) => Some(b > 0),
   (Some(_), None, None) => Some(false),
   (None, Some(a), _) => Some(a > 0),
   (None, None, Some(b)) => Some(b > 0),
   (None, None, None) => None,
   }
   } */

// [prop]
/// 屏幕状态检测（现役）：读 Android 系统属性 debug.tracing.screen_state（框架亮/灭屏时写 0/1）
/// 判定口径（用户定）：当前值 == 息屏判定值即息屏，其余一切值（含缺失/异常）一律亮屏；息屏判定值 =
/// common::screen_off_value（meta.yaml `screen_off_value`，缺省 1）每次读数打 trace 日志
/// 属性缺失告警去重：个别 ROM 不写该属性时恒判亮屏，warn 一条留痕，读到有效值即复位
/// TODO: 恢复 sysfs 投票检测（[source]/[select]/[read]）后移除属性轮询路径
static PROP_MISSING_LOGGED: AtomicBool = AtomicBool::new(false);

/// 读属性原始值：None = 缺失/为空；Some = 原始字符串（trim 后）供 status.csv 的 screen_prop 列与本模块判定共用
pub fn read_screen_prop_raw() -> Option<String> {
    // PROP_VALUE_MAX = 92（Android 系统属性单值上限）
    let mut buf = [0u8; 92];
    let name = c"debug.tracing.screen_state";
    // SAFETY: name 为 NUL 结尾常量、buf 定长缓冲；__system_property_get 只在缓冲内写并返回实际长度，无别名访问
    let len = unsafe { libc::__system_property_get(name.as_ptr(), buf.as_mut_ptr()) };
    if len <= 0 {
        if !PROP_MISSING_LOGGED.swap(true, Ordering::Relaxed) {
            warn!("screen-detect: debug.tracing.screen_state missing; treated as screen-on");
        }
        return None;
    }
    PROP_MISSING_LOGGED.store(false, Ordering::Relaxed);
    let raw = String::from_utf8_lossy(&buf[..len as usize]);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        if !PROP_MISSING_LOGGED.swap(true, Ordering::Relaxed) {
            warn!("screen-detect: debug.tracing.screen_state missing; treated as screen-on");
        }
        return None;
    }
    Some(trimmed.to_string())
}

/// 现役判定：当前值 == 息屏判定值 → 息屏；其余一切（含缺失/异常）→ 亮屏
fn read_screen_prop() -> bool {
    let raw = read_screen_prop_raw();
    let state = match &raw {
        Some(v) => *v != crate::common::screen_off_value().to_string(),
        None => true,
    };
    trace!(
        "screen-detect: screen_state={} off_value={} -> {}",
        raw.as_deref().unwrap_or("<missing>"),
        crate::common::screen_off_value(),
        if state { "on" } else { "off" }
    );
    state
}

/// 属性轮询周期：单次属性读是 bionic 共享内存读（`libc::__system_property_get`，µs 级、不起子进程），
/// 故压到 150ms——**亮屏→退出 scenemode（恢复在线核 + 恢复 CLG 配置）的感知延迟 ≤450ms**
/// （最坏：150ms 轮询 × 2 次去抖确认 + 被 `SCREEN_MIN_FLIP_INTERVAL` 再挡一轮 150ms；2026-09-28
/// 加快退出速度。原 500ms 时点亮屏幕最长要等半秒才恢复，手感上就是「亮屏后性能恢复慢」）
const SCREEN_PROP_POLL_MS: u64 = 150;

/// 去抖：同一新状态需连续读到该次数（150ms × 2 ≈ 300ms 稳定窗口）才上报。
/// 必要性：抬手亮屏/通知会写出「息屏 → 1s 后亮屏」这类秒级抖动对（0927 包实测多处），每次抖动都会走
/// 一整轮 doze 切换 + CLG 重建；周期缩短后更容易抓到瞬时值，单靠缩短周期反而会放大抖动代价。
/// 副作用（正向）：属性单次读失败（缺失→按亮屏）不再能翻转状态
const SCREEN_PROP_DEBOUNCE: u32 = 2;

/// 屏幕状态属性轮询线程（现役）：每 `SCREEN_PROP_POLL_MS` 读一次 debug.tracing.screen_state，
/// 变化**且稳定 `SCREEN_PROP_DEBOUNCE` 个周期**才更新共享状态并直推 `ScreenStateChange` 事件
/// （与原 uevent 直推同管道）
pub fn monitor_screen_state_property(state_arc: Arc<Mutex<bool>>, tx: SyncSender<DaemonEvent>) {
    let mut candidate = *state_arc.lock().unwrap();
    let mut hits = 0u32;
    loop {
        thread::sleep(Duration::from_millis(SCREEN_PROP_POLL_MS));
        let state = read_screen_prop();
        // 以共享状态为比较基准（verify 自愈路径与 uevent 也会写它），避免去抖计数与真实状态错位
        let current = *state_arc.lock().unwrap();
        if state == current {
            candidate = state;
            hits = 0;
            continue;
        }
        if state == candidate {
            hits += 1;
        } else {
            candidate = state;
            hits = 1;
        }
        if hits >= SCREEN_PROP_DEBOUNCE {
            if update_state_if_changed(&state_arc, state, "prop") {
                let _ = tx.send(DaemonEvent::ScreenStateChange(state));
            }
            candidate = state;
            hits = 0;
        }
    }
}

// [verify]
/// 屏幕状态自愈校验：app_detect 主循环每轮调用，读属性校正共享状态——覆盖轮询线程启动早期（首次读数前）与线程意外停滞的情况；事件由主循环兜底转发
/// [PAUSED] 原 sysfs 全节点投票口径见 [source]/[select]/[read]
pub fn verify_screen_state(state_arc: &Arc<Mutex<bool>>) {
    update_state_if_changed(state_arc, read_screen_prop(), "verify");
}

// [uevent]
pub fn monitor_screen_state_uevent(
    // [PAUSED] 屏幕分支暂停后本函数不再消费这两项；恢复屏幕分支时去掉下划线
    _state_arc: Arc<Mutex<bool>>,
    _tx: SyncSender<DaemonEvent>,
) -> Result<(), Box<dyn Error>> {
    let mut socket = Socket::new(NETLINK_KOBJECT_UEVENT)?;
    let sa = SocketAddr::new(process::id(), 1);
    socket.bind(&sa)?;
    let _ = socket.set_rx_buf_sz(2 * 1024 * 1024);
    info!("{}", t("screen-netlink-started"));

    loop {
        match socket.recv_from_full() {
            Ok((buf, _)) => {
                if let Ok(event) = UEvent::from_netlink_packet(&buf) {
                    debug!(
                        "{}",
                        t_with_args(
                            "screen-uevent-received",
                            &fluent_args!(
                                "subsystem" => event.subsystem.as_str(),
                                "devpath" => event.devpath.to_string_lossy().to_string()
                            )
                        )
                    );
                    // [PAUSED] power 屏幕分支暂停（属性轮询见 [prop]）；恢复 = 还原本分支及下方 backlight/leds 分支
                    if event.subsystem == "cpu" {
                        // CPU hotplug（cpuN/online 变更）：只置脏标记，下一轮 affinity 立即刷新在线核位图（≤2s），事件风暴也只是次原子写
                        crate::monitor::CPU_HOTPLUG_DIRTY.store(true, Ordering::Relaxed);
                    }
                    /* [PAUSED] backlight/leds 屏幕分支暂停（属性轮询见 [prop]）：恢复 = 还原两个分支体，并在导入处补回 kobject_uevent::ActionType。 */

                    /* thread::sleep(Duration::from_millis(100));
                       // 与 verify 自愈同口径（read_backlight_state）：bl_power==0 → 亮；非 0（含亮屏
                       // 路径不清零的陈旧值）以 actual_brightness 为准，不可读返回 None
                       let dev =
                       std::path::PathBuf::from(format!("/sys{}", event.devpath.display()));
                       let new_state = read_screen_state(ScreenSourceKind::Backlight, &dev);
                       if let Some(state) = new_state {
                       debug!(
                       "{}",
                       t_with_args(
                       "screen-uevent-backlight",
                       &fluent_args!(
                       "dev" => dev.display().to_string(),
                       "state" => state.to_string()
                       )
                       )
                       );
                       // 同 power 分支：变化直推事件，消除轮询延迟
                       if update_state_if_changed(&state_arc, state, "backlight") {
                       let _ = tx.send(DaemonEvent::ScreenStateChange(state));
                       }
                       } else {
                       debug!(
                       "{}",
                       t_with_args(
                       "screen-uevent-backlight-unreadable",
                       &fluent_args!(
                       "dev" => dev.display().to_string()
                       )
                       )
                       );
                       }
                       } else if event.subsystem == "leds" && event.action == ActionType::Change {
                       // leds 背光亮度变化 uevent：仅认面板背光节点（口径与枚举一致 is_backlight_led_name）；
                       // 多数 led 驱动不广播亮度变化 uevent，verify 轮询兜底是主路径，此处能收到即零延迟直推
                       let dev =
                       std::path::PathBuf::from(format!("/sys{}", event.devpath.display()));
                       let is_backlight_led = dev
                       .file_name()
                       .map(|n| is_backlight_led_name(&n.to_string_lossy()))
                       .unwrap_or(false);
                       if is_backlight_led {
                       thread::sleep(Duration::from_millis(100));
                       let new_state = read_screen_state(ScreenSourceKind::Leds, &dev);
                       if let Some(state) = new_state {
                       debug!(
                       "{}",
                       t_with_args(
                       "screen-uevent-leds",
                       &fluent_args!(
                       "dev" => dev.display().to_string(),
                       "state" => state.to_string()
                       )
                       )
                       );
                       if update_state_if_changed(&state_arc, state, "leds") {
                       let _ = tx.send(DaemonEvent::ScreenStateChange(state));
                       }
                       } else {
                       debug!(
                       "{}",
                       t_with_args(
                       "screen-uevent-leds-unreadable",
                       &fluent_args!(
                       "dev" => dev.display().to_string()
                       )
                       )
                       );
                       }
                       }
                       } */
                }
            }
            Err(_) => thread::sleep(Duration::from_secs(1)),
        }
    }
}
