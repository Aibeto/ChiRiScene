//! FAS（帧感知调度）管理器 —— 单实例（原「一个进程绑一个 FAS 实例」的多实例架构已废弃）
//! 区块索引: [types] [activate] [deactivate] [delayed_exit] [events]
//! 白名单前台判断 + 延迟进出：进白名单前台 → activate（governor 切 performance + 接管频率）；
//! 失去前台不立即退出：request_delayed_exit 进入延迟期（时长来自应用配置 deactivate_delay_secs，夹 1..=600s），
//! 期间仍持有接管（mode 保持 fas、按负载控制）；切回白名单 activate 即取消延迟续期；
//! 超时由 tick()（1s 周期）真正退出：恢复频率 + governor 快照，返回 true 让调用方按延迟期记住的目标模式重新接管
//! governor（performance）归 GovernorGuard 管、频率归 FasController 管：GovernorGuard 必须**先于**引擎 load_policies 快照——
//! 引擎自身也会快照 governor 再写 performance，本层若在其后快照会拿到 performance，退出时「引擎 reset_all_freqs 先写 performance、
//! 本层 release 后写快照」的终值就成了 performance（残留）；perfmgr_enable 由引擎写成 0，本层按同款快照/恢复兜住

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::info;

use crate::chiri::governor::GovernorGuard;
use crate::fluent_args;
use crate::i18n::t_with_args;
use crate::monitor::{FasSignal, FrameSource};
use crate::scheduler::fas::FasController;

// 进程身份取证单独成文件（PID 复用/存活判定）；#[path] 引入以免改主线 chiri/mod.rs 注册
#[path = "fas_process.rs"]
mod fas_process;
use fas_process::ProcessIdentity;

/// 活跃实例温度刷新周期（喂给 FAS 引擎内部限温逻辑，core_temp_threshold=0 时无效）
const FAS_TEMP_REFRESH: Duration = Duration::from_secs(3);

/// 接管期间写入 `sched_migration_cost_ns` 的节点（配置 None 时不动）
// TODO: 实机复核接管期写入与退出恢复（退出后该节点应回到接管前的值）
const MIGRATION_COST_PATH: &str = "/proc/sys/kernel/sched_migration_cost_ns";

/// 接管期间被 FAS 引擎 `load_policies` 无条件写成 0 的厂商 perfmgr 使能节点（与 `scheduler/fas/policy_mgmt.rs` 的写点同源）。
/// 引擎侧只写不恢复，故接管前必须快照原值、退出时写回，否则厂商默认被永久改写
const PERFMGR_ENABLE_PATHS: [&str; 2] = [
    "/sys/module/perfmgr/parameters/perfmgr_enable",
    "/sys/module/mtk_fpsgo/parameters/perfmgr_enable",
];

// [types]
struct FasInstance {
    package: String,
    frame_source: FrameSource,
    /// owner 进程身份（starttime + 可选 pidfd）：同包 PID 复用与存活判定的依据
    identity: ProcessIdentity,
    controller: FasController,
    /// 接管前 `sched_migration_cost_ns` 原值，deactivate 写回（None = 未写或读不到）
    migration_cost_restore: Option<String>,
    /// 接管前 perfmgr 使能节点原值快照（路径, 原值；缺失机型跳过）：引擎 load_policies 会写成 0，deactivate 逐条写回
    perfmgr_restore: Vec<(String, String)>,
}

pub struct FasManager {
    /// 单实例：同一时刻至多一个白名单应用被接管
    instance: Option<FasInstance>,
    /// 延迟退出的截止时刻：失去白名单前台后 Some(截止)；在前台/未接管为 None
    exit_deadline: Option<Instant>,
    /// 延迟时长（activate 时从应用配置刷新；normalize 已夹在 1..=600s）
    exit_delay: Duration,
    /// performance 调速器接管（activate 时切，deactivate 时按快照恢复）
    governor: GovernorGuard,
    last_temp: f64,
    last_temp_read: Instant,
    /// FAS 温度源节点路径。原始读数刻度因内核而异，每次刷新经 `utils::battery_temp_divisor()` 取全局预识别结论（与 CLG 热保护同源），避免两处口径漂移）；
    /// None = 无温度源（引擎侧护栏失效，不影响其余功能）
    temp_path: Option<PathBuf>,
    /// FAS 前台激活信号（monitor 层 fps_monitor 等待消费）：activate 置位、deactivate 清零——fps_monitor 据此推迟/摘除 eBPF uprobe（反偷跑门控），
    /// 置位瞬间唤醒待机线程（见 `crate::monitor::FasSignal`）
    fas_signal: Arc<FasSignal>,
    generation: u64,
}

impl FasManager {
    /// temp_path：FAS 专用温度源节点看电池不看处理器——电池温度是热安全边界（阈值按 ℃ 配置），处理器长期 95℃ 属正常工作区，不作降频依据；None = 内部限温关闭fas_signal 由 main
    /// rs 创建、monitor 与 chiri 两层共享
    pub fn new(temp_path: Option<PathBuf>, fas_signal: Arc<FasSignal>) -> Self {
        Self {
            instance: None,
            exit_deadline: None,
            exit_delay: Duration::from_secs(15),
            governor: GovernorGuard::new(),
            last_temp: 0.0,
            last_temp_read: Instant::now(),
            temp_path,
            fas_signal,
            generation: 0,
        }
    }

    /// C1：激活返回 false = 白名单/配置不可用或无可用 policy；同包同 PID 仅续期，PID 替换重建帧会话。
    /// 另一白名单包 → fas→fas 热切换（打点 scheduler-fas-switch）；首次 → FasController::new + load_policies
    /// 激活同时接管 governor（performance）；延迟退出请求在此被取消（无缝续期）
    // [activate]
    pub fn activate(&mut self, pkg: &str, pid: i32) -> bool {
        if pid <= 0 {
            return false;
        }
        // 白名单复查：包名 → 白名单配置名 → FAS 规则（'static，normalize 已在缓存时完成）
        let Some(rules) = crate::common::fas_whitelist_entry(pkg)
            .and_then(|cfg| crate::common::fas_app_config(cfg))
        else {
            return false;
        };
        self.exit_delay = Duration::from_secs(u64::from(rules.deactivate_delay_secs.max(1)));

        // 单实例不变量：另一包仍活跃，先按 C2 去激活（防御调用方未显式调用）
        let switch_from = self
            .instance
            .as_ref()
            .map(|i| i.package.clone())
            .filter(|a| a != pkg);
        if switch_from.is_some() {
            self.deactivate_active();
        }

        if let Some(inst) = self.instance.as_mut() {
            // 同包同 PID 仍需核对进程身份：PID 复用（starttime 变化）要当新会话，不能沿用旧帧控制状态；
            // 同 identity 续期仅取消 deadline，不重置窗口/齿轮，也不重复 set_game 打点。
            let same_identity = inst.frame_source.pid == pid as u32
                && inst
                    .identity
                    .matches_starttime(fas_process::read_process_starttime(pid));
            if !same_identity {
                self.generation = self
                    .generation
                    .checked_add(1)
                    .expect("FAS generation exhausted");
                inst.identity = ProcessIdentity::attach(pid);
                inst.frame_source.pid = pid as u32;
                inst.frame_source.generation = self.generation;
                inst.controller.set_game(pid, pkg);
                inst.controller.reset_frame_feedback();
            }
            self.exit_deadline = None;
            self.set_frame_feedback_enabled(true);
            self.fas_signal.set_frame_source(self.frame_source());
            return true;
        }

        // [perfmgr] 快照必须在 load_policies 之前：引擎会把两个 perfmgr_enable 无条件写成 0，先读原值才能退出写回
        let perfmgr_restore = snapshot_perfmgr_enable();
        // [governor] 快照也必须先于 load_policies：引擎自身会「先快照 governor 再写 performance」，本层若在其后快照，
        // 拿到的就是引擎写的 performance 而非系统原值；退出时序为「引擎 reset_all_freqs 先写回它的快照（=performance）、
        // 本层 release 后写回本层快照」，终值由后写者决定——必须让持有真值的本层收尾，否则残留 performance
        self.governor.activate();

        let mut controller = FasController::new();
        controller.load_policies(rules);
        if controller.policies.is_empty() {
            // 早退：不建实例、退出路径不会再来恢复，而 load_policies 已写 perfmgr=0、并把各 policy 的 governor 改成
            // performance；必须就地原路撤回，否则残留泄漏（这是 activate 唯一「已写但无实例」的路径）
            restore_perfmgr_enable(&perfmgr_restore);
            self.governor.release();
            return false;
        }
        // 迁移成本：读不到原值就不写，保证退出能恢复
        let migration_cost_restore = rules.migration_cost_ns.filter(|v| *v > 0).and_then(|want| {
                let orig = std::fs::read_to_string(MIGRATION_COST_PATH).ok()?;
                let orig = orig.trim().to_string();
                crate::utils::try_write_file(MIGRATION_COST_PATH, &want.to_string()).ok()?;
                Some(orig)
            });
        controller.set_game(pid, pkg);
        controller.reset_frame_feedback();
        controller.set_temperature(self.last_temp);
        controller.set_temp_threshold(rules.core_temp_threshold);
        self.generation = self
            .generation
            .checked_add(1)
            .expect("FAS generation exhausted");
        self.instance = Some(FasInstance {
            package: pkg.to_string(),
            frame_source: FrameSource {
                pid: pid as u32,
                generation: self.generation,
                frame_feedback: true,
            },
            identity: ProcessIdentity::attach(pid),
            controller,
            migration_cost_restore,
            perfmgr_restore,
        });
        self.exit_deadline = None;
        self.fas_signal.set_frame_source(self.frame_source());
        match switch_from.as_deref() {
            Some(old) => info!(
                "{}",
                t_with_args(
                    "scheduler-fas-switch",
                    &fluent_args!("old" => old, "new" => pkg)
                )
            ),
            None => info!(
                "{}",
                t_with_args(
                    "scheduler-fas-activate",
                    &fluent_args!("pkg" => pkg, "pid" => pid.to_string())
                )
            ),
        }
        crate::logger::main_event("fas", pkg, "activate");
        true
    }

    /// C2：立即去激活（reset_all_freqs + clear_game + governor 按快照恢复），并清零 fas_signal（fps_monitor 摘除 uprobe 回到零开销待机）
    /// 无活跃实例 no-op；延迟退出请求一并取消打点 info scheduler-fas-deactivate（pkg）+ main_event("fas", pkg, "deactivate")
    // [deactivate]
    pub fn deactivate_active(&mut self) {
        let Some(mut inst) = self.instance.take() else {
            return;
        };
        self.exit_deadline = None;
        self.fas_signal.set_frame_source(None);
        // 迁移成本与 perfmgr 先于频率恢复，让后续 governor 快照到系统原状
        if let Some(orig) = inst.migration_cost_restore.take() {
            let _ = crate::utils::try_write_file(MIGRATION_COST_PATH, &orig);
        }
        // perfmgr_enable 恢复：引擎接管前写成 0，按快照写回厂商原值（失败只记日志，不中断收尾）
        restore_perfmgr_enable(&inst.perfmgr_restore);
        // 先恢复频率再清状态：调用方随后 init 其他 governor（CLG/akmode/fast）时，对方才能快照到真实的系统状态
        inst.controller.reset_all_freqs();
        inst.controller.clear_game();
        self.governor.release();
        let pkg = inst.package;
        info!(
            "{}",
            t_with_args(
                "scheduler-fas-deactivate",
                &fluent_args!("pkg" => pkg.as_str())
            )
        );
        crate::logger::main_event("fas", &pkg, "deactivate");
    }

    /// C5：收尾/失败路径（panic 自愈、DOWN、fas_enabled=false 热重载）——立即全退
    pub fn deactivate_all(&mut self) {
        self.deactivate_active();
    }

    pub fn is_active(&self) -> bool {
        self.instance.is_some()
    }

    pub fn active_pkg(&self) -> Option<&str> {
        self.instance.as_ref().map(|i| i.package.as_str())
    }

    pub fn active_pid(&self) -> Option<i32> {
        self.frame_source().map(|source| source.pid as i32)
    }

    pub fn frame_source(&self) -> Option<FrameSource> {
        self.instance.as_ref().map(|instance| instance.frame_source)
    }

    /// 当前 owner 是否就是该 pid/starttime 进程：供主线同 session 门控（快事件只喂同一会话）。
    /// fail-closed：pid 不同或任一侧 starttime 未知都判 false，绝不把无法取证的事件算作同一会话。
    pub fn owner_matches_process(&self, pid: i32, process_starttime: Option<u64>) -> bool {
        self.instance.as_ref().is_some_and(|instance| {
            instance.frame_source.pid == pid as u32
                && instance.identity.matches_starttime(process_starttime)
        })
    }

    /// owner 进程是否仍存活：pidfd 零 poll 优先；回退路径读一次 /proc（仅供主线低频 reconcile，
    /// 不要放进每帧路径）。无实例 = 无 owner，返回 false，调用方走退出。
    pub fn owner_is_alive(&self) -> bool {
        self.instance.as_ref().is_some_and(|instance| {
            let Ok(pid) = i32::try_from(instance.frame_source.pid) else {
                return false;
            };
            instance
                .identity
                .cheap_alive()
                .unwrap_or_else(|| instance.identity.is_alive(pid))
        })
    }

    pub fn set_frame_feedback_enabled(&mut self, enabled: bool) {
        let Some(instance) = self.instance.as_mut() else {
            return;
        };
        if instance.frame_source.frame_feedback == enabled {
            return;
        }
        self.generation = self
            .generation
            .checked_add(1)
            .expect("FAS generation exhausted");
        instance.frame_source.generation = self.generation;
        instance.frame_source.frame_feedback = enabled;
        instance.controller.set_frame_feedback_enabled(enabled);
        self.fas_signal
            .set_frame_source(Some(instance.frame_source));
    }

    /// 是否存在 FAS 接管（含延迟退出期）scenemode 入口门控依据：接管期间禁止进入，延迟到期退出后自动放行
    pub fn has_any_instance(&self) -> bool {
        self.instance.is_some()
    }

    // [delayed_exit]
    /// 失去白名单前台：进入延迟退出期（FAS 仍持有接管，mode 保持 fas）；期间 activate（切回白名单）会取消延迟。未活跃时无操作
    pub fn request_delayed_exit(&mut self) {
        if self.instance.is_some() && self.exit_deadline.is_none() {
            self.exit_deadline = Some(Instant::now() + self.exit_delay);
        }
    }

    pub fn cancel_delayed_exit(&mut self) {
        self.exit_deadline = None;
    }

    /// 同包回到前台：取消延迟退出。延迟期内切回**同一个**白名单应用不走 activate（PackageSwitch 同包去重 / 1s 巡检同包 no-op），必须由调用方显式续期，
    /// 否则到期会把正在前台的游戏拆掉重建（局内卡顿）
    pub fn renew_if_same_pkg(&mut self, pkg: &str) {
        if self.exit_deadline.is_some() && self.instance.as_ref().is_some_and(|i| i.package == pkg)
        {
            self.exit_deadline = None;
        }
    }

    /// 1s 周期调用：延迟退出到期则完成退出（恢复频率 + governor 快照）。返回 true = 刚完成退出，调用方需按延迟期记住的目标模式重新接管
    pub fn tick(&mut self) -> bool {
        match self.exit_deadline {
            Some(deadline) if Instant::now() >= deadline => {
                self.deactivate_active();
                true
            }
            _ => false,
        }
    }

    /// 帧事件（仅活跃实例）：内部每 FAS_TEMP_REFRESH 读一次 temp_path（按全局预识别刻度换算后存 last_temp 并 set_temperature）
    // [events]
    pub fn on_frame(&mut self, delta_ns: u64, source_pid: u32, generation: u64) {
        // 入队旧帧前拒死 owner：有 pidfd 时零 poll 廉价；无 pidfd 不在此读 /proc（主线 owner_is_alive 兜底）
        if self
            .instance
            .as_ref()
            .is_some_and(|instance| instance.identity.cheap_alive() == Some(false))
        {
            return;
        }
        if !self.frame_source().is_some_and(|source| {
            source.frame_feedback && source.pid == source_pid && source.generation == generation
        }) {
            return;
        }
        self.refresh_temperature();
        if let Some(inst) = self.instance.as_mut() {
            inst.controller.update_frame(delta_ns);
        }
    }

    /// 负载事件（仅活跃实例）update_cpu_util(fg_util) + update_core_utils(core_utils)
    pub fn on_load_update(&mut self, fg_util: f32, core_utils: &[f32]) {
        if !self.is_active() {
            return;
        }
        self.refresh_temperature();
        if let Some(inst) = self.instance.as_mut() {
            if inst.frame_source.frame_feedback {
                inst.controller.update_cpu_util(fg_util);
            }
            inst.controller.update_core_utils(core_utils);
            inst.controller.update_load_control();
        }
    }

    /// 当前活跃实例的帧率（fps）：FAS 未启动或窗口尚无样本时返回 None——调用方据此在 status.csv 的 fps 列写 "-"。只读快照，不推进引擎状态
    pub fn current_fps(&self) -> Option<f32> {
        self.instance.as_ref()?.controller.current_fps()
    }

    // [helpers]
    /// 每 FAS_TEMP_REFRESH 读一次温度源（按全局预识别刻度换算为 ℃），缓存 last_temp 并喂给引擎内部限温逻辑（core_temp_threshold=0 时引擎侧无效，此处照常喂）
    fn refresh_temperature(&mut self) {
        let Some(path) = self.temp_path.as_ref() else {
            return;
        };
        if self.last_temp_read.elapsed() < FAS_TEMP_REFRESH {
            return;
        }
        self.last_temp_read = Instant::now();
        let Ok(raw) = crate::utils::read_f64_from_file(&path.to_string_lossy()) else {
            return;
        };
        // 刻度取全局预识别结论（与 CLG 热保护同源）；无法确定时按内核标准 0.1°C 兜底
        let divisor = crate::utils::battery_temp_divisor().unwrap_or(10.0);
        let temp = raw / divisor;
        self.last_temp = temp;
        if let Some(inst) = self.instance.as_mut() {
            inst.controller.set_temperature(temp);
        }
    }
}

// [perfmgr]
/// 探测并快照厂商 perfmgr 使能节点原值（存在且可读才记，缺失机型跳过）：FAS 引擎 `load_policies` 会把这两个节点
/// 无条件写成 0（关闭厂商 perfmgr 的频率干预），本层不先快照则退出后无从恢复，厂商默认被永久改写
fn snapshot_perfmgr_enable() -> Vec<(String, String)> {
    let mut snap = Vec::new();
    for path in PERFMGR_ENABLE_PATHS {
        if let Ok(v) = std::fs::read_to_string(path) {
            snap.push((path.to_string(), v.trim().to_string()));
        }
    }
    snap
}

/// 按快照逐条写回 perfmgr 使能节点。用 `try_write_file`：失败只记日志不 panic——收尾路径不能因单点失败中断
fn restore_perfmgr_enable(snap: &[(String, String)]) {
    for (path, val) in snap {
        let _ = crate::utils::try_write_file(path, val);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_manager() -> FasManager {
        let mut manager = FasManager::new(None, Arc::new(FasSignal::new(false)));
        manager.instance = Some(FasInstance {
            package: "game".to_string(),
            frame_source: FrameSource {
                pid: 100,
                generation: 0,
                frame_feedback: true,
            },
            identity: ProcessIdentity::for_test(Some(1234)),
            controller: FasController::new(),
            migration_cost_restore: None,
            perfmgr_restore: Vec::new(),
        });
        manager
    }

    #[test]
    fn owner_matches_process_uses_starttime() {
        let manager = active_manager();
        assert!(manager.owner_matches_process(100, Some(1234)));
        assert!(!manager.owner_matches_process(100, Some(9999)));
        assert!(!manager.owner_matches_process(101, Some(1234)));
        // fail-closed：请求侧 starttime 未知时不认同一会话
        assert!(!manager.owner_matches_process(100, None));
    }

    #[test]
    fn owner_is_alive_reconciles_without_pidfd() {
        let manager = active_manager();
        // 有 starttime 基准、真实 /proc 无此 pid → 判死（PID 复用/已退出）；无实例 → false
        assert!(!manager.owner_is_alive());
        let mut unknown_baseline = FasManager::new(None, Arc::new(FasSignal::new(false)));
        unknown_baseline.instance = Some(FasInstance {
            package: "game".to_string(),
            frame_source: FrameSource {
                pid: 100,
                generation: 0,
                frame_feedback: true,
            },
            identity: ProcessIdentity::for_test(None),
            controller: FasController::new(),
            migration_cost_restore: None,
            perfmgr_restore: Vec::new(),
        });
        // 基准未知：fail-closed 判死，交主线退出
        assert!(!unknown_baseline.owner_is_alive());
        let mut empty = FasManager::new(None, Arc::new(FasSignal::new(false)));
        assert!(!empty.owner_is_alive());
        empty.instance = None;
    }

    #[test]
    fn activate_same_pid_unknown_starttime_is_new_session_fail_closed() {
        let mut manager = FasManager::new(None, Arc::new(FasSignal::new(false)));
        // 取一个几乎不可能存在的 pid，确保 starttime 读取为 None（不依赖环境是否占用某 pid）
        let absent_pid = 2_000_000;
        assert!(manager.activate("game", absent_pid));
        let first = manager.frame_source().unwrap();
        assert_eq!(first.generation, 1);
        // 真实 /proc 无该 pid → starttime 未知；fail-closed 视为新会话：递增 generation 并重置帧控制
        assert!(manager.activate("game", absent_pid));
        let second = manager.frame_source().unwrap();
        assert_eq!(second.pid, absent_pid as u32);
        assert!(second.generation > first.generation);
    }

    #[test]
    fn repeated_departure_does_not_extend_deadline() {
        let mut manager = active_manager();
        manager.request_delayed_exit();
        let original_deadline = manager.exit_deadline;
        manager.request_delayed_exit();
        assert_eq!(manager.exit_deadline, original_deadline);
    }

    #[test]
    fn same_package_return_at_deadline_prevents_expiry() {
        let mut manager = active_manager();
        manager.exit_deadline = Some(Instant::now());
        manager.renew_if_same_pkg("other");
        assert!(manager.exit_deadline.is_some());
        manager.renew_if_same_pkg("game");
        assert!(!manager.tick());
        assert!(manager.is_active());
    }

    #[test]
    fn feedback_transitions_invalidate_sessions_but_preserve_owner() {
        let mut manager = active_manager();
        let initial = manager.frame_source().unwrap();
        manager.set_frame_feedback_enabled(false);
        let suspended = manager.frame_source().unwrap();
        assert!(suspended.generation > initial.generation);
        assert_eq!(manager.active_pid(), Some(100));
        assert!(manager.fas_signal.is_active());
        assert!(!manager.fas_signal.frame_source_active());
        manager.set_frame_feedback_enabled(false);
        assert_eq!(manager.frame_source(), Some(suspended));
        manager.set_frame_feedback_enabled(true);
        let resumed = manager.frame_source().unwrap();
        assert!(resumed.generation > suspended.generation);
        assert!(manager.fas_signal.frame_source_active());
    }

    #[test]
    fn old_generation_and_foreign_pid_cannot_feed_resumed_game() {
        let mut manager = active_manager();
        manager.instance.as_mut().unwrap().controller =
            crate::scheduler::fas::controller_with_policy();
        let initial = manager.frame_source().unwrap();
        manager.set_frame_feedback_enabled(false);
        manager.on_frame(16_666_667, initial.pid, initial.generation);
        assert_eq!(manager.current_fps(), None);
        manager.set_frame_feedback_enabled(true);
        let resumed = manager.frame_source().unwrap();
        manager.on_frame(16_666_667, initial.pid, initial.generation);
        manager.on_frame(16_666_667, resumed.pid + 1, resumed.generation);
        assert_eq!(manager.current_fps(), None);
        // 预热期间 current_fps 仍为 None，需喂满预热帧后才暴露窗口均值
        for _ in 0..12 {
            manager.on_frame(16_666_667, resumed.pid, resumed.generation);
        }
        assert!(manager.current_fps().is_some());
    }

    #[test]
    fn overlay_keepalive_cancels_deadline_without_resuming_feedback() {
        let mut manager = active_manager();
        manager.set_frame_feedback_enabled(false);
        manager.request_delayed_exit();
        let suspended = manager.frame_source();
        manager.cancel_delayed_exit();
        assert!(!manager.tick());
        assert_eq!(manager.frame_source(), suspended);
        assert!(manager.exit_deadline.is_none());
    }

    #[test]
    fn deadline_equality_expires_and_clears_frame_source() {
        let mut manager = active_manager();
        manager.exit_deadline = Some(Instant::now());
        assert!(manager.tick());
        assert!(!manager.is_active());
        assert_eq!(manager.frame_source(), None);
        assert_eq!(manager.fas_signal.frame_source(), None);
        assert!(!manager.tick());
    }

    #[test]
    fn repeated_activate_same_pid_keeps_frame_session() {
        let mut manager = FasManager::new(None, Arc::new(FasSignal::new(false)));
        assert!(manager.activate("game", 100));
        let first = manager.frame_source().unwrap();
        assert!(manager.activate("game", 100));
        assert_eq!(manager.frame_source(), Some(first));
        assert_eq!(manager.active_pid(), Some(100));
        assert_eq!(manager.generation, 1);
    }

    #[test]
    fn same_package_pid_replacement_advances_generation() {
        let mut manager = FasManager::new(None, Arc::new(FasSignal::new(false)));
        assert!(manager.activate("game", 100));
        let first = manager.frame_source().unwrap();
        assert!(manager.activate("game", 200));
        let replaced = manager.frame_source().unwrap();
        assert_eq!(replaced.pid, 200);
        assert!(replaced.generation > first.generation);
        assert_eq!(manager.active_pid(), Some(200));
    }
}
