//! power_base.rs: [types] [init] [tick] [touch] [release]
//! PowerBase（Stardust 家族）：以**放电功耗**为指标的调频器，用来替换 CLG
//! 与 CLG 的根本区别：CLG 只看「利用率够不够」，PowerBase 看「功耗超没超目标」（feature 的 target_power_w）——
//! 功耗低于目标：升频放宽（up_headroom_below）；达到/超过：守住不再升频，除非满占用核心占比达
//! overload_cores_pct 且持续 overload_hold_ms；降频恒激进（down_scale 直接砍目标，与当前功耗无关）
//! 档位体系（`tier_standard/tier_touch/tier_max`，真机能效前沿表的桶号）：频率上界钉在桶落点上，
//! 标准档工作、触摸事件沿阶梯瞬时放开（标准 → 触摸一级 → 触摸二级，已在顶档忽略本次调频），
//! 窗口（touch_break_ms）内允许突破 target_power_w，过期回落标准档
//! **不独占 owner**：本模块是 `CpuLoadGovernor` 的备用后端（与 PF 同款的可选后端），由它的接管入口
//! 在 Worker 与本模块之间二选一（见 cpu_load_governor.rs [backend]）；本模块对外不暴露接管的开关，
//! release/init 只在 CLG 内部被调。FAS / 场景特调 / DOWN 停摆 / 实验室的启停判据一律不受影响
//! 结构照 CLG 口径（快照/释放、5s 防篡改重写兜底收敛），**只压上限**：min 一次性锁硬件最低、下限交回默认调速器（选型见 common.rs [governor]），max 按功耗规则动态算每 policy 目标频率

use crate::chiri::config::PowerBaseConfig;
use crate::utils::FastWriter;
use log::{info, warn};
use std::fs;
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

/// 防篡改重写间隔（与 fast.rs 同口径；默认无竞争者，重写是异常兜底收敛）
const REWRITE_INTERVAL: Duration = Duration::from_secs(5);
/// 性能比死区：目标与当前的差小于它就认为「已经到位」不写频率（挡住利用率反馈滞后的抖动，sysfs 写入降到只在真正变化时）
const PERF_DEADBAND: f32 = 0.05;
/// 单次降频的最大幅度（性能比）：`down_scale` 再激进也不越过它，避免一步砍半导致下一 tick util 反弹成振荡
const MAX_DOWN_STEP: f32 = 0.25;
/// 档位阶梯级数：标准 → 触摸一级 → 触摸二级（绝对上限）
const TIER_STEPS: usize = 3;

// [types]
/// 接管前的系统状态快照，release 时恢复
struct PolicySnapshot {
    policy_id: i32,
    governor: Option<String>,
    min_freq: Option<u32>,
    max_freq: Option<u32>,
}

/// 单个 policy 的运行状态
struct BasePolicy {
    cpus: Vec<usize>,
    hw_min: u32,
    /// 可用频率表（升序），目标频率就近取档；最高档 = 表尾（档位未启用时的上界）
    freqs: Vec<u32>,
    /// 档位落点 [标准, 触摸一级, 触摸二级]，已 floor 对齐到本簇频率表；
    /// 未启用 / 无前沿表 / 桶越界 / 该桶该簇无目标 → 全 = hw_max（与不启用行为一致）
    tier_ceils: [u32; TIER_STEPS],
    /// 当前生效的频率上界（= tier_ceils[当前档位]）
    ceil: u32,
    perf: f32,
    max_writer: FastWriter,
    min_writer: FastWriter,
}

impl BasePolicy {
    /// 性能比 → 频率档位：`[hw_min, ceil]` 内线性映射，再取 ≤ 目标的最大档（floor 对齐）；
    /// perf 是功耗预算内允许的上限，落点不得高于它；目标落两档之间时内核本就把 max 向下 clamp
    fn freq_for(&self, perf: f32) -> u32 {
        perf_to_freq(self.hw_min, self.ceil, &self.freqs, perf)
    }
}

/// 性能比 → 频率档位（纯函数，便于单测）：`[hw_min, ceil]` 线性映射后取频率表内 ≤ 目标的最大档；
/// ceil 低于 hw_min（配置错）按 hw_min 处理，表内无 ≤ 目标档时同样落 hw_min
fn perf_to_freq(hw_min: u32, ceil: u32, freqs: &[u32], perf: f32) -> u32 {
    let ceil = ceil.max(hw_min);
    let want = hw_min as f32 + (ceil - hw_min) as f32 * perf.clamp(0.0, 1.0);
    let want = want as u32;
    freqs
        .iter()
        .rev()
        .copied()
        .find(|f| *f <= want)
        .unwrap_or(hw_min)
}

/// 档位阶梯推进：返回下一档；None = 已在最高档（触摸时**忽略本次调频**，只续窗口）
fn next_tier(idx: u8) -> Option<u8> {
    let next = idx.saturating_add(1);
    ((next as usize) < TIER_STEPS).then_some(next)
}

/// 功耗闸门状态
#[derive(Default)]
struct PowerGate {
    /// 过载（满占用核心占比达标）的起始时刻
    overload_since: Option<Instant>,
}

pub struct PowerBase {
    cfg: PowerBaseConfig,
    policies: Vec<BasePolicy>,
    snapshots: Vec<PolicySnapshot>,
    gate: PowerGate,
    active: bool,
    last_write: Instant,
    /// 档位体系是否启用：配置三个桶号都非 0 **且**至少一个 policy 取到了前沿表落点
    tier_enabled: bool,
    /// 当前档位索引（0=标准 1=触摸一级 2=触摸二级），所有 policy 同步
    tier_idx: u8,
    /// 触摸窗口截止（None = 不在窗口内）：窗口内允许突破 `target_power_w`，且是升档的存续期
    touch_until: Option<Instant>,
}

impl Default for PowerBase {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerBase {
    pub fn new() -> Self {
        Self {
            cfg: PowerBaseConfig::default(),
            policies: Vec::new(),
            snapshots: Vec::new(),
            gate: PowerGate::default(),
            active: false,
            last_write: Instant::now(),
            tier_enabled: false,
            tier_idx: 0,
            touch_until: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    // [init]
    /// 接管全部 cpufreq policy：读可用频率、快照原状态、写默认调速器（选型见 common.rs [governor]），
    /// 并把上限**立刻设到标准档**（内部 perf 同步为 1.0）；min 一次性压到硬件最低后不再动，与 CLG
    /// 同口径（只压上限，下限交回默认调速器）。
    /// 必须接管即写一次：否则 perf 与硬件实际状态不一致，首个降频 tick 无所适从。
    /// 起手取标准档（档位体系未启用时 = 硬件最高，与旧行为一致）：接管瞬间多有负载，先给足日常余量防掉帧，
    /// 紧随 tick 按功耗规则迅速压下；特需档只在触摸事件里瞬时放开
    pub fn init(&mut self, cfg: &PowerBaseConfig) {
        self.release();
        self.cfg = cfg.clone();
        self.cfg.normalize();

        let policies = crate::chiri::get_cpu_policies();
        let ranges = crate::common::chiri_core_ranges();
        // 任一 policy 取到档位落点即视为体系可用（表是 SoC 级资产，正常三簇同有同无）
        let mut tier_any = false;

        for policy in &policies {
            let pid = policy.id;
            let gov_path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                pid
            );
            let min_path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_min_freq",
                pid
            );
            let max_path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_max_freq",
                pid
            );
            let freq_path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_available_frequencies",
                pid
            );

            let mut freqs: Vec<u32> = fs::read_to_string(&freq_path)
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            if freqs.is_empty() {
                // scaling_available_frequencies 读不到/空表：按 policy 首核映射核心组，回退 soc.yaml [freq_khz] 兜底（只补表，
                // 不改取档/floor 对齐逻辑）
                match crate::common::soc_freq_fallback_for_policy(pid) {
                    Some(f) => freqs = f,
                    None => continue,
                }
            }
            freqs.sort_unstable();
            freqs.dedup();
            let hw_min = *freqs.first().unwrap();
            let hw_max = *freqs.last().unwrap();

            let governor = fs::read_to_string(&gov_path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let min_freq = fs::read_to_string(&min_path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
            let max_freq = fs::read_to_string(&max_path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
            self.snapshots.retain(|r| r.policy_id != pid);
            self.snapshots.push(PolicySnapshot {
                policy_id: pid,
                governor,
                min_freq,
                max_freq,
            });

            let _ = crate::utils::try_write_file(
                &gov_path,
                crate::common::cpu_governor_for_policy(pid),
            );

            let mut max_writer = FastWriter::new(max_path.clone());
            let mut min_writer = FastWriter::new(min_path.clone());
            if !max_writer.is_valid() || !min_writer.is_valid() {
                warn!(
                    "{}",
                    t_with_args(
                        "powerbase-writer-invalid",
                        &fluent_args!("pid" => pid.to_string())
                    )
                );
                continue;
            }

            // 该 policy 覆盖的 CPU：policy 名就是首个 CPU id，落到哪个区间就属于哪簇
            let id = pid.max(0) as usize;
            let cpus: Vec<usize> = if ranges.little.contains(&id) {
                ranges.little.clone().collect()
            } else if ranges.big.contains(&id) {
                ranges.big.clone().collect()
            } else {
                ranges.prime.clone().collect()
            };

            // 档位落点 [标准, 触摸一级, 触摸二级]：取真机能效前沿表的逐桶落点（frontier_aligned 已
            // floor 对齐到本簇频率表，索引 = 桶号）。配置三桶号全非 0 且取到落点才算启用；任一缺失
            // （非 PF SoC / 段缺失 / 桶越界 / 该桶该簇无目标）→ 全 hw_max，行为与不启用档位体系一致
            let mut tier_ceils = [hw_max; TIER_STEPS];
            let cfg_tiers = (
                self.cfg.tier_standard,
                self.cfg.tier_touch,
                self.cfg.tier_max,
            );
            if cfg_tiers.0 > 0 && cfg_tiers.1 > 0 && cfg_tiers.2 > 0 {
                let aligned = crate::common::core_group_of(id as u32)
                    .and_then(|g| crate::common::frontier_aligned(g, &freqs));
                if let Some(aligned) = aligned {
                    let pick = |b: usize| aligned.get(b).copied().filter(|&v| v > 0);
                    if let (Some(a), Some(b), Some(c)) =
                        (pick(cfg_tiers.0), pick(cfg_tiers.1), pick(cfg_tiers.2))
                    {
                        tier_ceils = [
                            a.clamp(hw_min, hw_max),
                            b.clamp(hw_min, hw_max),
                            c.clamp(hw_min, hw_max),
                        ];
                        tier_any = true;
                    }
                }
            }

            // 起手把 min 压到硬件最低、max 写到标准档上界，写完才把 perf 记为 1.0；失败保持 0.0，下一 tick 按目标重试。
            // min 一次性压到硬件最低并保持（CLG 口径：只压上限，下限交回默认调速器），此后只写 max——
            // 写序必须是「先 min 后 max」：接管前系统 min 可能高于标准档，先压 max 会被内核以 min>max 拒
            let _ = min_writer.write_value_force(hw_min);
            let start_ceil = tier_ceils[0];
            let mut perf = 0.0_f32;
            if max_writer.write_value_force(start_ceil) {
                perf = 1.0;
            }

            self.policies.push(BasePolicy {
                cpus,
                hw_min,
                freqs,
                tier_ceils,
                ceil: start_ceil,
                perf,
                max_writer,
                min_writer,
            });
        }

        // TODO: 真机验证——档位启用后触摸是唯一的放开通道，非触摸期频率硬顶在标准档（8550 是 #13）；
        // 触摸链路不可达（evdev 无权限/无设备）时无降级通道，确认要不要补「长期无触摸事件」抬 tier_touch
        self.tier_enabled = tier_any;
        self.tier_idx = 0;
        self.touch_until = None;
        self.active = !self.policies.is_empty();
        self.last_write = Instant::now();
        if self.active {
            info!("{}", t("powerbase-activated"));
        }
    }

    // [tick]
    /// 每个负载 tick 调用一次。`core_utils`：各 CPU 利用率（0..1）。
    /// 功耗读数是**自己取**的：仅放电时有效，否则视为「无读数」——此时不做功耗限制
    /// 返回距下次防篡改重写的剩余时间（非激活返回 None）
    pub fn on_load_update(&mut self, core_utils: &[f32]) -> Option<Duration> {
        if !self.active {
            return None;
        }
        let now = Instant::now();
        // 放电判定**不能看电流正负**（厂商节点方向不一，部分设备充放皆正），与 PowerAVG 同口径读 status；
        // 非放电 = 无读数（充电功率压频无意义）
        let power_w = if super::read_battery_charge_state() == "discharging" {
            crate::monitor::telemetry::telemetry().batt_power_w()
        } else {
            None
        };

        // ── 触摸升档窗口过期：回落到标准档（先切上界并按当前 perf 压频，随后 tick 按负载自然收敛）──
        if let Some(until) = self.touch_until {
            if now >= until {
                self.touch_until = None;
                if self.tier_idx != 0 {
                    self.tier_idx = 0;
                    self.apply_tier_ceil(false);
                }
            }
        }

        // ── 功耗闸门（放电才有意义：充电时功耗口径与电池功率不是一回事，不该压性能）──
        // 触摸窗口内允许短暂突破目标功耗（touch_break_ms；与升档共用一个窗口）
        let capped =
            power_w.is_some_and(|p| p >= self.cfg.target_power_w) && self.touch_until.is_none();
        let overload_ok = if capped {
            self.overload_reached(core_utils, now)
        } else {
            false
        };
        if !capped {
            self.gate.overload_since = None;
        }

        // 当前档位索引（档位体系下 on_touch 的阶梯保证它 < TIER_STEPS）
        let tier_idx = (self.tier_idx as usize).min(TIER_STEPS - 1);

        for p in &mut self.policies {
            let util = p
                .cpus
                .iter()
                .filter_map(|c| core_utils.get(*c).copied())
                .fold(0.0_f32, f32::max);

            let mut target = util;
            if !capped {
                target *= self.cfg.up_headroom_below;
            } else if !overload_ok {
                // 功耗已达标且没到过载门槛：只允许保持或下降
                target = target.min(p.perf);
            }

            // 触摸窗口内锁在当前档落点（与 CLG 的触摸地板同义）：负载决策不得把刚抬起的档位压回去；
            // 窗口过期由上面的回落分支解除。只锁本次确实抬过档的簇——未命中档位体系的簇三档上界同为
            // hw_max，锁 1.0 会把它顶到硬件最高频，且窗口过期时 apply_tier_ceil 整条跳过、perf 收不回
            if self.touch_until.is_some() && tier_idx > 0 && p.tier_ceils[tier_idx] != p.tier_ceils[0]
            {
                target = 1.0;
            }

            // 降频恒激进：目标低于当前再砍一刀（不看功耗），但限制单次降幅——一步砍半会因频率骤降把 util 顶回去（反馈滞后），形成振荡
            if target < p.perf {
                let stepped = target * self.cfg.down_scale;
                target = stepped.max(p.perf - MAX_DOWN_STEP);
            }
            target = target.clamp(0.0, 1.0);

            // 死区：目标与当前差不到 PERF_DEADBAND 就不写（抑制抖动、省 sysfs 写入）
            if (target - p.perf).abs() > PERF_DEADBAND {
                let want = p.freq_for(target);
                // 只重写上限：min 已在 init 压到硬件最低并保持，不存在 min>max 的写序问题
                if p.max_writer.write_value_force(want) {
                    p.perf = target;
                }
            }
        }

        // ── 防篡改重写 ──
        let elapsed = self.last_write.elapsed();
        if elapsed < REWRITE_INTERVAL {
            return Some(REWRITE_INTERVAL - elapsed);
        }
        self.last_write = now;
        for p in &mut self.policies {
            let want = p.freq_for(p.perf);
            let _ = p.max_writer.write_value_force(want);
            // min 补写：只在这里兜底重写（接管/负载决策都不碰它），防节点被外部改写后失去下界
            let _ = p.min_writer.write_value_force(p.hw_min);
        }
        Some(REWRITE_INTERVAL)
    }

    // [touch]
    /// 触摸事件（事件驱动，与 CLG 共用 touch_detect → sync_channel 链路）：
    /// ① 刷新触摸窗口（连续触摸不掉窗，窗口内允许突破 `target_power_w`）；
    /// ② 档位沿阶梯抬一级——标准 → 触摸一级 → 触摸二级，**已在最高档忽略本次调频**（只续窗）。
    /// 升档立即把各 policy 锁到新档落点（瞬时升频，不等 tick）；窗口过期由 tick 回落到标准档。
    /// 返回升到的桶号（None = 未升档，仅日志用）
    pub fn on_touch(&mut self) -> Option<usize> {
        if !self.active {
            return None;
        }
        self.touch_until = Some(Instant::now() + Duration::from_millis(self.cfg.touch_break_ms));
        if !self.tier_enabled {
            // 未启用档位体系：触摸只续功耗豁免窗口，不动频率
            return None;
        }
        let Some(next) = next_tier(self.tier_idx) else {
            // 已在最高档：忽略本次调频（窗口已续期）
            return None;
        };
        self.tier_idx = next;
        self.apply_tier_ceil(true);
        Some(self.tier_bucket())
    }

    /// 当前档位对应的前沿桶号（仅 tier_enabled 时有意义）
    fn tier_bucket(&self) -> usize {
        match self.tier_idx {
            0 => self.cfg.tier_standard,
            1 => self.cfg.tier_touch,
            _ => self.cfg.tier_max,
        }
    }

    /// 档位切换落地：把各 policy 的上界切到当前档落点并**立即重写 max**（不等 tick 死区——
    /// 死区只挡「目标与当前差太小」，档位切换本身就是一次确定的上界变化）。
    /// `lock = true`（触摸升档）：perf 抬到 1.0，上限即落到新档顶；
    /// `lock = false`（窗口过期回落）：不锁 perf，频率交给后续 tick 按负载收敛。
    /// **上界未变的簇整条跳过**（不碰 perf 也不写频）——抬了 perf 却不写会让状态与硬件分叉，
    /// 而窗口内的 target 锁定把死区补写也挡掉，只能等窗口过期或 5s 重写，等于「触摸后先升再降」。
    /// 写失败不回滚上界：下个 tick / 5s 防篡改重写会按新上界收敛
    fn apply_tier_ceil(&mut self, lock: bool) {
        let idx = (self.tier_idx as usize).min(TIER_STEPS - 1);
        for p in &mut self.policies {
            let new_ceil = p.tier_ceils[idx];
            if new_ceil == p.ceil {
                // 上界没变（未命中档位体系的簇 tier_ceils 恒 hw_max）：**连 perf 都不碰**——
                // 抬了 perf 却不写频会让状态与硬件分叉，而窗口内的锁定把死区补写也挡掉，
                // 只能等窗口过期或 5s 重写收敛，等于「触摸后反而先升再降」
                continue;
            }
            if lock {
                p.perf = 1.0;
            }
            p.ceil = new_ceil;
            // 只重写上限：min 恒硬件最低，升/降档都不必再排写序
            let _ = p
                .max_writer
                .write_value_force(perf_to_freq(p.hw_min, new_ceil, &p.freqs, p.perf));
        }
    }

    /// 过载判定：满占用（利用率 100%）核心占比达到门槛，且持续足够久
    fn overload_reached(&mut self, core_utils: &[f32], now: Instant) -> bool {
        let total = core_utils.len().max(1);
        let full = core_utils.iter().filter(|u| **u >= 0.995).count();
        let pct = full as f32 * 100.0 / total as f32;
        if pct < self.cfg.overload_cores_pct {
            self.gate.overload_since = None;
            return false;
        }
        let since = *self.gate.overload_since.get_or_insert(now);
        now.duration_since(since).as_millis() as u64 >= self.cfg.overload_hold_ms
    }

    // [release]
    /// 释放接管：恢复各 policy 的 governor / min / max
    pub fn release(&mut self) {
        if self.active {
            info!("{}", t("powerbase-deactivated"));
        }
        self.snapshots.retain(|r| !Self::restore_policy(r));
        self.policies.clear();
        self.gate.overload_since = None;
        self.tier_enabled = false;
        self.tier_idx = 0;
        self.touch_until = None;
        self.active = false;
    }

    fn restore_policy(r: &PolicySnapshot) -> bool {
        let gov_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
            r.policy_id
        );
        let min_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_min_freq",
            r.policy_id
        );
        let max_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_max_freq",
            r.policy_id
        );
        let mut all_ok = true;
        if let Some(max) = r.max_freq {
            if crate::utils::write_to_file(&max_path, max.to_string()).is_err() {
                all_ok = false;
            }
        }
        if let Some(min) = r.min_freq {
            if crate::utils::write_to_file(&min_path, min.to_string()).is_err() {
                all_ok = false;
            }
        }
        if let Some(gov) = &r.governor {
            if crate::utils::write_to_file(&gov_path, gov.as_bytes()).is_err() {
                all_ok = false;
            }
        }
        all_ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// little 真机频表片段（升序）：307200 起、含 1017600（#13 落点档）与 1344000（#20 落点档）
    const FREQS: [u32; 8] = [
        307200, 441600, 672000, 1017600, 1344000, 1555200, 1785600, 2016000,
    ];

    #[test]
    fn perf_endpoints_hit_min_and_ceil() {
        let freqs = FREQS;
        assert_eq!(perf_to_freq(307200, 1017600, &freqs, 0.0), 307200);
        assert_eq!(perf_to_freq(307200, 1017600, &freqs, 1.0), 1017600);
        // 上界 = 硬件最高（档位体系未启用时的路径）
        assert_eq!(perf_to_freq(307200, 2016000, &freqs, 1.0), 2016000);
    }

    #[test]
    fn perf_midpoint_floor_aligns() {
        let freqs = FREQS;
        // 0.5 × (307200..1017600) = 662400 → floor 到 441600
        assert_eq!(perf_to_freq(307200, 1017600, &freqs, 0.5), 441600);
        // 上界不在表内（如他簇落点）→ 仍取 ≤ 上界的最大档
        assert_eq!(perf_to_freq(307200, 1500000, &freqs, 1.0), 1344000);
    }

    #[test]
    fn perf_out_of_range_clamps() {
        let freqs = FREQS;
        assert_eq!(perf_to_freq(307200, 1017600, &freqs, 2.0), 1017600);
        assert_eq!(perf_to_freq(307200, 1017600, &freqs, -1.0), 307200);
    }

    #[test]
    fn ceil_below_hw_min_falls_back_to_min() {
        // 配置错（上界低于硬件最低档）：不得给出低于 hw_min 的落点
        let freqs = FREQS;
        assert_eq!(perf_to_freq(672000, 307200, &freqs, 1.0), 672000);
        assert_eq!(perf_to_freq(672000, 307200, &freqs, 0.0), 672000);
    }

    #[test]
    fn tier_ladder_stops_at_top() {
        // 触摸升档阶梯：0→1→2；已在最高档返回 None（「已经在#25 的则忽略本次调频」）
        assert_eq!(next_tier(0), Some(1));
        assert_eq!(next_tier(1), Some(2));
        assert_eq!(next_tier(2), None);
    }
}
