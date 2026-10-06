//! controller.rs: [struct] [load] [helpers] [game]

use crate::fas_types::{FasRulesConfig, PerAppProfile};
use log::{info, warn};
use std::time::Instant;

use crate::fluent_args;
use crate::i18n::t_with_args;

use super::fps_window::FpsWindow;
use super::pid::{PidController, fps_norm};
use super::policy_controller::PolicyController;

pub(super) const FRAME_FEEDBACK_WARMUP_FRAMES: u32 = 12;
const FRAME_FEEDBACK_STALE: std::time::Duration = std::time::Duration::from_millis(500);

// [struct]
// FasController — 主控制器：帧率档位匹配 + PID 控制
// CPU 负载数据（core_utils/fg_util）由 SystemLoadUpdate 事件喂入

pub struct FasController {
    pub(super) frame_feedback_enabled: bool,
    pub(super) warmup_remaining: u32,
    pub(super) last_frame_at: Option<Instant>,
    pub(super) cfg: FasRulesConfig,
    pub(super) fps_margin: f32,

    pub(super) pid: PidController,

    pub(super) fps_gears: Vec<f32>,
    pub(super) current_target_fps: f32,
    pub(super) perf_index: f32,
    pub(super) ema_actual_ms: f32,

    pub policies: Vec<PolicyController>,

    pub(super) fps_window: FpsWindow,
    pub(super) log_counter: u32,
    pub(super) consecutive_normal_frames: u32,

    // 加载
    pub(super) is_loading: bool,
    pub(super) loading_frames: u32,
    pub(super) loading_cumulative_ms: f32,
    pub(super) loading_normal_tolerance: u32,
    pub(super) post_loading_ignore: u32,
    pub(super) post_loading_downgrade_guard: u32,

    // 齿轮
    pub(super) upgrade_confirm_frames: u32,
    pub(super) downgrade_confirm_frames: u32,
    pub(super) upgrade_cooldown: u32,
    pub(super) gear_dampen_frames: u32,
    pub(super) consecutive_downgrade_count: u32,
    pub(super) last_downgrade_from_fps: f32,
    /// 上次降档时刻：退避计数的时间衰减依据（距上次超过 `DOWNGRADE_BACKOFF_RESET` 视为新一轮降档，计数归 1）
    pub(super) last_downgrade_at: Option<Instant>,
    pub(super) stable_gear_frames: u32,

    // 降档 Boost
    pub(super) downgrade_boost_active: bool,
    pub(super) downgrade_boost_remaining: u32,
    pub(super) downgrade_boost_perf_saved: f32,

    // Jank
    pub(super) jank_cooldown: u32,
    pub(super) jank_streak: u32,

    // 时间
    pub(super) init_time: Instant,
    /// 上一次「防篡改强制重写」的时刻（间隔 = FasRulesConfig::freq_force_reapply_interval 秒）
    pub(super) freq_force_timer: Instant,

    // 缓存
    pub(super) cached_norm: f32,
    pub(super) cached_budget_ms: f32,
    pub(super) cached_ema_budget: f32,

    // 温度感知
    pub(super) current_temperature: f64,
    pub(super) temp_threshold: f64,
    /// 温度护栏锁存：≥temp_threshold 进入、<temp_threshold-3℃ 退出（迟滞防振荡）PID 只看帧时间，感知不到 thermal 压频，热限频时继续抬频会与 thermal 形成正反馈
    pub(super) thermal_hold: bool,

    // [新] CPU 负载数据 — 由 SystemLoadUpdate 事件更新
    pub(super) foreground_max_util: f32,
    pub(super) core_utils: Vec<f32>,

    // 当前游戏包名
    pub(super) current_package: String,
    // 当前游戏的 per-app 配置
    pub(super) active_profile: Option<PerAppProfile>,

    // perf 地板死锁连续帧计数
    pub(super) floor_stuck_frames: u32,

    // util_cap EMA 平滑值，防止 200ms 采样周期的滞后数据造成断崖
    pub(super) ema_fg_util: f32,

    // [Jank 恢复保护] crit/heavy 后的 perf 地板：防恢复帧后 PID 2-3 帧内衰减到 floor 再引发连锁 jank
    pub(super) post_jank_perf_floor: f32,
    pub(super) post_jank_guard_frames: u32,

    // [动态 PID] CPU 利用率驱动的 target_fps 偏移，范围 [-3.0, 0.0]：利用率持续偏低时逐步降低有效 target 让 PID 少给频率省电，回升时逐步恢复
    pub(super) target_fps_offset: f32,
    pub(super) util_sample_timer: Instant,
}

impl FasController {
    pub fn new() -> Self {
        let cfg = FasRulesConfig::default();
        let pid_ctrl = PidController::new(cfg.pid.kp, cfg.pid.ki, cfg.pid.kd);
        Self {
            frame_feedback_enabled: true,
            warmup_remaining: FRAME_FEEDBACK_WARMUP_FRAMES,
            last_frame_at: None,
            fps_margin: 3.0,
            perf_index: cfg.perf_init,
            pid: pid_ctrl,
            fps_gears: cfg.fps_gears.clone(),
            current_target_fps: 60.0,
            ema_actual_ms: 0.0,
            policies: Vec::new(),
            fps_window: FpsWindow::new(),
            log_counter: 0,
            consecutive_normal_frames: 0,
            is_loading: false,
            loading_frames: 0,
            loading_cumulative_ms: 0.0,
            loading_normal_tolerance: 0,
            post_loading_ignore: 0,
            post_loading_downgrade_guard: 0,
            upgrade_confirm_frames: 0,
            downgrade_confirm_frames: 0,
            upgrade_cooldown: 0,
            gear_dampen_frames: 0,
            consecutive_downgrade_count: 0,
            last_downgrade_from_fps: 0.0,
            last_downgrade_at: None,
            stable_gear_frames: 0,
            downgrade_boost_active: false,
            downgrade_boost_remaining: 0,
            downgrade_boost_perf_saved: 0.0,
            jank_cooldown: 0,
            jank_streak: 0,
            init_time: Instant::now(),
            freq_force_timer: Instant::now(),
            cached_norm: 1.0,
            cached_budget_ms: 16.67,
            cached_ema_budget: 17.54,
            current_temperature: 0.0,
            temp_threshold: 0.0,
            thermal_hold: false,
            foreground_max_util: 0.0,
            core_utils: Vec::new(),
            current_package: String::new(),
            active_profile: None,
            floor_stuck_frames: 0,
            ema_fg_util: 0.0,
            post_jank_perf_floor: 0.0,
            post_jank_guard_frames: 0,
            target_fps_offset: 0.0,
            util_sample_timer: Instant::now(),
            cfg,
        }
    }

    // [load]
    // CPU 负载接口 (来自 SystemLoadUpdate 事件)

    /// 更新前台最重线程的 CPU 利用率
    pub fn update_cpu_util(&mut self, fg_util: f32) {
        self.foreground_max_util = fg_util;
        if self.ema_fg_util <= 0.001 {
            self.ema_fg_util = fg_util;
        } else {
            // 升快降慢（alpha 0.4/0.15），防止瞬时低 util 误杀频率
            let alpha = if fg_util > self.ema_fg_util {
                0.40
            } else {
                0.15
            };
            self.ema_fg_util = self.ema_fg_util * (1.0 - alpha) + fg_util * alpha;
        }
    }

    /// 更新各核心利用率快照
    pub fn update_core_utils(&mut self, utils: &[f32]) {
        self.core_utils.clear();
        self.core_utils.extend_from_slice(utils);
    }

    pub fn set_frame_feedback_enabled(&mut self, enabled: bool) {
        if self.frame_feedback_enabled == enabled {
            return;
        }
        self.frame_feedback_enabled = enabled;
        self.reset_frame_feedback();
    }

    /// 新会话清空帧控制状态；12 个有效新帧完成预热，期间只按簇负载调频。
    pub fn reset_frame_feedback(&mut self) {
        // set_game 已按 per-app 齿轮集合选定目标；这里只抬高非法目标，全局齿轮情形下保持既定 target 语义
        let max_gear = self.max_gear();
        if !self.fps_gears.iter().any(|gear| (gear - self.current_target_fps).abs() < 0.5) {
            self.current_target_fps = max_gear;
        }
        self.reset_runtime();
        self.refresh_cached_values();
        self.foreground_max_util = 0.0;
        self.downgrade_boost_perf_saved = 0.0;
        self.last_frame_at = None;
        self.warmup_remaining = FRAME_FEEDBACK_WARMUP_FRAMES;
        self.init_time = Instant::now()
            .checked_sub(std::time::Duration::from_millis(
                self.cfg.cold_boot_ms as u64,
            ))
            .unwrap_or_else(Instant::now);
        for policy in &mut self.policies {
            policy.freq_hold_frames = 0;
        }
    }

    pub(super) fn uses_load_control(&self) -> bool {
        !self.frame_feedback_enabled
            || self.warmup_remaining > 0
            || self
                .last_frame_at
                .is_none_or(|timestamp| timestamp.elapsed() >= FRAME_FEEDBACK_STALE)
    }

    /// load-only 的地板：沿用配置 base，不叠加 current_target_fps bonus（帧反馈暂停时目标帧率与负载无关）。
    pub(super) fn load_control_floor(&self) -> f32 {
        self.cfg.perf_floor.min(self.cfg.perf_ceil)
    }

    /// 负载事件推进热护栏；仅 load-only、热护栏进入或 perf 实际变化时才 apply_freqs——
    /// 正常 FPS 反馈且热态未变时不得改写 freq_hold_frames/强写节奏。
    pub fn update_load_control(&mut self) {
        let previous_perf = self.perf_index;
        let was_holding = self.thermal_hold;
        self.update_thermal_hold();
        let load_only = self.uses_load_control();
        let entering_hold = !was_holding && self.thermal_hold;
        if load_only {
            let demand = self
                .core_utils
                .iter()
                .copied()
                .filter(|util| util.is_finite())
                .fold(0.0_f32, f32::max);
            let ceiling = self.effective_perf_ceil();
            self.perf_index = demand.clamp(self.load_control_floor().min(ceiling), ceiling);
        }
        if load_only || entering_hold {
            if let Some(cap) = self.thermal_perf_cap() {
                self.perf_index = self.perf_index.min(cap);
            }
        }
        if load_only || entering_hold || self.perf_index != previous_perf {
            self.apply_freqs();
        }
    }

    // [helpers]
    // 辅助方法

    /// 有效 perf_floor：高刷 budget 仅 6.9~8.3ms，perf 过低时突发负载立刻卡顿，故随 target_fps 抬高
    /// 旧公式硬顶 0.35 使 120fps 下 perf 贴地；新公式 floor = base + (target_fps-60)*0.004，上限 0.45
    /// （60fps→0.22、90fps→0.34、120fps→0.40、144fps→0.45）
    pub(super) fn effective_perf_floor(&self) -> f32 {
        let base = self.cfg.perf_floor;
        let fps_bonus = ((self.current_target_fps - 60.0).max(0.0) * 0.004).min(0.25);
        (base + fps_bonus).min(0.45)
    }

    /// 获取有效 perf_ceil
    pub(super) fn effective_perf_ceil(&self) -> f32 {
        self.cfg.perf_ceil
    }

    pub(super) fn next_gear(&self) -> Option<f32> {
        self.fps_gears
            .iter()
            .copied()
            .filter(|&g| g > self.current_target_fps + 0.5)
            .reduce(f32::min)
    }

    pub(super) fn prev_gear(&self) -> Option<f32> {
        self.fps_gears
            .iter()
            .copied()
            .filter(|&g| g < self.current_target_fps - 0.5)
            .reduce(f32::max)
    }

    pub(super) fn max_gear(&self) -> f32 {
        self.fps_gears.iter().copied().fold(60.0_f32, f32::max)
    }

    pub(super) fn min_frame_ns(&self) -> u64 {
        (1_000_000_000.0 / self.max_gear()) as u64 / 2
    }

    /// 当前帧率——fps_window 窗口均值（120 帧窗口，60fps 下约 2s），与齿轮决策 avg_fps 同口径供 status.csv 的 fps 列读取，
    /// 窗口无样本（未收到帧/加载退出/切换后已 clear）时返回 None
    pub fn current_fps(&self) -> Option<f32> {
        // 预热期或帧源已过期（stale fallback 中）不算有效帧率，避免把旧统计当当前 fps
        if self.warmup_remaining > 0
            || self
                .last_frame_at
                .is_none_or(|timestamp| timestamp.elapsed() >= FRAME_FEEDBACK_STALE)
            || self.fps_window.count() == 0
        {
            None
        } else {
            Some(self.fps_window.mean())
        }
    }

    pub(super) fn refresh_cached_values(&mut self) {
        self.cached_norm = fps_norm(self.current_target_fps);
        self.cached_budget_ms = 1000.0 / self.current_target_fps.max(1.0);
        self.cached_ema_budget = 1000.0 / (self.current_target_fps - self.fps_margin).max(1.0);
        // 动态适配 PID 系数到当前 target_fps
        self.pid.adapt_to_target_fps(self.current_target_fps);
    }

    /// 基于 CPU 利用率动态偏移 target_fps（每秒采样一次）：util ≤ 0.10 重置偏移（可能在菜单/暂停画面）；util ≤ 0.55 逐步降低 -0.1/s、最多 -3fps；util ≥ 0.
    /// 65 逐步恢复 +0.1/s 至 0效果：GPU bound 场景自动放宽帧率目标，减少无效拉频
    pub(super) fn adjust_target_for_util(&mut self) {
        if self.util_sample_timer.elapsed().as_millis() < 1000 {
            return;
        }
        self.util_sample_timer = Instant::now();

        // jank_cooldown 期间禁止降低 target（只允许恢复），防止刚从卡顿恢复、util 未爬满就又降目标
        let allow_decrease = self.jank_cooldown == 0 && self.jank_streak == 0;

        let util = self.ema_fg_util;
        if util <= 0.10 {
            self.target_fps_offset = 0.0;
        } else if util <= 0.55 && allow_decrease {
            self.target_fps_offset = (self.target_fps_offset - 0.1).max(-3.0);
        } else if util >= 0.65 {
            self.target_fps_offset = (self.target_fps_offset + 0.1).min(0.0);
        }
    }

    /// 获取经过 util 偏移后的有效 target_fps
    #[inline]
    pub(super) fn effective_target_fps(&self) -> f32 {
        (self.current_target_fps + self.target_fps_offset).max(10.0)
    }

    // [game]
    // 公共接口：游戏生命周期

    /// 通知 FAS 当前前台游戏变化
    pub fn set_game(&mut self, _pid: i32, package: &str) {
        self.current_package = package.to_string();
        let profile = self.cfg.per_app_profiles.get(package).cloned();
        if let Some(ref p) = profile {
            if let Some(m) = p.fps_margin {
                self.fps_margin = m;
            }
            if let Some(ref gears) = p.target_fps {
                if !gears.is_empty() {
                    self.fps_gears = gears.clone();
                    if !self
                        .fps_gears
                        .iter()
                        .any(|&g| (g - self.current_target_fps).abs() < 0.5)
                    {
                        self.current_target_fps =
                            self.fps_gears.iter().copied().fold(60.0_f32, f32::max);
                    }
                    self.refresh_cached_values();
                }
            }
            info!(
                "{}",
                t_with_args(
                    "fas-set-game",
                    &fluent_args!(
                        "pkg" => package,
                        "gears" => format!("{:?}", self.fps_gears),
                        "target" => format!("{:.0}", self.current_target_fps)
                    )
                )
            );
        } else {
            warn!(
                "{}",
                t_with_args(
                    "fas-no-profile",
                    &fluent_args!(
                        "pkg" => package,
                        "gears" => format!("{:?}", self.fps_gears)
                    )
                )
            );
        }
        self.active_profile = profile;
    }

    /// 通知 FAS 退出游戏
    pub fn clear_game(&mut self) {
        self.current_package.clear();
        self.active_profile = None;
        self.foreground_max_util = 0.0;
        self.ema_fg_util = 0.0;
        self.core_utils.clear();
        self.target_fps_offset = 0.0;
        // 恢复全局 margin 和 gears
        self.fps_margin = self.cfg.fps_margin;
        self.fps_gears = self.cfg.fps_gears.clone();
    }

    pub fn set_temperature(&mut self, temp: f64) {
        self.current_temperature = temp;
    }
    pub fn set_temp_threshold(&mut self, thresh: f64) {
        self.temp_threshold = thresh;
    }

    pub(super) fn reset_runtime(&mut self) {
        let floor = self.effective_perf_floor();
        let ceil = self.effective_perf_ceil();
        // effective floor 上限 0.45 可能超过低 perf_ceil 配置，min 保证 clamp 边界合法
        self.perf_index = self.cfg.perf_init.clamp(floor.min(ceil), ceil);
        self.ema_actual_ms = 0.0;
        self.pid.reset();
        self.fps_window.clear();
        self.log_counter = 0;
        self.consecutive_normal_frames = 0;
        self.is_loading = false;
        self.loading_frames = 0;
        self.loading_cumulative_ms = 0.0;
        self.loading_normal_tolerance = 0;
        self.post_loading_ignore = 0;
        self.post_loading_downgrade_guard = 0;
        self.upgrade_confirm_frames = 0;
        self.downgrade_confirm_frames = 0;
        self.upgrade_cooldown = 0;
        self.gear_dampen_frames = 0;
        self.consecutive_downgrade_count = 0;
        self.last_downgrade_from_fps = 0.0;
        self.last_downgrade_at = None;
        self.stable_gear_frames = 0;
        self.downgrade_boost_active = false;
        self.downgrade_boost_remaining = 0;
        self.jank_cooldown = 0;
        self.jank_streak = 0;
        self.freq_force_timer = Instant::now();
        self.floor_stuck_frames = 0;
        self.ema_fg_util = 0.0;
        self.post_jank_perf_floor = 0.0;
        self.post_jank_guard_frames = 0;
        self.target_fps_offset = 0.0;
        self.util_sample_timer = Instant::now();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::fas_types::ClusterProfile;
    use crate::utils::FastWriter;

    pub(crate) fn controller_with_policy() -> FasController {
        let mut controller = FasController::new();
        controller.policies.push(PolicyController::new(
            FastWriter::new("/nonexistent/fas-test-max".to_string()),
            FastWriter::new("/nonexistent/fas-test-min".to_string()),
            vec![100, 200, 300, 400, 500],
            0,
            ClusterProfile::default(),
            500,
            None,
            1,
        ));
        controller.policies[0].cpu_ids = vec![0];
        controller.reset_frame_feedback();
        controller
    }

    #[test]
    fn warmup_counts_only_valid_frames_and_defers_pid() {
        let mut controller = controller_with_policy();
        controller.update_frame(0);
        controller.update_frame(1);
        controller.update_frame(u64::MAX);
        assert_eq!(controller.warmup_remaining, FRAME_FEEDBACK_WARMUP_FRAMES);
        for _ in 0..FRAME_FEEDBACK_WARMUP_FRAMES {
            controller.update_frame(16_666_667);
        }
        assert_eq!(controller.warmup_remaining, 0);
        assert_eq!(controller.ema_actual_ms, 0.0);
        assert_eq!(
            controller.fps_window.count(),
            FRAME_FEEDBACK_WARMUP_FRAMES as usize
        );
    }

    #[test]
    fn suspended_feedback_does_not_consume_frames() {
        let mut controller = controller_with_policy();
        controller.set_frame_feedback_enabled(false);
        controller.update_frame(16_666_667);
        assert_eq!(controller.warmup_remaining, FRAME_FEEDBACK_WARMUP_FRAMES);
        assert_eq!(controller.current_fps(), None);
    }

    #[test]
    fn load_only_applies_lower_frequency_without_foreground_util() {
        let mut controller = controller_with_policy();
        controller.set_frame_feedback_enabled(false);
        controller.update_cpu_util(1.0);
        controller.update_core_utils(&[0.1]);
        controller.update_load_control();
        assert!(controller.policies[0].current_freq < 500);
        assert!(controller.perf_index <= controller.effective_perf_floor());
    }

    #[test]
    fn load_only_temperature_enforces_cap_below_floor() {
        let mut controller = controller_with_policy();
        controller.set_frame_feedback_enabled(false);
        controller.cfg.core_temp_throttle_perf = 0.1;
        controller.set_temp_threshold(40.0);
        controller.set_temperature(41.0);
        controller.update_core_utils(&[1.0]);
        controller.update_load_control();
        assert!(controller.thermal_hold);
        assert!(controller.perf_index <= 0.1);
        assert_eq!(controller.policies[0].current_freq, 100);
    }

    #[test]
    fn stale_frame_stream_returns_to_load_control() {
        let mut controller = controller_with_policy();
        controller.warmup_remaining = 0;
        controller.last_frame_at = Some(Instant::now() - std::time::Duration::from_secs(1));
        controller.perf_index = 1.0;
        controller.update_core_utils(&[0.1]);
        controller.update_load_control();
        assert!(controller.policies[0].current_freq < 500);
    }

    #[test]
    fn normal_feedback_load_event_does_not_touch_frequency_path() {
        let mut controller = controller_with_policy();
        controller.warmup_remaining = 0;
        controller.last_frame_at = Some(Instant::now());
        controller.perf_index = 0.7;
        controller.policies[0].freq_hold_frames = 2;
        controller.update_core_utils(&[0.9]);
        controller.update_load_control();
        // 正常帧反馈路径不得 apply_freqs：freq_hold_frames 不被递减、perf 不被负载改写
        assert_eq!(controller.policies[0].freq_hold_frames, 2);
        assert_eq!(controller.perf_index, 0.7);
    }

    #[test]
    fn load_only_floor_uses_config_base_without_target_bonus() {
        let mut controller = controller_with_policy();
        controller.set_frame_feedback_enabled(false);
        controller.fps_gears = vec![60.0, 144.0];
        controller.current_target_fps = 144.0;
        controller.cfg.perf_floor = 0.10;
        controller.cfg.perf_ceil = 0.90;
        controller.update_core_utils(&[0.0]);
        controller.update_load_control();
        // 地板用配置 base（0.10），不叠加 144fps 的 target_fps bonus（否则会是 0.35）
        assert!(controller.effective_perf_floor() > controller.cfg.perf_floor);
        assert!((controller.perf_index - 0.10).abs() < 1e-6);
    }

    #[test]
    fn current_fps_hidden_during_warmup_and_stale() {
        let mut controller = controller_with_policy();
        controller.fps_window.push(120.0);
        assert_eq!(controller.current_fps(), None);
        controller.warmup_remaining = 0;
        controller.last_frame_at = Some(Instant::now());
        assert!(controller.current_fps().is_some());
        controller.last_frame_at = Some(Instant::now() - std::time::Duration::from_secs(1));
        assert_eq!(controller.current_fps(), None);
    }

    #[test]
    fn stale_fallback_discards_old_stats_and_restarts_warmup() {
        let mut controller = controller_with_policy();
        controller.warmup_remaining = 0;
        controller.jank_streak = 4;
        controller.ema_actual_ms = 33.0;
        controller.fps_window.push(999.0);
        controller.last_frame_at = Some(Instant::now() - std::time::Duration::from_secs(1));
        controller.update_frame(16_666_667);
        // stale 后首个有效帧应重置为预热，且不把旧 999 fps 并进新窗口
        assert_eq!(
            controller.warmup_remaining,
            FRAME_FEEDBACK_WARMUP_FRAMES - 1
        );
        assert_eq!(controller.fps_window.count(), 1);
        assert_eq!(controller.jank_streak, 0);
        assert_eq!(controller.ema_actual_ms, 0.0);
    }

    #[test]
    fn consecutive_valid_frames_advance_warmup_without_restart() {
        let mut controller = controller_with_policy();
        controller.warmup_remaining = 0;
        controller.last_frame_at = Some(Instant::now() - std::time::Duration::from_secs(1));
        controller.update_frame(16_666_667);
        controller.update_frame(16_666_667);
        assert_eq!(
            controller.warmup_remaining,
            FRAME_FEEDBACK_WARMUP_FRAMES - 2
        );
    }

    #[test]
    fn resume_clears_gear_jank_and_frame_history() {
        let mut controller = controller_with_policy();
        controller.current_target_fps = 30.0;
        controller.fps_gears = vec![60.0, 120.0];
        controller.jank_streak = 5;
        controller.ema_actual_ms = 40.0;
        controller.fps_window.push(30.0);
        controller.set_frame_feedback_enabled(false);
        controller.set_frame_feedback_enabled(true);
        assert_eq!(controller.current_fps(), None);
        assert_eq!(controller.jank_streak, 0);
        assert_eq!(controller.ema_actual_ms, 0.0);
        assert_eq!(controller.current_target_fps, controller.max_gear());
        assert_eq!(controller.warmup_remaining, FRAME_FEEDBACK_WARMUP_FRAMES);
    }

    #[test]
    fn resume_keeps_configured_gear_within_gear_set() {
        let mut controller = controller_with_policy();
        controller.fps_gears = vec![30.0, 60.0, 120.0];
        controller.current_target_fps = 30.0;
        controller.set_frame_feedback_enabled(false);
        controller.set_frame_feedback_enabled(true);
        assert_eq!(controller.current_target_fps, 30.0);
        assert_eq!(controller.warmup_remaining, FRAME_FEEDBACK_WARMUP_FRAMES);
    }

    #[test]
    fn unchanged_feedback_does_not_restart_warmup() {
        let mut controller = controller_with_policy();
        controller.update_frame(16_666_667);
        controller.set_frame_feedback_enabled(true);
        assert_eq!(
            controller.warmup_remaining,
            FRAME_FEEDBACK_WARMUP_FRAMES - 1
        );
        assert_eq!(controller.fps_window.count(), 1);
    }

    #[test]
    fn load_only_uses_each_clusters_demand() {
        let mut controller = controller_with_policy();
        controller.policies.push(PolicyController::new(
            FastWriter::new("/nonexistent/fas-test-max".to_string()),
            FastWriter::new("/nonexistent/fas-test-min".to_string()),
            vec![100, 200, 300, 400, 500],
            1,
            ClusterProfile::default(),
            500,
            None,
            1,
        ));
        controller.policies[1].cpu_ids = vec![1];
        controller.set_frame_feedback_enabled(false);
        controller.update_core_utils(&[0.1, 1.0]);
        controller.update_load_control();
        assert!(controller.policies[0].current_freq < controller.policies[1].current_freq);
        assert_eq!(controller.policies[1].current_freq, 500);
    }
}
