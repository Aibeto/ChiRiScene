//! config.rs: [meta] [clg_config] [clg_normalize] [mode_io] [toggles] [ak_config] [thermal_config]
//! [affinity_config] [corectl_config] [config_root] [config_impl]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::fluent_args;
use crate::i18n::t_with_args;

/// 全局元信息段（对应 meta.yaml 顶层字段）
// [meta]

/// 把 meta 覆盖值应用到生效配置：**None = 文件里没写该键 → 不改动**（沿用内嵌默认），语义同 common.rs::ExternalMetaOverrides
fn apply_meta_overrides(meta: &mut Meta, o: &crate::common::ExternalMetaOverrides) {
    if let Some(v) = &o.loglevel {
        meta.loglevel = v.clone();
    }
    if let Some(v) = &o.language {
        meta.language = v.clone();
    }
    if let Some(v) = o.dev_record {
        meta.dev_record = v;
    }
    // devimp_top_n 无 meta.yaml 模板载体（模板刻意不写、走代码缺省）：不能沿用derive Default 的 0，必须回缺省 10（0/超限最终由 Meta::normalize 钳到 1.
    // =64）
    meta.devimp_top_n = o.devimp_top_n.unwrap_or_else(d_devimp_top_n);
    if let Some(v) = o.fas_enabled {
        meta.fas_enabled = v;
    }
    if let Some(v) = o.scenemode_enabled {
        meta.scenemode_enabled = v;
    }
    if let Some(v) = o.thread_bind {
        meta.thread_bind = v;
    }
    if let Some(v) = o.powerbase_enabled {
        meta.powerbase_enabled = v;
    }
    if let Some(v) = o.power_avg {
        meta.power_avg = v;
    }
    if let Some(v) = o.notify {
        meta.notify = v;
    }
    if let Some(v) = o.oplus_chg {
        meta.oplus_chg = v;
    }
    if let Some(v) = o.oplus_dual_cell {
        meta.oplus_dual_cell = v;
    }
    if let Some(v) = o.voltage_double {
        meta.voltage_double = v;
    }
    if let Some(v) = o.current_double {
        meta.current_double = v;
    }
    if let Some(v) = o.voltage_divisor {
        meta.voltage_divisor = v;
    }
    if let Some(v) = o.current_divisor {
        meta.current_divisor = v;
    }
    if let Some(v) = o.power_max_w {
        meta.power_max_w = v;
    }
    if let Some(v) = o.screen_off_value {
        meta.screen_off_value = v;
    }
    if let Some(v) = o.nofix {
        meta.nofix = v;
    }
}
#[derive(Debug, Deserialize, Default)]
pub struct Meta {
    /// 日志级别：DEBUG / INFO / WARN / ERROR，热重载时即时生效
    #[serde(default = "default_loglevel", alias = "Loglevel")]
    pub loglevel: String,

    /// 守护进程日志语言：en / zh，改动后自动加载对应 .ftl
    #[serde(default = "default_language", alias = "Language")]
    pub language: String,

    /// 开发记录开关：true 时向 devimp/ 写双诊断文件——main_<前台包名>_<MMDD-HHmmss>.log 主诊断（tick/snap/event 行）与 aff_<MMDD-HHmmss>
    /// log 线程流（@A 动作帧 + @S 快照帧），供离线分析改善调度。 meta 段允许外部修改的字段之一（WebUI 开关，热重载生效）
    #[serde(default, alias = "DevRecord")]
    pub dev_record: bool,

    /// 快照 top-N 进程数（meta.yaml `devimp_top_n`，缺省 10）：aff_* 的 `@S` 每秒快照帧按 util 降序落盘的进程行数（前台树与被管进程不受截断）；钳到 1..=64（0 → 1）
    #[serde(default = "d_devimp_top_n", alias = "DevimpTopN")]
    pub devimp_top_n: usize,

    /// FAS 帧感知调度总开关（缺省 true）：关闭后 fas_available() 恒为 false——不再产生 fas 模式、FAS 监测线程不启动、运行中实例立即注销meta 段可外部修改（热重载生效）
    #[serde(default = "crate::utils::default_true", alias = "FasEnabled")]
    pub fas_enabled: bool,

    /// 息屏场景模式总开关（缺省 true）：关闭后息屏不进入 scenemode（已激活的下个tick 退出并恢复 affinity/core_ctl 快照 + doze 配置）meta 段可外部修改（热重载生效）
    #[serde(default = "crate::utils::default_true", alias = "ScenemodeEnabled")]
    pub scenemode_enabled: bool,

    /// 线程摆放总闸（meta.yaml `thread_bind`，缺省 true）：**实验室 frozen 专用机制**——用户侧开关已移除（线程功能默认常开），
    /// 仅 frozen 实验室模式写 false 以交还线程亲和/绑核与 core_ctl；与 `Affinity.enabled` / `CoreCtl.enabled` 取「与」（Config::load 合并，
    /// 下游只看子开关）
    #[serde(default = "crate::utils::default_true", alias = "ThreadBind")]
    pub thread_bind: bool,

    /// PowerBase 总开关（meta.yaml `powerbase_enabled`，**高级设置，默认关闭**）：
    /// 开启后 reduce / default / boost 原由 CLG 接管的场合改由 PowerBase 接管——以
    /// **放电功耗**为指标调频（CLG 只看利用率）；FAS / 场景特调 / DOWN 停摆 / 实验室判据不受影响
    #[serde(default, alias = "PowerbaseEnabled")]
    pub powerbase_enabled: bool,

    /// 功耗口径开关（meta.yaml `power_avg`，默认 false = 参考值）：控制 1s 状态采样写模块根 `PowerAVG.chr` 的口径——false 写参考值（(旧值 × 10 + 新值)
    /// / 11 递推，偏历史，含息屏样本）；true 写累计平均（(旧值 × 次数 + 新值) / (次数 + 1)，等权全史，仅亮屏）均只取放电样本；热重载即时生效，仅 ChiRi 有 1s 采样
    #[serde(default, alias = "PowerAvg")]
    pub power_avg: bool,

    /// 息屏判定值（meta.yaml `screen_off_value`，缺省 1）：debug.tracing.screen_state 属性等于该值视为息屏（高级设置；安装脚本按安装期实测自动校正）；Config::
    /// load 同步到 common::SCREEN_OFF_VALUE
    #[serde(default = "default_screen_off_value", alias = "ScreenOffValue")]
    pub screen_off_value: u32,

    /// 耗电读数满量程（W，meta.yaml `power_max_w`，可选，默认 12）：只影响 WebUI 状态页仪表盘的进度换算，不参与任何调度决策
    #[serde(default = "crate::utils::default_power_max_w", alias = "PowerMaxW")]
    pub power_max_w: f32,

    /// 常驻状态通知开关（meta.yaml `notify`，默认 true）：false = 不投递通知，并撤销已投递的那条（daemon 自己还在跑，有能力清理）。热重载即时生效
    #[serde(default = "crate::utils::default_true", alias = "Notify")]
    pub notify: bool,

    /// 电池读数：OPlus 私有节点优先（meta.yaml `oplus_chg`，默认 false）——读 `/sys/class/oplus_chg/battery/bcc_parms`（随采样刷新），
    /// 读不到回退标准节点
    #[serde(default, alias = "OplusChg")]
    pub oplus_chg: bool,

    /// OPlus 双电芯（`oplus_dual_cell`，默认 false）：私有节点电压取两节平均、电流 ×2（并联）
    #[serde(default, alias = "OplusDualCell")]
    pub oplus_dual_cell: bool,

    /// 倍电压（`voltage_double`，默认 false）：标准节点路径电压 ×2，与私有开关互斥
    #[serde(default, alias = "VoltageDouble")]
    pub voltage_double: bool,

    /// 倍电流（`current_double`，默认 false）：标准节点路径电流 ×2，互斥同上
    #[serde(default, alias = "CurrentDouble")]
    pub current_double: bool,

    /// 电压校准除数（默认 1000000，须 > 0）：节点原始值 ÷ 该值 = V，读取层不做换算。缺省 1000000 = 标准 Android ABI µV 口径；OPlus 私有节点报 mV，
    /// 安装脚本会写入 1000
    #[serde(
        default = "crate::utils::default_unit_divisor",
        alias = "VoltageDivisor"
    )]
    pub voltage_divisor: f32,

    /// 电流校准除数（默认 1000000，须 > 0）：节点原始值 ÷ 该值 = **安培**（口径同电压）。batt_power_w 按安培 × 伏特得瓦——口径必须与本注释一致
    #[serde(
        default = "crate::utils::default_unit_divisor",
        alias = "CurrentDivisor"
    )]
    pub current_divisor: f32,

    /// 「不改」开关（meta.yaml 可选字段 `nofix`，默认 false，默认不写进配置）：
    /// true = 启动时跳过二进制对外部文件的覆盖类操作（webui 资产还原 + meta.yaml 快照自愈，见 common.rs::read_nofix_flag）
    #[serde(default, alias = "NoFix")]
    pub nofix: bool,
}

// Meta 缺省值：meta.yaml 省略该字段时回退到此处
fn default_loglevel() -> String {
    "INFO".to_string()
}
fn default_language() -> String {
    "en".to_string()
}
/// 快照 top-N 进程数缺省值（`devimp_top_n`）：10 个进程 / 每秒 @S 帧
fn d_devimp_top_n() -> usize {
    10
}
/// 息屏判定值缺省值（meta.yaml `screen_off_value`）：debug.tracing.screen_state = 1 视为息屏
fn default_screen_off_value() -> u32 {
    1
}

impl Meta {
    /// 校验并规范化 meta 段（与 Config::load 各段 normalize 统一口径）：`devimp_top_n`钳到 1..=64（0 → 1、>64 → 64），不判整个文件非法
    pub fn normalize(&mut self) {
        self.devimp_top_n = self.devimp_top_n.clamp(1, 64);
    }
}

// [clg_config]
/// CLG（CPU Load Governor）调频参数。所有性能比/阈值均为 0.0~1.0 的相对值，`perf_init/perf_floor/perf_ceil` 再换算成最近频率档位
#[derive(Debug, Deserialize, Clone)]
pub struct CpuLoadGovernorConfig {
    /// CLG 总开关：false 时不接管 CPU，也不做任何频率写入
    #[serde(default = "crate::utils::default_true")]
    pub enabled: bool,
    /// 升频阈值：cluster 最大 util >= 此值时按全速升频（配合 headroom_factor 放大）
    #[serde(default = "d_clg_up_thresh")]
    pub up_threshold: f32,
    /// 降频阈值：util 跌破此值进入降频区间
    #[serde(default = "d_clg_down_thresh")]
    pub down_threshold: f32,
    /// 升频平滑系数：每 tick 目标性能只逼近该比例，越大响应越快，越小越省电
    #[serde(default = "d_clg_smooth_up")]
    pub smoothing_up: f32,
    /// 决策负载平滑（EMA 系数，1.0=关闭，语义同 tuned 的 util_smoothing）：不平滑时抖动负载会导致决策方向反复翻转、scaling_max_freq 高频改写日常/省电档建议 0.5，
    /// 性能档保持 1.0注意：0 = 负载冻结在首个采样值、决策不再跟随负载，勿设 0
    #[serde(default = "d_clg_util_smooth")]
    pub util_smoothing: f32,
    /// 降频速率限制：必须连续满足 down_wait >= 该 tick 数才执行一次降频。降频本身为“直接降频”一步到位（不做平滑渐变），该值仅作防抖
    #[serde(default = "d_clg_down_rate")]
    pub down_rate_limit_ticks: u32,
    /// 升频速率限制：必须连续满足 up_wait >= 该 tick 数才执行一次升频
    #[serde(default = "d_clg_up_rate")]
    pub up_rate_limit_ticks: u32,
    /// 性能余量：util 达 up_threshold 后，目标性能放大到此系数（>=1，给突发留余量）
    #[serde(default = "d_clg_headroom")]
    pub headroom_factor: f32,
    /// headroom 在 up_threshold 附近的过渡带宽度：从 up_threshold - headroom_ramp 到 up_threshold 线性由 1.0 渐变至 headroom_factor，
    /// 避免阶跃导致振荡
    #[serde(default = "d_clg_headroom_ramp")]
    pub headroom_ramp: f32,
    /// 性能下限：目标性能永不低于此值（锁频下限的百分比）
    #[serde(default = "d_clg_floor")]
    pub perf_floor: f32,
    /// 性能上限：目标性能永不超过此值（锁频上限的百分比）
    #[serde(default = "d_clg_ceil")]
    pub perf_ceil: f32,
    /// 按核心组的参数覆盖（键：little / big / prime，大小写敏感）：三簇负载形态差异大，
    /// 一刀切只能取「最松的那簇」能接受的上限未列出的组与字段回退模式级同名参数，
    /// **不配置就与加这个字段之前逐位一致**
    #[serde(default)]
    pub per_cluster: HashMap<String, ClgClusterOverride>,
    /// 接管瞬间的初始性能：init_policies 时先把频率锁到该档位，避免从 0 爬升
    #[serde(default = "d_clg_init")]
    pub perf_init: f32,
    /// 升频快速通道判定：target_perf 超过 current_perf 的幅度大于此值时直接快速升频
    #[serde(default = "d_clg_up_jump")]
    pub up_jump_threshold: f32,
    /// 低负载升频（负载未达 up_threshold 时）对 smoothing_up 的缩放系数
    #[serde(default = "d_clg_slow_up_scale")]
    pub slow_up_scale: f32,
    /// 极低负载阈值：util 低于此值时跳过降频防抖、立即直接降频
    #[serde(default = "d_clg_down_fast_thresh")]
    pub down_fast_threshold: f32,
    /// 尖峰抑制：单 tick util 跳升超过此值时，其增量按 spike_decay 比例衰减，避免孤立瞬时尖峰（如单核 0↔100%）瞬间拉满 perf
    #[serde(default = "d_clg_spike_jump")]
    pub spike_jump_threshold: f32,
    /// 尖峰增量保留比例（0.0=完全抑制，1.0=不抑制）
    #[serde(default = "d_clg_spike_decay")]
    pub spike_decay: f32,
    /// 触摸升频总开关：true 时触摸屏幕将把大核频率提前抬高一档（大核区间随命中 SoC 变化，见 common::chiri_core_ranges），减少操作卡顿
    #[serde(default = "crate::utils::default_true")]
    pub touch_boost_enabled: bool,
    /// 触摸升频保持时长（ms）：触摸后窗口期内大核锁定在抬高档位，窗口结束回落到负载调度
    #[serde(default = "d_clg_touch_boost_ms")]
    pub touch_boost_ms: u64,
    /// 触摸升频抬高的频率档数：在可用频率表中向上移动的档数（1 档即一个频率步进）
    #[serde(default = "d_clg_touch_boost_tiers")]
    pub touch_boost_tiers: u32,
    // [dwell] 写频决策层滞回（Phase 2）：最小驻留 + 死区
    /// 写频最小驻留（ms）+ 方向翻摆滞回：距上次实际写频不足该时长且方向相反（翻摆）时延迟写、下次 flush 补写缺省 320ms ≈ 2 个负载 tick（ChiRi 160ms 采样；特调侧 80ms）
    /// 豁免路径（触摸提频 / 极低负载立即降频 / 写失败补写）不经滞回；热保护 clamp 不豁免
    #[serde(default = "d_clg_write_dwell_ms")]
    pub write_dwell_ms: u64,
    /// 写频死区（比例，占硬件最高频）：|target−current| 小于该阈值不写，防相邻
    /// OPP 档来回改写与 tuned 的 hysteresis 同口径；0 = 关闭
    /// **P1-2（2026-09-27）**：本值同时是「方向稳定」的死区——上/下两个方向都按它判
    /// 「写不进 sysfs 的 no-op tick」，no-op 不再清零对面方向的反转确认计数（见 [dir_stabilize]）；
    /// 需要更保守就调高本值（每档死区 = write_deadzone × 硬件最高频）
    #[serde(default = "d_clg_write_deadzone")]
    pub write_deadzone: f32,
    // [steady_decay] T11 稳态下探：对「写侧性能偏移」做**限幅积分控制**（被控量是 util，见 cpu_load_governor.rs）。
    // 目标水位 T = up_threshold × target_ratio：util 高于 T 就回收下探、低于就加深、死区内保持 → 回路自带稳态，无锯齿
    /// 稳态下探总开关（缺省 true）
    #[serde(default = "crate::utils::default_true")]
    pub steady_decay_enabled: bool,
    /// 目标水位系数：T = up_threshold × 本值（缺省 0.90）
    #[serde(default = "d_clg_steady_target_ratio")]
    pub steady_decay_target_ratio: f32,
    /// 稳定带 + T 的保持死区：|Δsmoothed| ≤ 本值视为稳定；|smoothed − T| ≤ 本值保持不变
    #[serde(default = "d_clg_steady_band")]
    pub steady_decay_band: f32,
    /// 连续稳定 tick 数阈值：满 N tick 才启动积分（8550 约 160ms/tick → 12 tick ≈ 2s）
    #[serde(default = "d_clg_steady_ticks")]
    pub steady_decay_stable_ticks: u32,
    /// 每次加深的性能比步长（回收步长 = 本值 × 2，防过冲）
    #[serde(default = "d_clg_steady_step")]
    pub steady_decay_step: f32,
    /// 下探量幅值上限（性能比，抗积分饱和）
    #[serde(default = "d_clg_steady_max")]
    pub steady_decay_max: f32,
}

// CLG 各参数缺省值：feature.yaml 省略字段时回退到此处（与 normalize 的兜底默认一致）
fn d_clg_up_thresh() -> f32 {
    0.80
}
fn d_clg_down_thresh() -> f32 {
    0.50
}
fn d_clg_smooth_up() -> f32 {
    0.60
}
fn d_clg_util_smooth() -> f32 {
    1.0
}
fn d_clg_down_rate() -> u32 {
    3
}
fn d_clg_up_rate() -> u32 {
    2
}
fn d_clg_headroom() -> f32 {
    1.25
}
fn d_clg_headroom_ramp() -> f32 {
    0.15
}
fn d_clg_floor() -> f32 {
    0.15
}
fn d_clg_ceil() -> f32 {
    1.0
}
fn d_clg_init() -> f32 {
    0.50
}
fn d_clg_up_jump() -> f32 {
    0.35
}
fn d_clg_slow_up_scale() -> f32 {
    0.02
}
fn d_clg_down_fast_thresh() -> f32 {
    0.10
}
fn d_clg_spike_jump() -> f32 {
    0.35
}
fn d_clg_spike_decay() -> f32 {
    0.30
}
fn d_clg_touch_boost_ms() -> u64 {
    400
}
fn d_clg_touch_boost_tiers() -> u32 {
    1
}
/// [dwell] 写频最小驻留缺省：2 tick × ChiRi 160ms 负载采样
fn d_clg_write_dwell_ms() -> u64 {
    320
}
/// [dwell] 写频死区缺省：硬件最高频的 3%（与 tuned hysteresis 同口径）
fn d_clg_write_deadzone() -> f32 {
    0.03
}
// [steady_decay] T11 稳态下探缺省值（CLG）
fn d_clg_steady_target_ratio() -> f32 {
    0.90
}
fn d_clg_steady_band() -> f32 {
    0.05
}
fn d_clg_steady_ticks() -> u32 {
    12
}
fn d_clg_steady_step() -> f32 {
    0.01
}
fn d_clg_steady_max() -> f32 {
    0.20
}

/// 按核心组的参数覆盖（CLG `per_cluster` 段的元素）：只覆盖最常用的五个量（阈值 / 余量 / 上下限），其余沿用模式级
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ClgClusterOverride {
    pub up_threshold: Option<f32>,
    pub down_threshold: Option<f32>,
    pub headroom_factor: Option<f32>,
    pub perf_floor: Option<f32>,
    pub perf_ceil: Option<f32>,
}

impl ClgClusterOverride {
    /// 非有限值丢弃（回退模式级）：范围钳制统一在 `effective()` 里做
    pub fn normalize(&mut self) {
        if let Some(v) = self.up_threshold {
            if !v.is_finite() {
                self.up_threshold = None;
            }
        }
        if let Some(v) = self.down_threshold {
            if !v.is_finite() {
                self.down_threshold = None;
            }
        }
        if let Some(v) = self.headroom_factor {
            if !v.is_finite() {
                self.headroom_factor = None;
            }
        }
        if let Some(v) = self.perf_floor {
            if !v.is_finite() {
                self.perf_floor = None;
            }
        }
        if let Some(v) = self.perf_ceil {
            if !v.is_finite() {
                self.perf_ceil = None;
            }
        }
    }
}

impl Default for CpuLoadGovernorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            up_threshold: d_clg_up_thresh(),
            down_threshold: d_clg_down_thresh(),
            smoothing_up: d_clg_smooth_up(),
            util_smoothing: d_clg_util_smooth(),
            down_rate_limit_ticks: d_clg_down_rate(),
            up_rate_limit_ticks: d_clg_up_rate(),
            headroom_factor: d_clg_headroom(),
            headroom_ramp: d_clg_headroom_ramp(),
            perf_floor: d_clg_floor(),
            perf_ceil: d_clg_ceil(),
            per_cluster: HashMap::new(),
            perf_init: d_clg_init(),
            up_jump_threshold: d_clg_up_jump(),
            slow_up_scale: d_clg_slow_up_scale(),
            down_fast_threshold: d_clg_down_fast_thresh(),
            spike_jump_threshold: d_clg_spike_jump(),
            spike_decay: d_clg_spike_decay(),
            touch_boost_enabled: true,
            touch_boost_ms: d_clg_touch_boost_ms(),
            touch_boost_tiers: d_clg_touch_boost_tiers(),
            write_dwell_ms: d_clg_write_dwell_ms(),
            write_deadzone: d_clg_write_deadzone(),
            steady_decay_enabled: true,
            steady_decay_target_ratio: d_clg_steady_target_ratio(),
            steady_decay_band: d_clg_steady_band(),
            steady_decay_stable_ticks: d_clg_steady_ticks(),
            steady_decay_step: d_clg_steady_step(),
            steady_decay_max: d_clg_steady_max(),
        }
    }
}

// [clg_normalize]
impl CpuLoadGovernorConfig {
    /// 校验并规范化配置：
    /// - 非有限值（NaN/±Inf，如 YAML 溢出值）回退默认，防止污染控制链
    /// - 阈值/系数限制在合理区间
    /// - floor/ceil/init 交叉约束，保证 f32::clamp 永不 panic
    pub fn normalize(&mut self) {
        if !self.up_threshold.is_finite() {
            self.up_threshold = d_clg_up_thresh();
        }
        if !self.down_threshold.is_finite() {
            self.down_threshold = d_clg_down_thresh();
        }
        if !self.smoothing_up.is_finite() {
            self.smoothing_up = d_clg_smooth_up();
        }
        if !self.util_smoothing.is_finite() {
            self.util_smoothing = d_clg_util_smooth();
        }
        self.util_smoothing = self.util_smoothing.clamp(0.0, 1.0);
        if !self.headroom_factor.is_finite() {
            self.headroom_factor = d_clg_headroom();
        }
        if !self.headroom_ramp.is_finite() {
            self.headroom_ramp = d_clg_headroom_ramp();
        }
        if !self.perf_floor.is_finite() {
            self.perf_floor = d_clg_floor();
        }
        if !self.perf_ceil.is_finite() {
            self.perf_ceil = d_clg_ceil();
        }
        if !self.perf_init.is_finite() {
            self.perf_init = d_clg_init();
        }
        if !self.up_jump_threshold.is_finite() {
            self.up_jump_threshold = d_clg_up_jump();
        }
        if !self.slow_up_scale.is_finite() {
            self.slow_up_scale = d_clg_slow_up_scale();
        }
        if !self.down_fast_threshold.is_finite() {
            self.down_fast_threshold = d_clg_down_fast_thresh();
        }
        if !self.spike_jump_threshold.is_finite() {
            self.spike_jump_threshold = d_clg_spike_jump();
        }
        if !self.spike_decay.is_finite() {
            self.spike_decay = d_clg_spike_decay();
        }

        // 区间限制（语义约束）
        self.up_threshold = self.up_threshold.clamp(0.0, 1.0);
        self.down_threshold = self.down_threshold.clamp(0.0, 1.0);
        // 滞回语义：降频阈值不得高于升频阈值
        if self.down_threshold > self.up_threshold {
            self.down_threshold = self.up_threshold;
        }
        self.smoothing_up = self.smoothing_up.clamp(0.0, 1.0);
        self.slow_up_scale = self.slow_up_scale.clamp(0.0, 1.0);
        self.up_jump_threshold = self.up_jump_threshold.clamp(0.0, 1.0);
        self.down_fast_threshold = self.down_fast_threshold.clamp(0.0, 1.0);
        self.spike_jump_threshold = self.spike_jump_threshold.clamp(0.0, 1.0);
        self.spike_decay = self.spike_decay.clamp(0.0, 1.0);
        self.headroom_ramp = self.headroom_ramp.clamp(0.0, 1.0);
        // headroom 语义 >= 1（余量放大）
        self.headroom_factor = self.headroom_factor.clamp(1.0, 3.0);

        // 触摸升频参数限制：保持时长限制在 1s 内防误配，档数限制在 8 档内
        self.touch_boost_ms = self.touch_boost_ms.clamp(1, 1000);
        self.touch_boost_tiers = self.touch_boost_tiers.min(8);
        // 触摸升频关闭时（touch_boost_enabled=false）时长置 0，避免窗口逻辑误判
        if !self.touch_boost_enabled {
            self.touch_boost_ms = 0;
        }
        // [dwell] 写频滞回参数钳制：驻留 0 = 关闭翻摆延迟；死区 0 = 关闭（上限 0.2）
        self.write_dwell_ms = self.write_dwell_ms.min(5_000);
        if !self.write_deadzone.is_finite() {
            self.write_deadzone = d_clg_write_deadzone();
        }
        self.write_deadzone = self.write_deadzone.clamp(0.0, 0.2);

        // [steady_decay] 稳态下探参数钳制（限幅积分控制）：非有限值回退默认，范围限制
        if !self.steady_decay_target_ratio.is_finite() {
            self.steady_decay_target_ratio = d_clg_steady_target_ratio();
        }
        self.steady_decay_target_ratio = self.steady_decay_target_ratio.clamp(0.5, 1.0);
        if !self.steady_decay_band.is_finite() {
            self.steady_decay_band = d_clg_steady_band();
        }
        self.steady_decay_band = self.steady_decay_band.clamp(0.0, 0.3);
        self.steady_decay_stable_ticks = self.steady_decay_stable_ticks.clamp(1, 600);
        if !self.steady_decay_step.is_finite() {
            self.steady_decay_step = d_clg_steady_step();
        }
        self.steady_decay_step = self.steady_decay_step.clamp(0.0, 0.1);
        if !self.steady_decay_max.is_finite() {
            self.steady_decay_max = d_clg_steady_max();
        }
        self.steady_decay_max = self.steady_decay_max.clamp(0.0, 0.5);

        // 交叉约束（顺序保证 clamp 边界合法）
        if self.perf_floor > self.perf_ceil {
            self.perf_floor = self.perf_ceil;
        }
        self.perf_floor = self.perf_floor.clamp(0.0, 1.0);
        self.perf_ceil = self.perf_ceil.clamp(0.0, 1.0);
        if self.perf_floor > self.perf_ceil {
            self.perf_floor = self.perf_ceil;
        }
        self.perf_init = self.perf_init.clamp(self.perf_floor, self.perf_ceil);
        for ov in self.per_cluster.values_mut() {
            ov.normalize();
        }
    }

    /// 取某个核心组的**有效参数**：以模式级参数为底，套 `per_cluster` 覆盖后再走一遍交叉约束（floor/ceil 顺序、perf_init 落在区间内）
    /// 未配置该组时返回自身克隆（与加 per_cluster 之前逐位一致）；Worker 侧按单一 `self.cfg` 使用，无需感知覆盖
    pub fn effective(&self, cluster: &str) -> CpuLoadGovernorConfig {
        let mut c = self.clone();
        let Some(ov) = self.per_cluster.get(cluster) else {
            return c;
        };
        if let Some(v) = ov.up_threshold {
            c.up_threshold = v.clamp(0.0, 1.0);
        }
        if let Some(v) = ov.down_threshold {
            c.down_threshold = v.clamp(0.0, 1.0);
        }
        if let Some(v) = ov.headroom_factor {
            c.headroom_factor = v.clamp(1.0, 2.0);
        }
        if let Some(v) = ov.perf_floor {
            c.perf_floor = v.clamp(0.0, 1.0);
        }
        if let Some(v) = ov.perf_ceil {
            c.perf_ceil = v.clamp(0.0, 1.0);
        }
        if c.down_threshold > c.up_threshold {
            c.down_threshold = c.up_threshold;
        }
        if c.perf_floor > c.perf_ceil {
            c.perf_floor = c.perf_ceil;
        }
        c.perf_init = c.perf_init.clamp(c.perf_floor, c.perf_ceil);
        c
    }
}

// [mode_io]
// 核心模式与杂项配置
/// 单一性能模式的配置集合（feature.yaml 中 reduce / default / boost / vector 之一）
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Mode {
    /// 该模式下的 CLG 调频参数
    #[serde(default, alias = "CpuLoadGovernor")]
    pub cpu_load_governor: CpuLoadGovernorConfig,
}

/// IO 优化段（对应 feature.yaml `IO_Settings`），值均为写入 /sys/block/*/queue 的字符串
#[derive(Debug, Deserialize, Clone)]
pub struct IOSettings {
    /// I/O 调度器名（如 none / mq-deadline / cfq），空字符串则跳过不写
    #[serde(default, rename = "Scheduler")]
    pub scheduler: String,
    /// 预读大小（kB），写入 queue/read_ahead_kb
    #[serde(default = "default_read_ahead_kb")]
    pub read_ahead_kb: String,
    /// 请求合并策略：0=关闭，1=仅简单合并，2=完全合并
    #[serde(default = "default_nomerges")]
    pub nomerges: String,
    /// IO 统计开关：0=关闭，1=开启（写入 queue/iostats）
    #[serde(default = "default_iostats")]
    pub iostats: String,
}

impl Default for IOSettings {
    fn default() -> Self {
        Self {
            scheduler: String::new(),
            read_ahead_kb: default_read_ahead_kb(),
            nomerges: default_nomerges(),
            iostats: default_iostats(),
        }
    }
}

// IO_Settings 缺省值
fn default_read_ahead_kb() -> String {
    "128".to_string()
}
fn default_nomerges() -> String {
    "2".to_string()
}
fn default_iostats() -> String {
    "0".to_string()
}

/// cpuidle 段（对应 feature.yaml `CpuIdle`）
#[derive(Debug, Deserialize, Default)]
// [toggles]
#[serde(rename_all = "snake_case")]
pub struct CpuIdle {
    /// 要切换到的 cpuidle 调度器名（写入 /sys/devices/system/cpu/cpuidle/current_governor）
    pub current_governor: String,
}

/// 功能总开关段（对应 feature.yaml `Function`）
#[derive(Debug, Deserialize, Default)]
pub struct FunctionToggles {
    /// 是否应用 cpuidle governor 切换
    #[serde(rename = "CpuIdleScalingGovernor")]
    pub cpu_idle_scaling_governor: bool,
    /// 是否应用 IO 优化（调度器/预读/合并/统计）
    #[serde(rename = "IOOptimization")]
    pub io_optimization: bool,
}

/// 明日方舟特调（akmode）配置，与 CLG 完全解耦
/// **无档位**：不做模式分档，按各核心组实时负载直接移动 scaling_max_freq 上限
/// （FAS 式连续控制）机制：激活时统一内核调速器为默认调速器（选型见 common.rs [governor]）、min 压到硬件最低；
/// 之后每个负载 tick 目标上限比例 = clamp(组内最大核心占用率 × headroom, perf_floor, 1.0)：
/// 升：目标 > 当前上限 + hysteresis → 立即上调（schedutil 在新上限内自由取频）；
/// 降：目标 < 当前上限 − hysteresis 且持续 down_hold_ms → 上限收到目标档位
// [ak_config]
#[derive(Debug, Deserialize, Clone)]
pub struct SpecialTunedConfig {
    /// 负载系数：目标上限 = 组内最大核心占用率 × headroom
    /// >1.0 = 放大留余量（响应优先，游戏默认 1.15）；1.0 = 原样；
    /// <1.0 = **收紧**（省电方向，视频稳态用 0.95，配合 perf_ceil 压天花板）
    #[serde(default = "d_ak_headroom")]
    pub headroom: f32,
    /// 目标上限比例下限（0 = 空闲组上限可收到硬件最低频）
    #[serde(default = "d_ak_perf_floor")]
    pub perf_floor: f32,
    /// 变更死区（比例）：目标与当前上限差值不超过该值时不写，防写抖动
    #[serde(default = "d_ak_hysteresis")]
    pub hysteresis: f32,
    /// 降频保持（ms）：目标持续低于当前上限该时长才真正下调，防负载抖动来回改写
    #[serde(default = "d_ak_down_hold_ms")]
    pub down_hold_ms: u64,
    /// 是否走 boost 类亲和（cpuset 收窄到 big+prime + core_ctl 保大核常在线）。游戏特调 true（保响应、防热插拔打架）；视频/轻载省电特调 false——保大核常在线与「贴负载降频」
    /// 的省电目标相反（空转漏电、解码线程被低上限压住）
    #[serde(default = "crate::utils::default_true")]
    pub boost_affinity: bool,
    /// 决策负载的 EMA 平滑系数（0.05~1.0）：1.0 = 不平滑（游戏默认，瞬时升频是响应性的一部分）；< 1 时 util = α×瞬时 + (1-α)×上次，滤掉瞬时尖峰、防止上限被「升频立即执行」反复推高
    /// 视频/轻载用 0.3~0.5
    #[serde(default = "d_ak_util_smoothing")]
    pub util_smoothing: f32,
    /// 性能比例**天花板**（0..1）：目标比例的上限钳制（CLG 有同名参数；缺它无法单独压某组频率上限）1.0 = 不设天花板（与加该字段前完全一致）
    #[serde(default = "d_ak_perf_ceil")]
    pub perf_ceil: f32,
    /// 按核心组的参数覆盖（键：little / big / prime，大小写敏感）：三簇负载形态往往
    /// 完全不同，模式级一刀切顾此失彼未列出的组/字段一律回退模式级同名参数——
    /// **不给配置就与旧行为逐位一致**
    #[serde(default)]
    pub per_cluster: HashMap<String, ClusterTunedOverride>,
    /// 接管期间写入 `/proc/sys/kernel/sched_migration_cost_ns`（None = 完全不动）：偏小会让调度器频繁跨核搬迁，调高可减无谓迁移与 cache 失效；
    /// release 按快照恢复
    #[serde(default)]
    pub migration_cost_ns: Option<u64>,
    // [dwell] 写频决策层滞回（Phase 2，与 CLG 同口径）
    /// 写频最小驻留（ms）+ 方向翻摆滞回，与 CLG 的 write_dwell_ms 同口径：缺省 80ms ≈ 2 个特调 tick（40ms）。死区沿用上方 hysteresis 字段；
    /// 接管初写/恢复与写失败补写不经滞回
    #[serde(default = "d_ak_write_dwell_ms")]
    pub write_dwell_ms: u64,
    // [up_confirm] T11 严格升频审批：目标比当前更高时须连续 N tick 满足才抬升（0/1 = 关闭确认）；下降路径不变
    /// 升频确认 tick 数（缺省 2）
    #[serde(default = "d_ak_up_confirm")]
    pub up_confirm_ticks: u32,
    // [steady_decay] T11 稳态下探（与 CLG 同款**限幅积分控制**，作用在「将要写入的比例」上，不回写已应用状态）
    /// 稳态下探总开关（缺省 true）
    #[serde(default = "crate::utils::default_true")]
    pub steady_decay_enabled: bool,
    /// 目标水位系数：T = up_ref × 本值，up_ref = 目标比例顶到 ceil 时的 util（缺省 0.90）
    #[serde(default = "d_ak_steady_target_ratio")]
    pub steady_decay_target_ratio: f32,
    /// 稳定带 + T 的保持死区：|Δutil| ≤ 本值视为稳定；|util − T| ≤ 本值保持不变
    #[serde(default = "d_ak_steady_band")]
    pub steady_decay_band: f32,
    /// 连续稳定 tick 数阈值：满 N tick 才启动积分
    #[serde(default = "d_ak_steady_ticks")]
    pub steady_decay_stable_ticks: u32,
    /// 每次加深的比例步长（回收步长 = 本值 × 2，防过冲）
    #[serde(default = "d_ak_steady_step")]
    pub steady_decay_step: f32,
    /// 下探量幅值上限（比例，抗积分饱和）
    #[serde(default = "d_ak_steady_max")]
    pub steady_decay_max: f32,
}

/// 按核心组的参数覆盖（`per_cluster` 段的元素）：**全部字段可选**，
/// None = 回退模式级同名参数（这样新增覆盖不必把每个字段都抄一遍）
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ClusterTunedOverride {
    pub headroom: Option<f32>,
    pub perf_floor: Option<f32>,
    pub perf_ceil: Option<f32>,
    pub hysteresis: Option<f32>,
    pub down_hold_ms: Option<u64>,
    pub util_smoothing: Option<f32>,
}

impl ClusterTunedOverride {
    /// 非有限值直接丢弃（回退模式级）：范围钳制统一在 `for_cluster` 里做，避免两处各钳一次导致口径不一致
    pub fn normalize(&mut self) {
        if let Some(v) = self.headroom {
            if !v.is_finite() {
                self.headroom = None;
            }
        }
        if let Some(v) = self.perf_floor {
            if !v.is_finite() {
                self.perf_floor = None;
            }
        }
        if let Some(v) = self.perf_ceil {
            if !v.is_finite() {
                self.perf_ceil = None;
            }
        }
        if let Some(v) = self.hysteresis {
            if !v.is_finite() {
                self.hysteresis = None;
            }
        }
        if let Some(v) = self.util_smoothing {
            if !v.is_finite() {
                self.util_smoothing = None;
            }
        }
    }
}

/// 单个核心组的**有效**参数：`per_cluster` 覆盖 + 范围钳制之后的最终值。 tuned.rs 的决策只用这个，不再直接读 `SpecialTunedConfig` 的字段
#[derive(Debug, Clone, Copy)]
pub struct EffectiveTuned {
    pub headroom: f32,
    pub perf_floor: f32,
    pub perf_ceil: f32,
    pub hysteresis: f32,
    pub down_hold_ms: u64,
    pub util_smoothing: f32,
}

// SpecialTunedConfig 缺省值：tuned_profiles.yaml 没写的字段回退到这里
fn d_ak_headroom() -> f32 {
    1.15
}
fn d_ak_perf_floor() -> f32 {
    0.0
}
fn d_ak_hysteresis() -> f32 {
    0.03
}
fn d_ak_down_hold_ms() -> u64 {
    100
}
fn d_ak_util_smoothing() -> f32 {
    1.0
}
/// 默认不设天花板（1.0 = 与加该字段前一致）
fn d_ak_perf_ceil() -> f32 {
    1.0
}
/// [dwell] 写频最小驻留缺省：2 tick × 特调 40ms 采样
fn d_ak_write_dwell_ms() -> u64 {
    80
}
// [up_confirm]/[steady_decay] 特调侧缺省值（与 CLG 同口径）
fn d_ak_up_confirm() -> u32 {
    2
}
fn d_ak_steady_target_ratio() -> f32 {
    0.90
}
fn d_ak_steady_band() -> f32 {
    0.05
}
fn d_ak_steady_ticks() -> u32 {
    12
}
fn d_ak_steady_step() -> f32 {
    0.01
}
fn d_ak_steady_max() -> f32 {
    0.20
}

impl Default for SpecialTunedConfig {
    fn default() -> Self {
        Self {
            headroom: d_ak_headroom(),
            perf_floor: d_ak_perf_floor(),
            hysteresis: d_ak_hysteresis(),
            down_hold_ms: d_ak_down_hold_ms(),
            boost_affinity: true,
            util_smoothing: d_ak_util_smoothing(),
            perf_ceil: d_ak_perf_ceil(),
            per_cluster: HashMap::new(),
            migration_cost_ns: None,
            write_dwell_ms: d_ak_write_dwell_ms(),
            up_confirm_ticks: d_ak_up_confirm(),
            steady_decay_enabled: true,
            steady_decay_target_ratio: d_ak_steady_target_ratio(),
            steady_decay_band: d_ak_steady_band(),
            steady_decay_stable_ticks: d_ak_steady_ticks(),
            steady_decay_step: d_ak_steady_step(),
            steady_decay_max: d_ak_steady_max(),
        }
    }
}

impl SpecialTunedConfig {
    /// 校验配置：参数钳制在合理范围，非有限值回退默认
    pub fn normalize(&mut self) {
        if !self.headroom.is_finite() {
            self.headroom = d_ak_headroom();
        }
        // 下限放到 0.5：<1.0 是**收紧**（省电方向），1.0 = 原样，>1.0 = 放大留余量
        self.headroom = self.headroom.clamp(0.5, 2.0);
        if !self.perf_floor.is_finite() {
            self.perf_floor = d_ak_perf_floor();
        }
        self.perf_floor = self.perf_floor.clamp(0.0, 0.5);
        if !self.hysteresis.is_finite() {
            self.hysteresis = d_ak_hysteresis();
        }
        self.hysteresis = self.hysteresis.clamp(0.0, 0.2);
        self.down_hold_ms = self.down_hold_ms.min(5_000);
        if !self.util_smoothing.is_finite() {
            self.util_smoothing = d_ak_util_smoothing();
        }
        self.util_smoothing = self.util_smoothing.clamp(0.05, 1.0);
        if !self.perf_ceil.is_finite() {
            self.perf_ceil = d_ak_perf_ceil();
        }
        self.perf_ceil = self.perf_ceil.clamp(0.1, 1.0);
        // floor 高于 ceil 时 clamp 会静默取 ceil（语义退化成恒等于上限），直接对齐
        if self.perf_floor > self.perf_ceil {
            self.perf_floor = self.perf_ceil;
        }
        // [dwell] 写频驻留钳制（ms）：0 = 关闭翻摆延迟
        self.write_dwell_ms = self.write_dwell_ms.min(5_000);
        // [up_confirm] 升频确认 tick 数上限；[steady_decay] 稳态下探参数钳制（与 CLG 同口径）
        self.up_confirm_ticks = self.up_confirm_ticks.min(20);
        if !self.steady_decay_target_ratio.is_finite() {
            self.steady_decay_target_ratio = d_ak_steady_target_ratio();
        }
        self.steady_decay_target_ratio = self.steady_decay_target_ratio.clamp(0.5, 1.0);
        if !self.steady_decay_band.is_finite() {
            self.steady_decay_band = d_ak_steady_band();
        }
        self.steady_decay_band = self.steady_decay_band.clamp(0.0, 0.3);
        self.steady_decay_stable_ticks = self.steady_decay_stable_ticks.clamp(1, 600);
        if !self.steady_decay_step.is_finite() {
            self.steady_decay_step = d_ak_steady_step();
        }
        self.steady_decay_step = self.steady_decay_step.clamp(0.0, 0.1);
        if !self.steady_decay_max.is_finite() {
            self.steady_decay_max = d_ak_steady_max();
        }
        self.steady_decay_max = self.steady_decay_max.clamp(0.0, 0.5);
        for ov in self.per_cluster.values_mut() {
            ov.normalize();
        }
    }

    /// 取某个核心组的有效参数：先套 `per_cluster` 覆盖，再按全局口径钳制。未配置该组时返回模式级参数（与加 per_cluster 之前逐位一致）
    pub fn for_cluster(&self, name: &str) -> EffectiveTuned {
        let mut e = EffectiveTuned {
            headroom: self.headroom,
            perf_floor: self.perf_floor,
            perf_ceil: self.perf_ceil,
            hysteresis: self.hysteresis,
            down_hold_ms: self.down_hold_ms,
            util_smoothing: self.util_smoothing,
        };
        let Some(ov) = self.per_cluster.get(name) else {
            return e;
        };
        if let Some(v) = ov.headroom.filter(|v| v.is_finite()) {
            e.headroom = v.clamp(0.5, 2.0);
        }
        if let Some(v) = ov.perf_floor.filter(|v| v.is_finite()) {
            e.perf_floor = v.clamp(0.0, 0.5);
        }
        if let Some(v) = ov.perf_ceil.filter(|v| v.is_finite()) {
            e.perf_ceil = v.clamp(0.1, 1.0);
        }
        if let Some(v) = ov.hysteresis.filter(|v| v.is_finite()) {
            e.hysteresis = v.clamp(0.0, 0.2);
        }
        if let Some(v) = ov.down_hold_ms {
            e.down_hold_ms = v.min(5_000);
        }
        if let Some(v) = ov.util_smoothing.filter(|v| v.is_finite()) {
            e.util_smoothing = v.clamp(0.05, 1.0);
        }
        if e.perf_floor > e.perf_ceil {
            e.perf_floor = e.perf_ceil;
        }
        e
    }
}

// [thermal_config]

/// 热保护配置（feature.yaml `Thermal` 段）。双温度源取较小值：电池温度是主参考（壳体发热由电池主导，温升慢但持续）；CPU 温度仅在极端情况参与——内核 95°C 温控已兜底，
/// 软件层阈值设得很高（90/95°C）只防极端场景豁免档 free_above 仅在 `clamp_heavy=false`（重钳关闭）时生效：当前性能上限已高于豁免档时不钳制，持续高负载可冲到硬件最高频；
/// `clamp_heavy=true`（默认）时 cap 窗口内对所有簇恒钳写频目标（不回写 current_perf，窗口解除即恢复全速）
/// 仅对 CLG 接管模式生效（reduce/default/boost/doze/scenemode）；vector/akmode 不受影响
#[derive(Debug, Deserialize, Clone)]
pub struct ThermalGuardConfig {
    /// false = 完全不采温、不压制
    #[serde(default = "crate::utils::default_true")]
    pub enabled: bool,
    /// 电池软限（°C）：默认 41°C，手机壳体开始发烫的临界点
    #[serde(default = "d_batt_soft_temp")]
    pub batt_soft_temp_c: f32,
    /// 电池硬限（°C）：默认 45°C，手握明显发烫
    #[serde(default = "d_batt_hard_temp")]
    pub batt_hard_temp_c: f32,
    /// 电池中限（°C）：默认 43°C，取软/硬限中点 (41+45)/22026-09-27 实测：原三级阶梯在 41–45°C 这 4°C 区间里几乎无压制力（cap=85 窗口真钳制率仅 13.8%、
    /// 该区间占会话时长 64%），中档给中段一个台阶**必须** 软限 < 中限 < 硬限（normalize 越界钳制 + warn）
    #[serde(default = "d_batt_mid_temp")]
    pub batt_mid_temp_c: f32,
    /// CPU 软限（°C）：默认 90°C，一般游戏不会到这里（内核 95°C 才兜底）
    #[serde(default = "d_cpu_soft_temp")]
    pub cpu_soft_temp_c: f32,
    /// CPU 硬限（°C）：默认 95°C，极端情况才触发
    #[serde(default = "d_cpu_hard_temp")]
    pub cpu_hard_temp_c: f32,
    /// CPU 中限（°C）：默认 92°C，软/硬限中点取整向下——soc_max 聚合温区在高载下 85°C+常态、噪声大，中档刻意贴硬限一侧，避免合成温区的正常波动被当成中档触发
    #[serde(default = "d_cpu_mid_temp")]
    pub cpu_mid_temp_c: f32,
    /// 软限触发后性能上限压到这个比例（0..1）
    #[serde(default = "d_thermal_soft_cap")]
    pub soft_perf_cap: f32,
    /// 中限触发后性能上限压到这个比例，必须 <= soft_perf_cap
    #[serde(default = "d_thermal_mid_cap")]
    pub mid_perf_cap: f32,
    /// 硬限触发后性能上限压到这个比例，必须 <= mid_perf_cap
    #[serde(default = "d_thermal_hard_cap")]
    pub hard_perf_cap: f32,
    /// 豁免档（0..1）：当前性能比已超过此值时不钳制。默认 0.80，意味着 sustained load 能冲到 80%+ 硬件频率，只在中低负载积热时压住
    #[serde(default = "d_thermal_free_above")]
    pub free_above: f32,
    /// 重钳开关（默认 true，老配置缺该键亦 = true）：cap 窗口内对**所有簇**恒钳写频目标、窗口解除立即恢复全速；
    /// false = 回退旧豁免行为（current_perf >= free_above 的簇不钳制，此时 free_above 生效）只决定钳制形态，不改 cap 计算与温度判定；Config::
    /// load 同步到 CLG 层原子量，热重载即时生效
    #[serde(default = "crate::utils::default_true")]
    pub clamp_heavy: bool,
    /// 回滞（°C）：温度降到 软限 - hysteresis 以下才解除压制。设太小会在阈值附近反复触发/解除，频率抖动
    #[serde(default = "d_thermal_hysteresis")]
    pub hysteresis_c: f32,
    /// 硬限专用回滞（°C）：温度降到 硬限 - hysteresis_hard_c 以下才从硬档退到中档，独立于上方
    /// `hysteresis_c`（缺省 2.0，见 `d_thermal_hysteresis_hard`）：硬档 0.40↔软档 0.85 的阶跃比软档解除
    /// 敏感得多，2026-09-27 实测原实现硬档**无回滞**——batt 一降到 44.9°C 立刻跳回 0.85，批次2 在 75 分钟
    /// 内 5 次进硬档、两次 60s 内重入（最近相隔 2.0 s）；分离出来才能按机型单独调（8550 取 2.0，即 45→43°C 才退）
    #[serde(default = "d_thermal_hysteresis_hard")]
    pub hysteresis_hard_c: f32,
    /// 中限专用回滞（°C）：温度降到 中限 - hysteresis_mid_c 以下才从中档退到软档，缺省 2.0（见
    /// `d_thermal_hysteresis_mid`）。与软档共用同一回滞时，中档解除点 = 中限 - hyst 可能**低于软档跳闸点**
    /// （软限），此时中档一咬住就整段跳过软档、必须一路冷到软限以下才恢复（2026-09-28 实测 8550：
    /// 43-3=40 < 41，cap=60 的 5152 s 里 2556 s 落在 batt 40-41℃ 桶，另有 2216 s 是「温度从 43 降到 40、
    /// cap 一动不动」）。normalize 强制 `中限 - 本值 >= 软限`（每档解除点不得低于下一档跳闸点），
    /// 故缺省只能取 2.0——旧缺省 3.0 的解除点 40°C 本身就违反该不变量，**不是「旧行为不变」**
    #[serde(default = "d_thermal_hysteresis_mid")]
    pub hysteresis_mid_c: f32,
    /// 热态 tuned 响应开关（默认 true）：把 cap 也发给 tuned（playback/akmode 等）执行器加这个开关前 `thermal_cap` 只下发 CLG、tuned 对温度零响应（结构缺口）
    /// ，见 tuned.rs [thermal_ceil]
    #[serde(default = "crate::utils::default_true")]
    pub tuned_resp_enabled: bool,
    /// 热态 tuned 性能下限（0..1，默认 0.55）：tuned 侧有效上限 = min(档位天花板, max(cap, 本值))不直接把硬档 0.40 套到播放态——掉帧风险实打实（播放态帧信号 2026-09-27 起有，
    /// 但只是离线直方图 `decision=playback_fps`、在线无判据），本值把最坏情况钉在 0.55；normalize 保证 >= hard_perf_cap（否则热态比硬档还松、无意义）
    #[serde(default = "d_tuned_thermal_floor")]
    pub tuned_thermal_floor: f32,
    /// CPU 温度 thermal zone type 匹配名单（对 /sys/class/thermal 的 zone type 做contains 匹配，见 utils::find_cpu_temp_path）
    /// 默认 = 内置混合名单（高通soc_max/cpuss + MTK mtktscpu/cpu-1-/cpu-0-0-usr），**所有 SoC 的 feature.yaml 都不必写、行为不变**；
    /// 个别 SoC 需增删探测节点时用本字段覆盖Config::load 同步到 utils 静态缓存语义：`soc_max` 是 DTB virtual-sensor 聚合温区（多传感器取 max，保守值、
    /// 非纯 CPU 温度）；`cpuss*` 是 cluster 级 tsens 单传感器，作 soc_max 缺失时的回退匹配按名单序分层扫描（外层名单项、内层 zones，首个命中项胜出），
    /// 保证 soc_max 优先于 cpuss
    #[serde(default = "crate::utils::default_cpu_temp_zone_types")]
    pub cpu_temp_zone_types: Vec<String>,
}

// Thermal 缺省值：feature.yaml 省略该段时回退到此处
fn d_batt_soft_temp() -> f32 {
    41.0
}
fn d_batt_hard_temp() -> f32 {
    45.0
}
fn d_batt_mid_temp() -> f32 {
    43.0
}
fn d_cpu_soft_temp() -> f32 {
    90.0
}
fn d_cpu_hard_temp() -> f32 {
    95.0
}
fn d_cpu_mid_temp() -> f32 {
    92.0
}
fn d_thermal_soft_cap() -> f32 {
    0.70
}
fn d_thermal_mid_cap() -> f32 {
    0.60
}
fn d_thermal_hard_cap() -> f32 {
    0.40
}
fn d_thermal_free_above() -> f32 {
    0.80
}
fn d_thermal_hysteresis() -> f32 {
    3.0
}
/// 中/硬档回滞缺省（°C）：取缺省温度阶梯允许的上限——`mid` 侧 `min(batt_mid-batt_soft, cpu_mid-cpu_soft)`
/// = `min(43-41, 92-90)` = 2.0，`hard` 侧 `min(45-43, 95-92)` = 2.0（缺省 batt 41/43/45、cpu 90/92/95）。
/// **不能沿用 `d_thermal_hysteresis()`(3.0)**：3.0 每次加载都越界、被 [hyst_invariant] 钳到 2.0 并打
/// warn，等于「未写该键的配置永远拿不到注释承诺的值」（2026-09-28 审查发现）。另外旧缺省 3.0 的解除点
/// 43-3=40 / 45-3=42 **本身就违反不变量**（低于下一档跳闸点），故缺省收到 2.0 是修 bug，不是「保持旧行为」
fn d_thermal_hysteresis_mid() -> f32 {
    2.0
}
fn d_thermal_hysteresis_hard() -> f32 {
    2.0
}
fn d_tuned_thermal_floor() -> f32 {
    0.55
}

/// 热配置越界「钳到最近合法值 + warn 一次」：yaml 是编译期嵌入的用户可改文件，误配必须能自愈（normalize 不得 panic），同时留一条 warn 让判读时定位是哪个字段key 传字段名、
/// value/fixed 传原值与钳后值
/// [clamp_invariant] `hi < lo` 时把 `hi` 夹到 `lo`（`f32::clamp` 在 `min > max` 时 panic，
/// 自愈路径绝不能因上游漏钳把误配升级成崩溃循环）；不变量 `hi >= lo` 仍由各调用点上游保证
fn thermal_clamp_warn(v: f32, lo: f32, hi: f32, key: &str) -> f32 {
    // [clamp_invariant] `f32::clamp` 在 lo > hi 时 **panic**（assert!(min <= max)）：本函数是 normalize 的自愈路径，
    // 上游任一字段漏钳都会把「配置误配」升级成 `Config::load` panic → 守护进程崩溃 → watchdog 重启 → 崩溃循环。
    // 这里把非法区间夹成不 panic 的合法区间（lo <= hi 时结果与直接 clamp 完全相同，行为零变化），
    // 真正的区间不变量仍由各调用点上游的硬限兜底保证。
    let hi = if hi < lo { lo } else { hi };
    let fixed = v.clamp(lo, hi);
    if fixed != v {
        log::warn!(
            "{}",
            t_with_args(
                "thermal-config-clamped",
                &fluent_args!(
                    "key" => key,
                    "value" => format!("{:.2}", v),
                    "fixed" => format!("{:.2}", fixed)
                )
            )
        );
    }
    fixed
}

impl Default for ThermalGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            batt_soft_temp_c: d_batt_soft_temp(),
            batt_hard_temp_c: d_batt_hard_temp(),
            batt_mid_temp_c: d_batt_mid_temp(),
            cpu_soft_temp_c: d_cpu_soft_temp(),
            cpu_hard_temp_c: d_cpu_hard_temp(),
            cpu_mid_temp_c: d_cpu_mid_temp(),
            soft_perf_cap: d_thermal_soft_cap(),
            mid_perf_cap: d_thermal_mid_cap(),
            hard_perf_cap: d_thermal_hard_cap(),
            free_above: d_thermal_free_above(),
            clamp_heavy: true,
            hysteresis_c: d_thermal_hysteresis(),
            hysteresis_hard_c: d_thermal_hysteresis_hard(),
            hysteresis_mid_c: d_thermal_hysteresis_mid(),
            tuned_resp_enabled: true,
            tuned_thermal_floor: d_tuned_thermal_floor(),
            cpu_temp_zone_types: crate::utils::default_cpu_temp_zone_types(),
        }
    }
}

impl ThermalGuardConfig {
    /// 校验并规范化：非有限值回退默认；保证各传感器 soft < mid < hard、
    /// hard_cap <= mid_cap <= soft_cap <= 1.0、豁免档 >= soft_perf_cap（否则压制完全失效）、
    /// tuned 下限 >= hard_cap；temperature/cap 的单调性越界一律「钳到最近合法值 + warn」，
    /// **不 panic**（yaml 编译期嵌入、热重载要能自愈）
    /// [clamp_invariant] 硬限兜底判据是 `hard < soft + 1.0`（不是 `hard <= soft`）：mid 的钳制区间是
    /// `(soft + 0.5, hard - 0.5)`，只有 `hard - soft >= 1.0` 才凑得出 `lo <= hi`——差在 (0, 1) 的
    /// 配置（如 batt 44.9/45.0）在旧判据下不被兜底，`f32::clamp` 直接 panic
    pub fn normalize(&mut self) {
        // 非有限值（NaN/±Inf，如 YAML 溢出值）回退默认
        if !self.batt_soft_temp_c.is_finite() {
            self.batt_soft_temp_c = d_batt_soft_temp();
        }
        if !self.batt_hard_temp_c.is_finite() {
            self.batt_hard_temp_c = d_batt_hard_temp();
        }
        if !self.batt_mid_temp_c.is_finite() {
            self.batt_mid_temp_c = d_batt_mid_temp();
        }
        if !self.cpu_soft_temp_c.is_finite() {
            self.cpu_soft_temp_c = d_cpu_soft_temp();
        }
        if !self.cpu_hard_temp_c.is_finite() {
            self.cpu_hard_temp_c = d_cpu_hard_temp();
        }
        if !self.cpu_mid_temp_c.is_finite() {
            self.cpu_mid_temp_c = d_cpu_mid_temp();
        }
        if !self.soft_perf_cap.is_finite() {
            self.soft_perf_cap = d_thermal_soft_cap();
        }
        if !self.mid_perf_cap.is_finite() {
            self.mid_perf_cap = d_thermal_mid_cap();
        }
        if !self.hard_perf_cap.is_finite() {
            self.hard_perf_cap = d_thermal_hard_cap();
        }
        if !self.free_above.is_finite() {
            self.free_above = d_thermal_free_above();
        }
        if !self.hysteresis_c.is_finite() {
            self.hysteresis_c = d_thermal_hysteresis();
        }
        if !self.hysteresis_hard_c.is_finite() {
            self.hysteresis_hard_c = d_thermal_hysteresis_hard();
        }
        if !self.hysteresis_mid_c.is_finite() {
            self.hysteresis_mid_c = d_thermal_hysteresis_mid();
        }
        if !self.tuned_thermal_floor.is_finite() {
            self.tuned_thermal_floor = d_tuned_thermal_floor();
        }
        // 电池阈值（主参考）：软限限制在合理区间，硬限必须高于软限
        // [clamp_invariant] 下方中限的区间是 (软限+0.5, 硬限-0.5)，要 `lo <= hi` 就必须 `硬限 >= 软限 + 1.0`：
        // 判据必须是 `< 软限 + 1.0` 而不是 `<= 软限`——后者在「硬限 - 软限 ∈ (0, 1)」（如 44.9 / 45.0）时不兜底，
        // 直接把 lo=45.4 > hi=44.5 喂给 thermal_clamp_warn → 原实现 panic、现实现静默负向钳制。
        self.batt_soft_temp_c = self.batt_soft_temp_c.clamp(25.0, 60.0);
        if self.batt_hard_temp_c < self.batt_soft_temp_c + 1.0 {
            self.batt_hard_temp_c = (self.batt_soft_temp_c + 4.0).min(65.0);
        }
        // 中限必须落在 (软限, 硬限) 开区间：越界钳到最近合法值（上方已保证硬限 >= 软限 + 1.0，故 lo <= hi 恒成立、clamp 不会 panic）
        self.batt_mid_temp_c = thermal_clamp_warn(
            self.batt_mid_temp_c,
            self.batt_soft_temp_c + 0.5,
            self.batt_hard_temp_c - 0.5,
            "batt_mid_temp_c",
        );
        // CPU 阈值（极端参考）：软限限制在合理区间，硬限必须高于软限
        // [clamp_invariant] 同电池侧：下方区间是 (软限+0.5, 硬限-0.5)，`硬限 - 软限 ∈ (0, 1)`（如 94.9 / 95.0）同样会
        // 造出 lo > hi（95.4 / 94.5），故判据取 `硬限 < 软限 + 1.0`
        self.cpu_soft_temp_c = self.cpu_soft_temp_c.clamp(50.0, 95.0);
        if self.cpu_hard_temp_c < self.cpu_soft_temp_c + 1.0 {
            self.cpu_hard_temp_c = (self.cpu_soft_temp_c + 10.0).min(100.0);
        }
        self.cpu_mid_temp_c = thermal_clamp_warn(
            self.cpu_mid_temp_c,
            self.cpu_soft_temp_c + 0.5,
            self.cpu_hard_temp_c - 0.5,
            "cpu_mid_temp_c",
        );
        // 比例限制在 0..1，并保证 hard <= mid <= soft（温度更高反而压得更松是误配）
        self.soft_perf_cap = thermal_clamp_warn(self.soft_perf_cap, 0.0, 1.0, "soft_perf_cap");
        self.mid_perf_cap = thermal_clamp_warn(self.mid_perf_cap, 0.0, 1.0, "mid_perf_cap");
        self.hard_perf_cap = thermal_clamp_warn(self.hard_perf_cap, 0.0, 1.0, "hard_perf_cap");
        if self.mid_perf_cap > self.soft_perf_cap {
            self.mid_perf_cap = thermal_clamp_warn(
                self.mid_perf_cap,
                0.0,
                self.soft_perf_cap,
                "mid_perf_cap>soft_perf_cap",
            );
        }
        if self.hard_perf_cap > self.mid_perf_cap {
            self.hard_perf_cap = thermal_clamp_warn(
                self.hard_perf_cap,
                0.0,
                self.mid_perf_cap,
                "hard_perf_cap>mid_perf_cap",
            );
        }
        // 豁免档必须严格高于软限值，否则压制带 (soft_perf_cap, free_above) 为空、软限档完全不生效：过低或相等都抬到软限 + 0.10
        self.free_above = self.free_above.clamp(0.0, 1.0);
        if self.free_above <= self.soft_perf_cap {
            self.free_above = (self.soft_perf_cap + 0.10).min(1.0);
        }
        self.hysteresis_c = self.hysteresis_c.clamp(0.0, 20.0);
        self.hysteresis_hard_c = self.hysteresis_hard_c.clamp(0.0, 20.0);
        self.hysteresis_mid_c = self.hysteresis_mid_c.clamp(0.0, 20.0);
        // [hyst_invariant] 每档解除点（本级阈值 - 本级回滞）必须 >= 下一档（更浅档）的跳闸点，否则
        // 该档会「跳过」浅一档：中档解除点 < 软限 → 中档咬住后不回软档 0.85、必须一路冷过软限才恢复
        // （2026-09-28 实测就是这条）；硬档同理。两条传感器阶梯共用同一组回滞，故取两者中更严的上限
        // 钳制 + warn（yaml 编译期嵌入、误配必须自愈，不得 panic）
        let mid_room = (self.batt_mid_temp_c - self.batt_soft_temp_c)
            .min(self.cpu_mid_temp_c - self.cpu_soft_temp_c)
            .max(0.0);
        if self.hysteresis_mid_c > mid_room {
            self.hysteresis_mid_c =
                thermal_clamp_warn(self.hysteresis_mid_c, 0.0, mid_room, "hysteresis_mid_c");
        }
        let hard_room = (self.batt_hard_temp_c - self.batt_mid_temp_c)
            .min(self.cpu_hard_temp_c - self.cpu_mid_temp_c)
            .max(0.0);
        if self.hysteresis_hard_c > hard_room {
            self.hysteresis_hard_c =
                thermal_clamp_warn(self.hysteresis_hard_c, 0.0, hard_room, "hysteresis_hard_c");
        }
        // tuned 下限必须不低于硬档 cap：低于则热态 tuned 比硬档还松、这段响应毫无意义
        self.tuned_thermal_floor = thermal_clamp_warn(
            self.tuned_thermal_floor,
            self.hard_perf_cap,
            1.0,
            "tuned_thermal_floor",
        );
    }
}

// [affinity_config]
// CPU 亲和 / core_ctl 配置

/// CPU 亲和与线程迁移配置（feature.yaml `Affinity` 段）。boost 类模式（boost/vector/特调）下由 AffinityManager 应用：
/// top-app/foreground cpuset 收窄到大核+超大核、后台分组压小核、可选 uclamp.min 抬前台利用率下限、可选前台线程 sched_setaffinity 迁移
/// normal/doze 下 top-app 恢复系统布局，后台保持压小核。配置热重载即时生效
#[derive(Debug, Deserialize, Clone)]
pub struct AffinityConfig {
    /// 总开关：false 时全量恢复系统布局，不做任何写入
    #[serde(default = "crate::utils::default_true")]
    pub enabled: bool,
    /// boost 模式下 top-app 的 cpu.uclamp.min 百分比（0 = 不启用）。uclamp.min 会让 schedutil 独立于 CLG 抬频，与动态上限语义叠加，默认关闭
    #[serde(default = "d_aff_uclamp_min")]
    pub top_app_uclamp_min_pct: u32,
    /// boost 模式下 top-app 的 cpu.uclamp.max 百分比（0 = 不启用）。任务级钳制：只限 top-app 的 util 需求，schedutil 频率随之回落、EAS 能量计算同步感知；
    /// 空闲间隙微秒级降到地板，比 scaling_max_freq 硬顶更贴合 EAS。 按机型在 yaml 配置；运行时内核 < 5.3 / 节点缺失 / 写入回读无效时自动纠正关闭
    #[serde(default = "d_aff_uclamp_max")]
    pub top_app_uclamp_max_pct: u32,
    /// boost 模式下把前台进程全部线程迁移（sched_setaffinity）到大核+超大核；退出 boost 恢复全核
    #[serde(default = "crate::utils::default_true")]
    pub pin_foreground_threads: bool,
    /// 后台分组（background / restricted）的 cpu.uclamp.max 百分比（0 = 不启用）：把后台任务的 util 需求钳低（50 = 512），
    /// EAS 放置与 schedutil 频率随之回落——后台**不禁止使用任何核心**（空闲时仍会被调度上去），只是权重压低、优先落小核，给 UI/视频等线程让路；normal/boost 两态持续生效
    /// **不含 system-background**（系统后台含媒体/音频等可感知服务，保守跳过）；节点缺失/写入无效时静默跳过
    #[serde(default = "d_aff_bg_uclamp_max")]
    pub background_uclamp_max_pct: u32,
    /// normal（非 boost）模式下把 top-app/foreground 组掩码剔除 little：A510 能效差
    /// 且 DT 能耗模型低估其能耗，EAS 会把 64 位前台线程吸进 little；32 位任务允许核
    /// 交集由内核兜底每 2s 周期纠偏（框架可能把核加回）。默认 false = 保持系统布局
    /// TODO: 待内核信息（32 位核位图 / A710 核位）针对化后决定开启
    #[serde(default)]
    pub normal_fg_exclude_little: bool,
    /// 后台 promote 的核池只允许 big（不溢出 prime），且选核失败即放弃 promote、不 move_group：
    /// 依据 prime 冲顶段边际能效最差（2092800→3187200 只剩 600 Mdmips/W），可并行的批处理
    /// （HTTP 池/下载/软解）不需要 X3。false = 回到原路径（big 满溢 prime、先移组后选核）
    #[serde(default = "crate::utils::default_true")]
    pub bg_promote_exclude_prime: bool,
    /// big 饱和保护是否对所有会 promote 的模式生效：true = 亮屏任意模式（非 boost 也生效）；
    /// false = 仅 boost 态（旧行为）。判据保持「big 簇最大 util > big_high_water」
    #[serde(default = "crate::utils::default_true")]
    pub big_pressure_any_mode: bool,
    /// 支持 AArch32 的核心组名（**缺省为空 = 不限制**）：只有显式列出时才把 32 位线程的落点限制在这些簇内。
    /// 缺省留空是因为主流系统都带转译层，32 位程序实际能跑全核；只有确认无转译的机型才需要填。
    #[serde(default = "d_aff_aarch32_clusters")]
    pub aarch32_clusters: Vec<String>,
    /// 位宽感知总开关：true = 判位宽（落 `elf32` 观测帧，并按 `aarch32_clusters` 过滤落点）；
    /// false = 完全不判（旧行为）。注意：`aarch32_clusters` 为空时即使本项为 true 也不拦截。
    #[serde(default = "crate::utils::default_true")]
    pub bitwidth_aware: bool,
    /// 写去重：目标掩码已生效、或同 tid 同目标在 repin_debounce 内已写过时跳过写入与 @A 帧；false = 旧行为
    #[serde(default = "crate::utils::default_true")]
    pub write_dedup: bool,
    /// FDP（前沿主导的放置内核）总开关：true 时后台/批处理线程 promote 的**目的簇**由边际能耗成本
    /// （等算力下更低功耗）决定，取代「big 优先、钉满溢出 prime」；**默认 false**（8550 的
    /// feature.yaml 显式写 true）。false = 逐位回到既有 bg_promote_exclude_prime → big_pool 路径
    #[serde(default)]
    pub fdp_enabled: bool,
    /// FDP 迁移最小净收益（W，缺省 20 mW）：只有该次搬迁能省 ≥ 该值才动（差小于一个频档等效时不动作）
    #[serde(default = "d_aff_fdp_cost_hysteresis_w")]
    pub fdp_cost_hysteresis_w: f32,
    /// FDP 每轮每个目的簇的迁入条数硬上限（缺省 2）：同轮防 burst 靠它；**0 视为 1**（防误配成无上限）
    #[serde(default = "d_aff_fdp_max_inserts_per_round")]
    pub fdp_max_inserts_per_round: u32,
    /// FDP 目的簇落点频率上限比例（缺省 0.85，钳 0.3..=1.0）：迁入后该簇模型工作频率不得高于
    /// `该值 × 该簇最高频`——P-a「任何簇贴顶都不行」的落点
    #[serde(default = "d_aff_fdp_dst_freq_cap_ratio")]
    pub fdp_dst_freq_cap_ratio: f32,
    /// 亲和调优参数（可配化）：缺省值与原 const 逐位相等，不写 yaml 时行为与加本段前完全一致
    #[serde(default)]
    pub tuning: AffinityTuningConfig,
}

/// 亲和调优参数（feature.yaml `Affinity.tuning` 段）：所有字段缺省值取自 affinity.rs 的同名 const
/// （const 是缺省值唯一来源，避免两份数字漂移）；运行路径一律读本结构，不再直接用 const
#[derive(Debug, Deserialize, Clone)]
pub struct AffinityTuningConfig {
    /// 后台 promote util 门槛（%）
    #[serde(default = "d_aff_promote_util_pct")]
    pub promote_util_pct: f32,
    /// 小核高水位（比例 0..1）：超此值视为小核饱和、启用关键线程绑定与低升核门槛
    #[serde(default = "d_aff_little_high_water")]
    pub little_high_water: f32,
    /// 关键线程组绑定解除水位（比例 0..1，迟滞下沿）
    #[serde(default = "d_aff_key_bind_release_water")]
    pub key_bind_release_water: f32,
    /// default 小核高水位时非关键前台线程「忙」阈值（%）
    #[serde(default = "d_aff_fg_busy_util_pct")]
    pub fg_busy_util_pct: f32,
    /// normal_busy 释放水位（%）
    #[serde(default = "d_aff_fg_busy_release_util_pct")]
    pub fg_busy_release_util_pct: f32,
    /// 小核高水位时的后台 promote util 门槛（%）
    #[serde(default = "d_aff_little_promote_util_pct")]
    pub little_promote_util_pct: f32,
    /// big 簇高水位（比例 0..1）：超此值判定 big 饱和、暂停 promote
    #[serde(default = "d_aff_big_high_water")]
    pub big_high_water: f32,
    /// demote util 门槛（%）
    #[serde(default = "d_aff_demote_util_pct")]
    pub demote_util_pct: f32,
    /// demote 连续低 util 复查次数
    #[serde(default = "d_aff_demote_streak")]
    pub demote_streak: u32,
    /// 已钉线程对 score 的保守负载基准
    #[serde(default = "d_aff_pinned_weight")]
    pub pinned_weight: f32,
    /// 单核钉定上限
    #[serde(default = "d_aff_max_pins_per_core")]
    pub max_pins_per_core: u32,
    /// 过载重钉分数滞回
    #[serde(default = "d_aff_overload_margin")]
    pub overload_margin: f32,
    /// 核心过载阈值（比例 0..1）
    #[serde(default = "d_aff_core_overload_util")]
    pub core_overload_util: f32,
    /// 线程迁移最小间隔（ms）：R6 硬口径，钳制下界 = 现值 4000，不得改小/绕过
    #[serde(default = "d_aff_min_migrate_interval_ms")]
    pub min_migrate_interval_ms: u64,
    /// 过载重钉防抖（ms）：R6 硬口径，钳制下界 = 现值 8000
    #[serde(default = "d_aff_repin_debounce_ms")]
    pub repin_debounce_ms: u64,
    /// 反跳回冷却（ms）：R6 硬口径，钳制下界 = 现值 16000
    #[serde(default = "d_aff_return_cooldown_ms")]
    pub return_cooldown_ms: u64,
    /// 线程失联清理时长（s）
    #[serde(default = "d_aff_thread_stale_secs")]
    pub thread_stale_secs: u64,
}

impl Default for AffinityTuningConfig {
    fn default() -> Self {
        Self {
            promote_util_pct: d_aff_promote_util_pct(),
            little_high_water: d_aff_little_high_water(),
            key_bind_release_water: d_aff_key_bind_release_water(),
            fg_busy_util_pct: d_aff_fg_busy_util_pct(),
            fg_busy_release_util_pct: d_aff_fg_busy_release_util_pct(),
            little_promote_util_pct: d_aff_little_promote_util_pct(),
            big_high_water: d_aff_big_high_water(),
            demote_util_pct: d_aff_demote_util_pct(),
            demote_streak: d_aff_demote_streak(),
            pinned_weight: d_aff_pinned_weight(),
            max_pins_per_core: d_aff_max_pins_per_core(),
            overload_margin: d_aff_overload_margin(),
            core_overload_util: d_aff_core_overload_util(),
            min_migrate_interval_ms: d_aff_min_migrate_interval_ms(),
            repin_debounce_ms: d_aff_repin_debounce_ms(),
            return_cooldown_ms: d_aff_return_cooldown_ms(),
            thread_stale_secs: d_aff_thread_stale_secs(),
        }
    }
}

impl AffinityTuningConfig {
    /// 范围钳制：百分比 0..=100、比例 0..=1、pinned_weight 0..=2、max_pins_per_core 1..=16、
    /// demote_streak 1..=20；三道时间闸（min_migrate/repin/return）下界恒为现值（R6：不得改小/绕过）
    pub fn normalize(&mut self) {
        self.promote_util_pct = self.promote_util_pct.clamp(0.0, 100.0);
        self.fg_busy_util_pct = self.fg_busy_util_pct.clamp(0.0, 100.0);
        self.fg_busy_release_util_pct = self.fg_busy_release_util_pct.clamp(0.0, 100.0);
        self.little_promote_util_pct = self.little_promote_util_pct.clamp(0.0, 100.0);
        self.demote_util_pct = self.demote_util_pct.clamp(0.0, 100.0);
        self.little_high_water = self.little_high_water.clamp(0.0, 1.0);
        self.key_bind_release_water = self.key_bind_release_water.clamp(0.0, 1.0);
        self.big_high_water = self.big_high_water.clamp(0.0, 1.0);
        self.core_overload_util = self.core_overload_util.clamp(0.0, 1.0);
        self.overload_margin = self.overload_margin.clamp(0.0, 1.0);
        self.pinned_weight = self.pinned_weight.clamp(0.0, 2.0);
        self.max_pins_per_core = self.max_pins_per_core.clamp(1, 16);
        self.demote_streak = self.demote_streak.clamp(1, 20);
        self.min_migrate_interval_ms = self.min_migrate_interval_ms.clamp(4_000, 60_000);
        self.repin_debounce_ms = self.repin_debounce_ms.clamp(8_000, 60_000);
        self.return_cooldown_ms = self.return_cooldown_ms.clamp(16_000, 60_000);
        self.thread_stale_secs = self.thread_stale_secs.clamp(1, 600);
    }

    pub fn min_migrate_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.min_migrate_interval_ms)
    }
    pub fn repin_debounce(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.repin_debounce_ms)
    }
    pub fn return_cooldown(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.return_cooldown_ms)
    }
    pub fn thread_stale(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.thread_stale_secs)
    }
}

fn d_aff_uclamp_min() -> u32 {
    0
}
fn d_aff_uclamp_max() -> u32 {
    0
}
fn d_aff_bg_uclamp_max() -> u32 {
    50
}
/// `aarch32_clusters` 缺省 = **空**（不限制）：主流系统带转译层，32 位程序可跑全核；
/// 无转译的机型需显式列出支持 AArch32 的簇（如 `["little"]`）才恢复「32 位只上小核」的约束
fn d_aff_aarch32_clusters() -> Vec<String> {
    Vec::new()
}
/// FDP 阈值缺省（缺省值唯一来源 = affinity.rs 的同名 const）
fn d_aff_fdp_cost_hysteresis_w() -> f32 {
    crate::chiri::affinity::FDP_COST_HYSTERESIS_W
}
fn d_aff_fdp_max_inserts_per_round() -> u32 {
    crate::chiri::affinity::FDP_MAX_INSERTS_PER_ROUND
}
fn d_aff_fdp_dst_freq_cap_ratio() -> f32 {
    crate::chiri::affinity::FDP_DST_FREQ_CAP_RATIO
}
// 缺省值唯一来源 = affinity.rs 的同名 const（改 const 即改缺省，不在两处写数字）
fn d_aff_promote_util_pct() -> f32 {
    crate::chiri::affinity::PROMOTE_UTIL_PCT
}
fn d_aff_little_high_water() -> f32 {
    crate::chiri::affinity::LITTLE_HIGH_WATER
}
fn d_aff_key_bind_release_water() -> f32 {
    crate::chiri::affinity::KEY_BIND_RELEASE_WATER
}
fn d_aff_fg_busy_util_pct() -> f32 {
    crate::chiri::affinity::FG_BUSY_UTIL_PCT
}
fn d_aff_fg_busy_release_util_pct() -> f32 {
    crate::chiri::affinity::FG_BUSY_RELEASE_UTIL_PCT
}
fn d_aff_little_promote_util_pct() -> f32 {
    crate::chiri::affinity::LITTLE_PROMOTE_UTIL_PCT
}
fn d_aff_big_high_water() -> f32 {
    crate::chiri::affinity::BIG_HIGH_WATER
}
fn d_aff_demote_util_pct() -> f32 {
    crate::chiri::affinity::DEMOTE_UTIL_PCT
}
fn d_aff_demote_streak() -> u32 {
    crate::chiri::affinity::DEMOTE_STREAK
}
fn d_aff_pinned_weight() -> f32 {
    crate::chiri::affinity::PINNED_WEIGHT
}
fn d_aff_max_pins_per_core() -> u32 {
    crate::chiri::affinity::MAX_PINS_PER_CORE
}
fn d_aff_overload_margin() -> f32 {
    crate::chiri::affinity::OVERLOAD_MARGIN
}
fn d_aff_core_overload_util() -> f32 {
    crate::chiri::affinity::CORE_OVERLOAD_UTIL
}
fn d_aff_min_migrate_interval_ms() -> u64 {
    crate::chiri::affinity::MIN_MIGRATE_INTERVAL.as_millis() as u64
}
fn d_aff_repin_debounce_ms() -> u64 {
    crate::chiri::affinity::REPIN_DEBOUNCE.as_millis() as u64
}
fn d_aff_return_cooldown_ms() -> u64 {
    crate::chiri::affinity::RETURN_COOLDOWN.as_millis() as u64
}
fn d_aff_thread_stale_secs() -> u64 {
    crate::chiri::affinity::THREAD_STALE.as_secs()
}

impl Default for AffinityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            top_app_uclamp_min_pct: d_aff_uclamp_min(),
            top_app_uclamp_max_pct: d_aff_uclamp_max(),
            pin_foreground_threads: true,
            background_uclamp_max_pct: d_aff_bg_uclamp_max(),
            normal_fg_exclude_little: false,
            bg_promote_exclude_prime: true,
            big_pressure_any_mode: true,
            aarch32_clusters: d_aff_aarch32_clusters(),
            bitwidth_aware: true,
            write_dedup: true,
            fdp_enabled: false,
            fdp_cost_hysteresis_w: d_aff_fdp_cost_hysteresis_w(),
            fdp_max_inserts_per_round: d_aff_fdp_max_inserts_per_round(),
            fdp_dst_freq_cap_ratio: d_aff_fdp_dst_freq_cap_ratio(),
            tuning: AffinityTuningConfig::default(),
        }
    }
}

impl AffinityConfig {
    /// 校验：uclamp 百分比限制在 0..=100；tuning 各字段范围钳制
    pub fn normalize(&mut self) {
        self.top_app_uclamp_min_pct = self.top_app_uclamp_min_pct.clamp(0, 100);
        self.top_app_uclamp_max_pct = self.top_app_uclamp_max_pct.clamp(0, 100);
        self.background_uclamp_max_pct = self.background_uclamp_max_pct.clamp(0, 100);
        // FDP 阈值：非有限回退默认；滞回钳 0..=1、迁入条数钳 1..=16（0 视为 1，不设上限是误配）、
        // 落点频率上限比例钳 0.3..=1.0
        if !self.fdp_cost_hysteresis_w.is_finite() {
            self.fdp_cost_hysteresis_w = d_aff_fdp_cost_hysteresis_w();
        }
        self.fdp_cost_hysteresis_w = self.fdp_cost_hysteresis_w.clamp(0.0, 1.0);
        self.fdp_max_inserts_per_round = self.fdp_max_inserts_per_round.clamp(1, 16);
        if !self.fdp_dst_freq_cap_ratio.is_finite() {
            self.fdp_dst_freq_cap_ratio = d_aff_fdp_dst_freq_cap_ratio();
        }
        self.fdp_dst_freq_cap_ratio = self.fdp_dst_freq_cap_ratio.clamp(0.3, 1.0);
        self.tuning.normalize();
    }
}

/// core_ctl（内核核心在线控制器）接管配置（feature.yaml `CoreCtl` 段）。boost 模式下把各 cluster 的 min_cpus 抬到全组常在线，防低负载热插拔与 ChiRi 调频打架；
/// 退出 boost 恢复快照。仅动 min_cpus
// [corectl_config]
#[derive(Debug, Deserialize, Clone)]
pub struct CoreCtlConfig {
    /// 总开关：false 时不写任何 core_ctl 节点
    #[serde(default = "crate::utils::default_true")]
    pub enabled: bool,
    /// scenemode 是否下线核心（息屏深度省电）。true = 进入 scenemode 时下线 little 簇（除引导核）
    /// 并用 WALT core_ctl `max_cpus` 钳住保留核数（见 `core_ctl.rs` 的 `scenemode_targets`）；
    /// false = 不下线任何核，仅靠 scenemode.yaml 的频率上限（`perf_ceil`）+ uclamp 压制省电。
    /// 2026-10-01 恢复字段效力（此前误标「已停用」，实为 mod.rs 未接线、由 `enabled` 无条件驱动）：
    /// 8550 实测饱和抖动（26 次进入中 20 次 `持续顶满（little util 100%）→ 退回 reduce + 300s 冷却`，
    /// 见 logd_1001-045147）置 false，消除「下线到单核 → 单核顶满 → 自我撤销」拉锯。
    #[serde(default = "crate::utils::default_true")]
    pub scenemode_offline: bool,
}

impl Default for CoreCtlConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            scenemode_offline: true,
        }
    }
}

/// 档位桶号的防御性上限（真机前沿表只有几十个桶；越界由查表回退 hw_max，不在这里判错）
const MAX_TIER_BUCKET: usize = 4096;

/// PowerBase 配置（feature.yaml `powerbase` 段）：以功耗为指标的调频参数
/// 与 CLG 的根本区别：CLG 只看「利用率够不够」，PowerBase 看「功耗超没超目标」——
/// 放电状态下以 `target_power_w` 为闸门，超了就不再升频（除非确实压不住：满占用核心
/// 占比达 `overload_cores_pct` 且持续 `overload_hold_ms`）。降频**恒激进**，不看功耗。
/// 档位体系（`tier_*`）把频率上界钉在能效前沿表的固定桶上：标准档工作、触摸事件沿
/// 阶梯瞬时放开（含功耗豁免窗口），见 `power_base.rs`
#[derive(Debug, Deserialize, Clone)]
pub struct PowerBaseConfig {
    /// 放电状态下的目标功耗（W）：功耗低于它时按 `up_headroom_below` 放宽升频，达到或超过它时守住不升（过载判定除外）。各 SoC 在自己的 feature.yaml 里覆盖
    #[serde(default = "d_pb_target_power")]
    pub target_power_w: f32,
    /// 功耗低于目标时的升频宽松度（>1 = 放宽，1.0 = 不放宽）
    #[serde(default = "d_pb_up_headroom")]
    pub up_headroom_below: f32,
    /// 降频激进度：目标性能乘以它（0.5 = 直接砍半），**与功耗无关，恒生效**
    #[serde(default = "d_pb_down_scale")]
    pub down_scale: f32,
    /// 功耗已达上限时仍允许升频的门槛：满占用（100%）核心占比（%）
    #[serde(default = "d_pb_overload_cores")]
    pub overload_cores_pct: f32,
    /// 上面那个占比需持续这么久（ms）才放行升频
    #[serde(default = "d_pb_overload_hold")]
    pub overload_hold_ms: u64,
    /// 触摸事件时允许短暂突破功率上限的窗口（ms）；档位体系启用时同时是**触摸升档窗口**
    #[serde(default = "d_pb_touch_break")]
    pub touch_break_ms: u64,
    /// 频率上限档（真机能效前沿表 `demand_buckets` 的桶号，0 = 不启用档位体系 → 沿用 hw_max 满频行为）：
    /// 标准上限 / 触摸一级 / 触摸二级（绝对上限）。三者任一为 0 即整体不启用；
    /// 非 PF SoC（无前沿表）或桶号越界同样静默回退 hw_max，行为与不启用一致
    #[serde(default)]
    pub tier_standard: usize,
    #[serde(default)]
    pub tier_touch: usize,
    #[serde(default)]
    pub tier_max: usize,
}

fn d_pb_target_power() -> f32 {
    3.0
}
fn d_pb_up_headroom() -> f32 {
    1.15
}
fn d_pb_down_scale() -> f32 {
    0.5
}
fn d_pb_overload_cores() -> f32 {
    50.0
}
fn d_pb_overload_hold() -> u64 {
    3000
}
fn d_pb_touch_break() -> u64 {
    400
}

impl Default for PowerBaseConfig {
    fn default() -> Self {
        Self {
            target_power_w: d_pb_target_power(),
            up_headroom_below: d_pb_up_headroom(),
            down_scale: d_pb_down_scale(),
            overload_cores_pct: d_pb_overload_cores(),
            overload_hold_ms: d_pb_overload_hold(),
            touch_break_ms: d_pb_touch_break(),
            tier_standard: 0,
            tier_touch: 0,
            tier_max: 0,
        }
    }
}

impl PowerBaseConfig {
    /// 参数钳制：非有限值回退默认，比例类限制在合理区间
    pub fn normalize(&mut self) {
        if !self.target_power_w.is_finite() || self.target_power_w <= 0.0 {
            self.target_power_w = d_pb_target_power();
        }
        self.target_power_w = self.target_power_w.clamp(0.5, 30.0);
        if !self.up_headroom_below.is_finite() {
            self.up_headroom_below = d_pb_up_headroom();
        }
        self.up_headroom_below = self.up_headroom_below.clamp(1.0, 2.0);
        if !self.down_scale.is_finite() {
            self.down_scale = d_pb_down_scale();
        }
        self.down_scale = self.down_scale.clamp(0.1, 1.0);
        if !self.overload_cores_pct.is_finite() {
            self.overload_cores_pct = d_pb_overload_cores();
        }
        self.overload_cores_pct = self.overload_cores_pct.clamp(1.0, 100.0);
        self.overload_hold_ms = self.overload_hold_ms.clamp(0, 30_000);
        self.touch_break_ms = self.touch_break_ms.clamp(0, 5_000);
        // 档位桶号：仅做防御性上限（表只有几十个桶；越界由查表回退 hw_max，不在这里判错）
        self.tier_standard = self.tier_standard.min(MAX_TIER_BUCKET);
        self.tier_touch = self.tier_touch.min(MAX_TIER_BUCKET);
        self.tier_max = self.tier_max.min(MAX_TIER_BUCKET);
        // 阶梯必须全非 0 且严格递增，否则「触摸升档」会变成降频：不猜配置意图（不重排），
        // 整段置 0 = 不启用档位体系，逐位回退 hw_max 满频行为
        let (a, b, c) = (self.tier_standard, self.tier_touch, self.tier_max);
        if a == 0 || b == 0 || c == 0 || !(a < b && b < c) {
            self.tier_standard = 0;
            self.tier_touch = 0;
            self.tier_max = 0;
        }
    }
}

/// 顶层配置（feature.yaml 调优段 + meta.yaml 抬头）
// [config_root]
#[derive(Debug, Deserialize, Default)]
pub struct Config {
    /// 全局元信息：日志级别 / 语言
    #[serde(default, alias = "Meta")]
    pub meta: Meta,
    /// 功能开关
    #[serde(default)]
    pub function: FunctionToggles,
    /// IO 优化参数
    #[serde(default, rename = "IO_Settings")]
    pub io_settings: IOSettings,
    /// cpuidle 参数
    #[serde(default, rename = "CpuIdle")]
    pub cpu_idle: CpuIdle,

    // 按场景划分的性能模式：键名即 mode，get_mode 按名检索
    #[serde(default)]
    pub reduce: Mode,
    #[serde(default)]
    pub default: Mode,
    #[serde(default)]
    pub boost: Mode,
    #[serde(default)]
    pub vector: Mode,
    /// PowerBase（Stardust 家族）：以放电功耗为指标的调频器参数（开关在 meta 段）。未定义该段时用代码默认值（见 `PowerBaseConfig::default`）
    #[serde(default)]
    pub powerbase: PowerBaseConfig,
    /// 息屏场景模式（scenemode）：屏幕熄灭超过 `scene_mode_delay_secs` 秒后切换到的低功耗配置（压低频率上限、禁止主动升频），亮屏后恢复原模式；未定义时回退 CLG 默认参数
    #[serde(default)]
    pub scenemode: Mode,
    /// 息屏进入 scenemode 的延迟（秒）：默认 300s（5 分钟），YAML 可覆盖
    #[serde(default = "default_scene_mode_delay_secs")]
    pub scene_mode_delay_secs: u64,

    /// 热保护配置：按 CPU 温度动态压低 CLG 性能上限（采样在 scheduler_ipc 线程）。省略该段时用代码默认值（enabled=true，温度阈值见 ThermalGuardConfig 各缺省值）
    #[serde(default, rename = "Thermal")]
    pub thermal: ThermalGuardConfig,

    /// 特调缺省参数段（`akmode`，明日方舟特调）：来自嵌入的 config/normal/tuned_profiles.yaml （编译期打包，非处理器绑定），与 CLG 完全解耦
    /// 前台为白名单应用时由 TunedGovernor 接管，参数不走 CLG；同时充当 tuned_profiles 里未注册模式名的回退参数
    #[serde(default)]
    pub akmode: SpecialTunedConfig,

    /// 特调参数组：按特调模式名分派（模式名在 special_tuned.yaml 的模式列表中注册），即同一份 tuned_profiles.yaml 的 `tuned_profiles:` 段；
    /// 未注册的模式回退 `akmode` 段。全部特调共用 TunedGovernor 连续控制机制，差别只在参数（playback 视频稳态 / daily 交互轻载 / …）
    #[serde(default)]
    pub tuned_profiles: std::collections::HashMap<String, SpecialTunedConfig>,

    /// CPU 亲和与线程迁移控制（cpuset / cpuctl uclamp / sched_setaffinity）
    #[serde(default, rename = "Affinity")]
    pub affinity: AffinityConfig,

    /// core_ctl 核心在线接管（boost 模式保持大核常在线）
    #[serde(default, rename = "CoreCtl")]
    pub core_ctl: CoreCtlConfig,

    /// 内核调度器参数微调（Sched 段，借鉴 LittleYouran CTS 的 Scheduler 段）：与模式无关的一次性系统设置，
    /// 执行器按白名单写 /proc/sys/kernel/<key>（白名单 chiri::scheduler::SCHED_ALLOWED_PARAMS，防任意内核参数写入）
    #[serde(default, rename = "Sched")]
    pub sched: SchedTuning,
}

/// 内核调度器参数微调配置：`params` 的 key 为 /proc/sys/kernel/ 下的节点名，value 为写入值（空串跳过）；未列入白名单的 key 打 warn 后跳过
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SchedTuning {
    /// 总开关：false 时不写任何节点
    pub enabled: bool,
    pub params: std::collections::HashMap<String, String>,
}

/// scenemode 延迟缺省值：5 分钟
fn default_scene_mode_delay_secs() -> u64 {
    300
}

// [config_impl]
impl Config {
    /// 加载生效配置：feature 段以嵌入 feature.yaml 为基准（磁盘不落盘，防篡改），meta 以嵌入 meta.yaml 为默认值（common::embedded_meta_defaults），
    /// 再被磁盘 meta.yaml 覆盖（sync_meta_snapshot 已先行校验/纠正；缺失或非法时沿用嵌入默认）。`path` 为生效 meta.yaml 路径（common::get_config_path()
    /// ）。加载后合并嵌入的 akmode/scenemode 段，并把功能总开关同步到进程级原子标志（fas_available / scenemode 判定读取）
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let mut config: Config = serde_yaml::from_str(crate::common::embedded_feature_str())?;
        // 缺省 = 沿用上一层：先内嵌默认，再按磁盘文件「写了哪个字段才覆盖哪个」
        let d = crate::common::embedded_meta_defaults();
        apply_meta_overrides(&mut config.meta, &d);
        if let Some(m) = crate::common::read_external_meta(std::path::Path::new(path)) {
            apply_meta_overrides(&mut config.meta, &m);
        }
        // 功能总开关同步到进程级原子标志（覆盖启动 + config_watcher 热重载两条路径）
        crate::common::set_fas_enabled(config.meta.fas_enabled);
        crate::common::set_scenemode_enabled(config.meta.scenemode_enabled);
        crate::common::set_powerbase_enabled(config.meta.powerbase_enabled);
        crate::common::set_screen_off_value(config.meta.screen_off_value);
        // 线程功能总闸与机型内的两个子开关取「与」：关掉后 affinity 走 release()（逐线程恢复全核 + cpuset/uclamp 快照回写）、
        // core_ctl 回 Normal（恢复 min_cpus/online 快照）——即「把绑定分配全部改成全核心」热重载后由scheduler_ipc 的 config_dirty 分支调
        // apply_affinity_and_corectl 落地，周期块（2s）也会兜一次。thread_bind 是实验室 frozen 专用闸（用户开关已移除、默认 true）：frozen 期间为 false，
        // 交还线程亲和/绑核与 core_ctl；其余场合恒 true 等于不干预
        config.affinity.enabled &= config.meta.thread_bind;
        config.core_ctl.enabled &= config.meta.thread_bind;
        config.merge_tuned_profiles();
        config.merge_scenemode();
        config.meta.normalize();
        config.thermal.normalize();
        // 热压制「重钳」开关同步到 CLG 层（进程级原子量，同 fas/scenemode 口径）：true（默认）= cap 窗口内恒钳所有簇，false = 回退 free_above 豁免旧行为
        // 见 cpu_load_governor.rs 的 [thermal_clamp]
        crate::chiri::cpu_load_governor::set_clamp_heavy(config.thermal.clamp_heavy);
        // 热态 tuned 响应（开关 + 性能下限）同点同步到 tuned 层（tuned.rs [thermal_ceil]）：播放态占会话 68% 能量却原本对温度零响应，这是补上那条链路的配置面
        crate::chiri::tuned::set_thermal_response(
            config.thermal.tuned_resp_enabled,
            config.thermal.tuned_thermal_floor,
        );
        // CPU 温度 zone 名单同步到探测层（utils [temp_probe]）名单来自编译期嵌入的 feature.yaml（进程内恒定），set 仅首次生效，热重载重复调用无副作用
        crate::utils::set_cpu_temp_zone_types(config.thermal.cpu_temp_zone_types.clone());
        config.affinity.normalize();
        // PowerBase 参数同样在加载处钳制（各段统一口径，别等 init 时才钳）
        config.powerbase.normalize();
        // 参数下发到进程级副本：PowerBase 是 CLG 的备用后端，接管那一刻从这里取值（与总开关同一处同步）
        crate::common::set_powerbase_config(config.powerbase.clone());
        // 电池读数选项同步到遥测层（原子量，热重载即时生效）。倍电压/倍电流与私有节点互斥：私有开关打开时这里强制关掉它们——UI 侧同时置灰并清值，手改 meta 也兜得住
        crate::monitor::telemetry::set_battery_options(
            config.meta.oplus_chg,
            config.meta.oplus_dual_cell,
            !config.meta.oplus_chg && config.meta.voltage_double,
            !config.meta.oplus_chg && config.meta.current_double,
            config.meta.voltage_divisor,
            config.meta.current_divisor,
        );
        Ok(config)
    }

    /// 合并嵌入的特调参数组（tuned_profiles.yaml，编译期打包进二进制）。嵌入内容随版本发布、始终存在；仅当嵌入 YAML 意外损坏时置特调不可用
    fn merge_tuned_profiles(&mut self) {
        match serde_yaml::from_str::<Config>(crate::common::embedded_tuned_profiles_str()) {
            Ok(special) => {
                self.akmode = special.akmode;
                self.akmode.normalize();
                // 特调参数组同样逐组钳制；未列出的模式由 get_tuned_profile 回退 akmode 段
                self.tuned_profiles = special.tuned_profiles;
                for cfg in self.tuned_profiles.values_mut() {
                    cfg.normalize();
                }
                crate::common::set_special_tuned_available(true);
                // 白名单注册了模式、却没有对应参数组时会静默回退 akmode 段（游戏参数在省电场景是反效果）——拼写错/漏配不能静默生效（akmode 本身用缺省段，跳过）
                for m in crate::common::special_tuned_mode_names() {
                    if m != "akmode" && !self.tuned_profiles.contains_key(&m) {
                        log::warn!(
                            "{}",
                            t_with_args(
                                "tuned-profile-missing",
                                &fluent_args!("mode" => m.clone())
                            )
                        );
                    }
                }
                log::debug!(
                    "{}",
                    t_with_args(
                        "config-special-merged",
                        &fluent_args!("path" => "<embedded>".to_string())
                    )
                );
            }
            Err(e) => {
                log::warn!(
                    "{}",
                    t_with_args(
                        "config-special-parse-failed",
                        &fluent_args!("path" => "<embedded>".to_string(), "error" => e.to_string())
                    )
                );
                crate::common::set_special_tuned_available(false);
            }
        }
    }

    /// 合并嵌入的 scenemode 配置。只反序列化 scenemode 段（先解析成 Value 再提取）：段缺失时保持 feature.yaml 已配置的值，而不是用默认值覆盖
    fn merge_scenemode(&mut self) {
        let scene_value = match serde_yaml::from_str::<serde_yaml::Value>(
            crate::common::embedded_scenemode_str(),
        ) {
            Ok(value) => match value.get("scenemode").cloned() {
                Some(v) => v,
                // 嵌入文件无 scenemode 段：保持当前已配置的 scenemode
                None => return,
            },
            Err(e) => {
                log::warn!(
                    "{}",
                    t_with_args(
                        "config-scenemode-parse-failed",
                        &fluent_args!("path" => "<embedded>".to_string(), "error" => e.to_string())
                    )
                );
                return;
            }
        };
        match serde_yaml::from_value::<Mode>(scene_value) {
            Ok(scene_config) => {
                self.scenemode = scene_config;
                log::debug!(
                    "{}",
                    t_with_args(
                        "config-scenemode-merged",
                        &fluent_args!("path" => "<embedded>".to_string())
                    )
                );
            }
            Err(e) => {
                log::warn!(
                    "{}",
                    t_with_args(
                        "config-scenemode-parse-failed",
                        &fluent_args!("path" => "<embedded>".to_string(), "error" => e.to_string())
                    )
                );
            }
        }
    }

    /// 按特调模式名取参数组：`tuned_profiles.<mode>` 优先，缺省回退 `akmode` 段
    /// 返回 owned（调用点本就 clone 给 init_policies/reload_config，避免借用纠缠）
    /// **全部特调取参的唯一入口**——不要再加第二个取参方法（两个入口必然搞混）
    pub fn get_tuned_profile(&self, mode: &str) -> SpecialTunedConfig {
        self.tuned_profiles
            .get(mode)
            .cloned()
            .unwrap_or_else(|| self.akmode.clone())
    }

    /// 按模式名取对应 CLG 配置段；未知模式（含特调模式）返回 None。 特调模式（akmode）不走 CLG，由 TunedGovernor 独立接管
    pub fn get_mode(&self, mode_name: &str) -> Option<&Mode> {
        match mode_name {
            "reduce" => Some(&self.reduce),
            "default" => Some(&self.default),
            "boost" => Some(&self.boost),
            "vector" => Some(&self.vector),
            _ => None,
        }
    }
}
