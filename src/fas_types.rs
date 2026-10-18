//! fas_types.rs: [pid] [cluster] [per_app] [rules]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// [pid]
// PID 系数（60fps 基准，运行时按 target_fps 缩放）：kp 按 target_fps/60 线性、
// ki 按 sqrt(target_fps/60)（防高刷积分饱和）、kd 按 (target_fps/60)^0.3（高刷噪声大）

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PidCoefficients {
    #[serde(default = "default_kp")]
    pub kp: f32,
    #[serde(default = "default_ki")]
    pub ki: f32,
    #[serde(default = "default_kd")]
    pub kd: f32,
}
fn default_kp() -> f32 {
    0.050
}
fn default_ki() -> f32 {
    0.010
}
fn default_kd() -> f32 {
    0.006
}
impl Default for PidCoefficients {
    fn default() -> Self {
        Self {
            kp: default_kp(),
            ki: default_ki(),
            kd: default_kd(),
        }
    }
}

// [cluster]

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClusterProfile {
    #[serde(default = "default_capacity_weight")]
    pub capacity_weight: f32,
}
fn default_capacity_weight() -> f32 {
    1.0
}
impl Default for ClusterProfile {
    fn default() -> Self {
        Self {
            capacity_weight: 1.0,
        }
    }
}
pub fn default_cluster_profiles() -> Vec<ClusterProfile> {
    vec![
        ClusterProfile {
            capacity_weight: 1.0,
        },
        ClusterProfile {
            capacity_weight: 1.5,
        },
        ClusterProfile {
            capacity_weight: 2.5,
        },
        ClusterProfile {
            capacity_weight: 3.5,
        },
    ]
}

// [per_app]

/// 每个游戏的配置档案：只需指定 target_fps 数组，运行时按实际帧率匹配最近档位
/// ```yaml
/// per_app_profiles:
/// "com.miHoYo.GenshinImpact": { target_fps: [30, 60], fps_margin: 4.0 }
/// "com.tencent.tmgp.sgame": { target_fps: [60, 90, 120], fps_margin: 3.0 }
/// ```
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PerAppProfile {
    /// 目标帧率数组（如 [30, 60] 表示可能以 30 或 60fps 渲染），运行时匹配最近档位
    #[serde(default)]
    pub target_fps: Option<Vec<f32>>,

    /// 该应用的帧率余量（覆盖全局 fps_margin）
    #[serde(default)]
    pub fps_margin: Option<f32>,
}

// [rules]

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FasRulesConfig {
    #[serde(default = "default_fps_gears")]
    pub fps_gears: Vec<f32>,
    #[serde(default = "default_fps_margin")]
    pub fps_margin: f32,
    #[serde(default)]
    pub pid: PidCoefficients,
    #[serde(default = "default_cluster_profiles")]
    pub cluster_profiles: Vec<ClusterProfile>,
    #[serde(default = "d_auto_cap")]
    pub auto_capacity_weight: bool,

    #[serde(default = "d_perf_floor")]
    pub perf_floor: f32,
    #[serde(default = "d_perf_ceil")]
    pub perf_ceil: f32,
    #[serde(default = "d_perf_init")]
    pub perf_init: f32,
    #[serde(default = "d_perf_cold")]
    pub perf_cold_boot: f32,
    #[serde(default = "d_hysteresis")]
    pub freq_hysteresis: f32,

    #[serde(default = "d_heavy_ms")]
    pub heavy_frame_threshold_ms: f32,
    #[serde(default = "d_load_ms")]
    pub loading_cumulative_ms: f32,
    #[serde(default = "d_load_tol")]
    pub loading_normal_tolerance: u32,
    #[serde(default = "d_load_pf")]
    pub loading_perf_floor: f32,
    #[serde(default = "d_load_pc")]
    pub loading_perf_ceiling: f32,

    #[serde(default = "d_post_ign")]
    pub post_loading_ignore_frames: u32,
    #[serde(default = "d_post_perf")]
    pub post_loading_perf: f32,
    #[serde(default = "d_post_guard")]
    pub post_loading_downgrade_guard: u32,

    #[serde(default = "d_up_confirm")]
    pub upgrade_confirm_frames: u32,
    #[serde(default = "d_dn_confirm")]
    pub downgrade_confirm_frames: u32,
    #[serde(default = "d_up_cd")]
    pub upgrade_cooldown_after_downgrade: u32,
    #[serde(default = "d_dampen")]
    pub gear_dampen_frames: u32,

    #[serde(default = "d_boost_inc")]
    pub downgrade_boost_perf_inc: f32,
    #[serde(default = "d_boost_dur")]
    pub downgrade_boost_duration: u32,

    #[serde(default = "d_sd_thresh")]
    pub steady_decay_frame_threshold: u32,
    #[serde(default = "d_sd_perf")]
    pub steady_decay_perf_threshold: f32,
    #[serde(default = "d_sd_max")]
    pub steady_decay_max_step: f32,
    #[serde(default = "d_sd_min")]
    pub steady_decay_min_step: f32,

    /// 失去白名单前台后的延迟退出秒数：期间切回白名单应用无缝续期（缺省 15）
    #[serde(default = "d_fas_exit_delay")]
    pub deactivate_delay_secs: u32,

    #[serde(default = "d_jank_cd")]
    pub jank_cooldown_frames: u32,

    #[serde(default = "d_max_inc_d")]
    pub max_inc_damped: f32,
    #[serde(default = "d_max_inc_n")]
    pub max_inc_normal: f32,
    #[serde(default = "d_damped_cap")]
    pub damped_perf_cap: f32,

    #[serde(default = "d_switch_ms")]
    pub app_switch_gap_ms: f32,
    #[serde(default = "d_switch_perf")]
    pub app_switch_resume_perf: f32,

    /// 防篡改强制重写间隔（秒，最小 1，默认 30）：每 N 秒把锁频值无条件重写一遍，收敛被改写/压制的 scaling_min_freq / scaling_max_freq
    #[serde(default = "d_force_int")]
    pub freq_force_reapply_interval: u32,
    #[serde(default = "d_max_frame")]
    pub fixed_max_frame_ms: f32,
    #[serde(default = "d_cold_ms")]
    pub cold_boot_ms: u64,

    #[serde(default = "d_verify_interval")]
    pub verify_freq_interval_secs: u32,

    #[serde(default)]
    pub per_app_profiles: HashMap<String, PerAppProfile>,

    #[serde(default)]
    pub per_app_margins: HashMap<String, f32>,

    /// 温度降频阈值（℃），0 = 禁用
    #[serde(default = "d_temp_thresh")]
    pub core_temp_threshold: f64,

    /// 温度降频时的最低 perf
    #[serde(default = "d_temp_perf")]
    pub core_temp_throttle_perf: f32,

    /// 接管期间写入 `/proc/sys/kernel/sched_migration_cost_ns`（None = 不动），退出按快照恢复
    #[serde(default)]
    pub migration_cost_ns: Option<u64>,

    /// CPU 负载辅助：前台线程利用率封顶的除数（越小越激进）
    #[serde(default = "d_util_cap_divisor")]
    pub util_cap_divisor: f32,
}

pub fn default_fps_gears() -> Vec<f32> {
    vec![30.0, 60.0, 90.0, 120.0, 144.0]
}
pub fn default_fps_margin() -> f32 {
    3.0
}
fn d_auto_cap() -> bool {
    true
}
fn d_perf_floor() -> f32 {
    0.22
}
fn d_perf_ceil() -> f32 {
    1.0
}
fn d_perf_init() -> f32 {
    0.45
}
fn d_perf_cold() -> f32 {
    0.85
}
pub fn d_hysteresis() -> f32 {
    0.015
}
pub fn d_heavy_ms() -> f32 {
    150.0
}
pub fn d_load_ms() -> f32 {
    2500.0
}
fn d_load_tol() -> u32 {
    3
}
fn d_load_pf() -> f32 {
    0.60
}
fn d_load_pc() -> f32 {
    0.70
}
pub fn d_post_ign() -> u32 {
    5
}
pub fn d_post_perf() -> f32 {
    0.65
}
fn d_post_guard() -> u32 {
    90
}
fn d_up_confirm() -> u32 {
    // 快升慢降（非对称滞回）：升档确认 60 → 45 帧（0.75s@60fps），升档要快；
    // 慢降由 gear_state.rs 升档后最短驻留 + 现有降档确认保证
    45
}
fn d_dn_confirm() -> u32 {
    90
}
fn d_up_cd() -> u32 {
    90
}
fn d_dampen() -> u32 {
    60
}
fn d_boost_inc() -> f32 {
    0.18
}
fn d_boost_dur() -> u32 {
    45
}
fn d_sd_thresh() -> u32 {
    75
}
fn d_sd_perf() -> f32 {
    0.70
}
fn d_sd_max() -> f32 {
    0.022
}
fn d_sd_min() -> f32 {
    0.004
}
fn d_fas_exit_delay() -> u32 {
    // 15s → 3s：15s 本意是白名单应用间切换无缝续期，但非白名单前台接管时 mode 残留 fas
    // 造成归因污染与功耗虚高，3s 足以覆盖同引擎快速切换
    3
}
fn d_jank_cd() -> u32 {
    15
}
fn d_max_inc_d() -> f32 {
    0.045
}
fn d_max_inc_n() -> f32 {
    0.075
}
fn d_damped_cap() -> f32 {
    0.92
}
fn d_switch_ms() -> f32 {
    3000.0
}
fn d_switch_perf() -> f32 {
    0.60
}
/// 防篡改强制重写间隔默认 30，单位为秒
fn d_force_int() -> u32 {
    30
}
fn d_max_frame() -> f32 {
    500.0
}
fn d_cold_ms() -> u64 {
    3500
}
/// 写频后校验间隔（秒）：更快发现锁频值被压低（内核 thermal cap / QoS 收窄），仅写频后触发一次
fn d_verify_interval() -> u32 {
    1
}
fn d_temp_thresh() -> f64 {
    0.0
}
fn d_temp_perf() -> f32 {
    0.70
}
fn d_util_cap_divisor() -> f32 {
    0.45
}

impl FasRulesConfig {
    /// 校验并规范化配置：非有限值回退默认，perf/步长交叉约束保证 clamp 永不 panic，fps_gears 过滤非法值
    pub fn normalize(&mut self) {
        // [log] 非法值回退默认原本完全静默，排障只看到行为不对而无线索
        // 这里累计被修正的字段名，末尾一次性 warn（仅真有修正时打，稳态不出现）
        let mut fixed: Vec<&'static str> = Vec::new();
        macro_rules! finite_or_default {
            ($field:expr, $default:expr) => {
                if !($field).is_finite() {
                    $field = $default;
                    fixed.push(stringify!($field));
                }
            };
        }
        finite_or_default!(self.perf_floor, d_perf_floor());
        finite_or_default!(self.perf_ceil, d_perf_ceil());
        finite_or_default!(self.perf_init, d_perf_init());
        finite_or_default!(self.perf_cold_boot, d_perf_cold());
        finite_or_default!(self.steady_decay_max_step, d_sd_max());
        finite_or_default!(self.steady_decay_min_step, d_sd_min());
        finite_or_default!(self.freq_hysteresis, d_hysteresis());
        finite_or_default!(self.max_inc_normal, d_max_inc_n());
        finite_or_default!(self.max_inc_damped, d_max_inc_d());
        finite_or_default!(self.damped_perf_cap, d_damped_cap());
        finite_or_default!(self.pid.kp, default_kp());
        finite_or_default!(self.pid.ki, default_ki());
        finite_or_default!(self.pid.kd, default_kd());

        let gears_before = self.fps_gears.len();
        self.fps_gears.retain(|&g| g.is_finite() && g > 0.0);
        if self.fps_gears.len() != gears_before {
            fixed.push("fps_gears");
        }
        if self.fps_gears.is_empty() {
            self.fps_gears = default_fps_gears();
        }
        // per-app 的 target_fps 同理过滤（set_game 直接使用，绕过顶层过滤）
        for profile in self.per_app_profiles.values_mut() {
            if let Some(gears) = &mut profile.target_fps {
                let before = gears.len();
                gears.retain(|&g| g.is_finite() && g > 0.0);
                if gears.len() != before {
                    fixed.push("per_app_profiles.target_fps");
                }
                if gears.is_empty() {
                    profile.target_fps = None;
                }
            }
        }

        // perf 交叉约束（顺序保证 clamp 边界合法）
        let (floor0, ceil0) = (self.perf_floor, self.perf_ceil);
        self.perf_floor = self.perf_floor.clamp(0.0, 1.0);
        self.perf_ceil = self.perf_ceil.clamp(0.0, 1.0);
        if self.perf_floor > self.perf_ceil {
            self.perf_floor = self.perf_ceil;
        }
        if self.perf_floor != floor0 || self.perf_ceil != ceil0 {
            fixed.push("perf_floor/perf_ceil");
        }
        self.perf_init = self.perf_init.clamp(self.perf_floor, self.perf_ceil);
        self.perf_cold_boot = self.perf_cold_boot.clamp(self.perf_floor, self.perf_ceil);
        finite_or_default!(self.loading_perf_floor, d_load_pf());
        finite_or_default!(self.loading_perf_ceiling, d_load_pc());
        finite_or_default!(self.post_loading_perf, d_post_perf());
        finite_or_default!(self.core_temp_throttle_perf, d_temp_perf());
        finite_or_default!(self.util_cap_divisor, d_util_cap_divisor());
        finite_or_default!(self.app_switch_resume_perf, d_switch_perf());
        // loading 上下限拍平，防止 frame_pipeline clamp(min>max) panic
        if self.loading_perf_floor > self.loading_perf_ceiling {
            self.loading_perf_floor = self.loading_perf_ceiling;
        }
        self.loading_perf_floor = self
            .loading_perf_floor
            .clamp(self.perf_floor, self.perf_ceil);
        self.loading_perf_ceiling = self
            .loading_perf_ceiling
            .clamp(self.perf_floor, self.perf_ceil);
        self.post_loading_perf = self
            .post_loading_perf
            .clamp(self.perf_floor, self.perf_ceil);
        // [钳制留痕] damped_perf_cap 硬钳制会改变行为，与 perf_floor/perf_ceil 一样 push 进 fixed
        // warn 只在 !fixed.is_empty() 时打，漏 push 则配置被改却无线索
        let damped_perf_cap0 = self.damped_perf_cap;
        self.damped_perf_cap = self.damped_perf_cap.clamp(self.perf_floor, self.perf_ceil);
        if self.damped_perf_cap != damped_perf_cap0 {
            fixed.push("damped_perf_cap");
        }
        self.app_switch_resume_perf = self
            .app_switch_resume_perf
            .clamp(self.perf_floor, self.perf_ceil);

        // steady_decay 步长约束：min <= max*0.6（decay_scale 最坏 0.6），否则 clamp 边界反转 panic
        // [钳制留痕] min_step 两步可改值（负数归零、超上限拍平），任一生效即 push（dedup 兜底重复）
        let steady_decay_min_step0 = self.steady_decay_min_step;
        self.steady_decay_max_step = self.steady_decay_max_step.max(0.0);
        self.steady_decay_min_step = self.steady_decay_min_step.max(0.0);
        if self.steady_decay_min_step > self.steady_decay_max_step * 0.6 {
            self.steady_decay_min_step = self.steady_decay_max_step * 0.6;
        }
        if self.steady_decay_min_step != steady_decay_min_step0 {
            fixed.push("steady_decay_min_step");
        }
        // 延迟退出：1s 下限防抖，10 分钟上限防呆
        // [钳制留痕] 硬钳制改变行为需进 fixed
        let deactivate_delay_secs0 = self.deactivate_delay_secs;
        self.deactivate_delay_secs = self.deactivate_delay_secs.clamp(1, 600);
        if self.deactivate_delay_secs != deactivate_delay_secs0 {
            fixed.push("deactivate_delay_secs");
        }
        // 防篡改强制重写间隔最小 1：时间基准下 0 会令「已到期」恒真（每帧强制重写），必须钳到 ≥ 1
        self.freq_force_reapply_interval = self.freq_force_reapply_interval.max(1);

        // [log] 仅在真有修正时打一条 warn，字段名列表直接给出被改了什么；normalize 在启动/热重载时调用
        if !fixed.is_empty() {
            fixed.dedup();
            log::warn!(
                "[FAS] 配置非法值已回退默认/钳制: {}", // 字段名列表
                fixed.join(", ")
            );
        }
    }

    /// 将旧的 per_app_margins 迁移到 per_app_profiles
    pub fn migrate_legacy_margins(&mut self) {
        for (pkg, margin) in self.per_app_margins.drain() {
            self.per_app_profiles.entry(pkg).or_default().fps_margin = Some(margin);
        }
    }
}

impl Default for FasRulesConfig {
    fn default() -> Self {
        Self {
            fps_gears: default_fps_gears(),
            fps_margin: default_fps_margin(),
            pid: PidCoefficients::default(),
            cluster_profiles: default_cluster_profiles(),
            auto_capacity_weight: d_auto_cap(),
            perf_floor: d_perf_floor(),
            perf_ceil: d_perf_ceil(),
            perf_init: d_perf_init(),
            perf_cold_boot: d_perf_cold(),
            freq_hysteresis: d_hysteresis(),
            heavy_frame_threshold_ms: d_heavy_ms(),
            loading_cumulative_ms: d_load_ms(),
            loading_normal_tolerance: d_load_tol(),
            loading_perf_floor: d_load_pf(),
            loading_perf_ceiling: d_load_pc(),
            post_loading_ignore_frames: d_post_ign(),
            post_loading_perf: d_post_perf(),
            post_loading_downgrade_guard: d_post_guard(),
            upgrade_confirm_frames: d_up_confirm(),
            downgrade_confirm_frames: d_dn_confirm(),
            upgrade_cooldown_after_downgrade: d_up_cd(),
            gear_dampen_frames: d_dampen(),
            downgrade_boost_perf_inc: d_boost_inc(),
            downgrade_boost_duration: d_boost_dur(),
            steady_decay_frame_threshold: d_sd_thresh(),
            steady_decay_perf_threshold: d_sd_perf(),
            steady_decay_max_step: d_sd_max(),
            steady_decay_min_step: d_sd_min(),
            deactivate_delay_secs: d_fas_exit_delay(),
            jank_cooldown_frames: d_jank_cd(),
            max_inc_damped: d_max_inc_d(),
            max_inc_normal: d_max_inc_n(),
            damped_perf_cap: d_damped_cap(),
            app_switch_gap_ms: d_switch_ms(),
            app_switch_resume_perf: d_switch_perf(),
            freq_force_reapply_interval: d_force_int(),
            fixed_max_frame_ms: d_max_frame(),
            cold_boot_ms: d_cold_ms(),
            verify_freq_interval_secs: d_verify_interval(),
            per_app_profiles: HashMap::new(),
            per_app_margins: HashMap::new(),
            core_temp_threshold: d_temp_thresh(),
            core_temp_throttle_perf: d_temp_perf(),
            migration_cost_ns: None,
            util_cap_divisor: d_util_cap_divisor(),
        }
    }
}
