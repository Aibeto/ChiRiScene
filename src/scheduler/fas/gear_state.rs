//! gear_state.rs: [decision] [gear]

use std::time::{Duration, Instant};

use log::{debug, info, warn};

use crate::fluent_args;
use crate::i18n::t_with_args;

use super::FasController;
use super::pid::scale_frames;

/// 降档退避计数的**时间衰减**窗口：距上次降档超过该时长即视为「新一轮降档」，计数归 1。
/// 必要性：退避计数不再被升档清零后（见下），若没有时间衰减，`min(4)` 会让一次抖动期的
/// 16 倍冷却（≈24s）永久咬住后续的孤立降档——24s 不许升档对真实场景过重
const DOWNGRADE_BACKOFF_RESET: Duration = Duration::from_secs(60);

/// 升档后最短驻留帧数（快升慢降的「慢降」半边）：升档后未住满该帧数不允许降档。
/// 必要性：logd_1016-165413 实测升档确认 60 帧后升档、随即 90 帧降档确认即可再降，
/// 短帧率低谷就能造成 1~3 秒级 30↔45 往返（daemon.log 15:13 附近 30↔45 连打）。
/// 驻留期给新档位 PID/dampen 充分收敛时间，短谷被驻留吸收；深谷仍由原生档识别
/// 与极端卡顿路径（它们绕过驻留，见 evaluate_gear 降档段）兜底
const DOWNGRADE_MIN_DWELL_FRAMES: u32 = 120;

/// 升档落点 perf 的下限上限：`(new_fps / 144)` 在高帧率档会超出本值、被截断成本值，
/// 即升档瞬间的 perf 落点。调低 = 升档后从更低处起步（稳态由 PID decay 收敛），
/// 突发卡顿仍由 pid_jank 的紧急跳频（大帧直接跳 perf）抬回
const MIN_UPGRADE_PERF_CEIL: f32 = 0.62;

// [decision]
// GearDecision

pub(super) enum GearDecision {
    Hold,
    Upgrade { target: f32, perf: f32, dampen: u32 },
    Downgrade { target: f32, perf: f32, dampen: u32 },
}

impl FasController {
    pub(super) fn detect_native_gear(&self, avg_fps: f32) -> Option<f32> {
        // [silent-return] 以下两处 None 提前返回与一次空匹配都是静默失效：调用方（evaluate_gear
        // 降档段）拿到 None 就退回常规确认流，而「样本不足 / 抖动过大 / 无档位落在 ±8fps」与
        // 「帧率正常不需降档」在 status.csv 上完全同形。本函数每帧调用（高频）→ debug! +
        // log_enabled! 门控，INFO 级别下不产生任何 format! 开销
        let count = self.fps_window.count();
        let stddev = self.fps_window.stddev();
        if count < 20 {
            if log::log_enabled!(log::Level::Debug) {
                debug!(
                    "{}",
                    t_with_args(
                        "fas-native-gear-skip-samples",
                        &fluent_args!(
                            "count" => count.to_string(),
                            "need" => "20".to_string(),
                            "avg" => format!("{:.1}", avg_fps)
                        )
                    )
                );
            }
            return None;
        }
        if avg_fps > 5.0 && stddev < avg_fps * 0.10 {
            let hit = self
                .fps_gears
                .iter()
                .rev()
                .copied()
                .find(|&g| g < self.current_target_fps - 0.5 && (avg_fps - g).abs() < 8.0);
            // [silent-return] 样本与抖动都已达标，却没有档位落在 ±8fps 内：既判不出原生档、
            // 也拿不到常规降档依据，静默丢失。这种「差一点命中」最易在真机排查时被误判成
            // 降档链路失效，故单独落一条
            if hit.is_none() && log::log_enabled!(log::Level::Debug) {
                debug!(
                    "{}",
                    t_with_args(
                        "fas-native-gear-no-match",
                        &fluent_args!(
                            "avg" => format!("{:.1}", avg_fps),
                            "stddev" => format!("{:.1}", stddev),
                            "target" => format!("{:.0}", self.current_target_fps)
                        )
                    )
                );
            }
            hit
        } else {
            if log::log_enabled!(log::Level::Debug) {
                debug!(
                    "{}",
                    t_with_args(
                        "fas-native-gear-skip-jitter",
                        &fluent_args!(
                            "avg" => format!("{:.1}", avg_fps),
                            "stddev" => format!("{:.1}", stddev),
                            "limit" => format!("{:.1}", avg_fps * 0.10)
                        )
                    )
                );
            }
            None
        }
    }

    pub(super) fn do_gear_switch(&mut self, new_fps: f32, perf: f32, dampen: u32) {
        let old = self.current_target_fps;
        self.current_target_fps = new_fps;
        self.refresh_cached_values();
        self.upgrade_confirm_frames = 0;
        self.downgrade_confirm_frames = 0;
        self.ema_actual_ms = 0.0;
        self.pid.reset();
        self.fps_window.clear();
        let final_perf = if new_fps > old {
            let min_upgrade_perf = (new_fps / 144.0).clamp(0.45, MIN_UPGRADE_PERF_CEIL);
            perf.max(min_upgrade_perf)
        } else {
            let max_downgrade_perf = (new_fps / 144.0 + 0.30).clamp(0.45, 0.75);
            perf.min(max_downgrade_perf)
        };
        self.perf_index = final_perf;
        self.gear_dampen_frames = dampen;
        // 快升慢降：升档进入降档驻留期（按档位缩放，墙钟约 2s）；降档/平切不驻留
        self.downgrade_dwell_frames = if new_fps > old {
            scale_frames(DOWNGRADE_MIN_DWELL_FRAMES, new_fps)
        } else {
            0
        };
        self.downgrade_boost_active = false;
        self.downgrade_boost_remaining = 0;
        self.floor_stuck_frames = 0;
        // 齿轮切换是重大状态变更，重置 util 偏移让 PID 从零开始适应新档位
        self.target_fps_offset = 0.0;
        self.util_sample_timer = std::time::Instant::now();
        // 齿轮切换后清除 jank 保护（新档位有自己的 perf 基线）
        self.post_jank_perf_floor = 0.0;
        self.post_jank_guard_frames = 0;
        info!(
            "{}",
            t_with_args(
                "fas-gear-switch",
                &fluent_args!(
                    "old" => format!("{:.0}", old),
                    "new" => format!("{:.0}", new_fps),
                    "perf" => format!("{:.2}", final_perf)
                )
            )
        );
    }

    // [gear]
    // Phase 3: 齿轮决策

    pub(super) fn evaluate_gear(&mut self, avg_fps: f32, recent30: f32) -> GearDecision {
        let tfps = self.current_target_fps;

        // ── 升档判断 ──
        if let Some(next) = self.next_gear() {
            let overshoot = if tfps > 1.0 { avg_fps / tfps } else { 1.0 };

            if overshoot > 1.35
                && self.fps_window.count() >= 15
                && recent30 > tfps * 1.2
                && self.perf_index < 0.45
                && !self.downgrade_boost_active
            {
                let ref_fps = self.fps_window.recent_mean(15).max(avg_fps);
                let best = self
                    .fps_gears
                    .iter()
                    .copied()
                    .filter(|&g| g <= ref_fps + 15.0 && g > tfps + 0.5)
                    .reduce(f32::max)
                    .unwrap_or(next);
                self.stable_gear_frames = 0;
                return GearDecision::Upgrade {
                    target: best,
                    perf: 0.60,
                    dampen: scale_frames(30, best),
                };
            }

            if self.upgrade_cooldown == 0 {
                let confirm = scale_frames(self.cfg.upgrade_confirm_frames, tfps);
                if recent30 >= next - 10.0 && avg_fps >= tfps * 0.9 && self.fps_window.count() >= 60
                {
                    self.upgrade_confirm_frames += 1;
                    self.downgrade_confirm_frames = 0;
                    if self.upgrade_confirm_frames >= confirm {
                        self.stable_gear_frames = 0;
                        return GearDecision::Upgrade {
                            target: next,
                            perf: self.perf_index,
                            dampen: scale_frames(self.cfg.gear_dampen_frames, next),
                        };
                    }
                } else if avg_fps >= tfps - 5.0
                    && self.perf_index < 0.50
                    && self.fps_window.count() >= 90
                    && !self.downgrade_boost_active
                {
                    self.upgrade_confirm_frames += 1;
                    if self.upgrade_confirm_frames >= confirm * 2 {
                        return GearDecision::Upgrade {
                            target: next,
                            perf: (self.perf_index + 0.15).min(0.65),
                            dampen: scale_frames(self.cfg.gear_dampen_frames, next),
                        };
                    }
                // 低 perf 稳帧升档路径：打破「频率被压住 → 帧率上不去 → 升不了档」死锁——perf 低说明档位下频率有余量，游戏稳帧跑满当前目标（stddev 低），
                // 即使 recent30 未达 next-10 也给升档机会；余量门槛 0.40，仍低于爆发路径的 0.45
                } else if avg_fps >= tfps - 2.0
                    && self.perf_index < 0.40
                    && self.fps_window.count() >= 90
                    && self.fps_window.stddev() < tfps * 0.08
                    && !self.downgrade_boost_active
                {
                    self.upgrade_confirm_frames += 2;
                    self.downgrade_confirm_frames = 0;
                    if self.upgrade_confirm_frames >= confirm {
                        self.stable_gear_frames = 0;
                        info!(
                            "{}",
                            t_with_args(
                                "fas-low-perf-upgrade",
                                &fluent_args!(
                                    "perf" => format!("{:.2}", self.perf_index),
                                    "avg" => format!("{:.1}", avg_fps),
                                    "stddev" => format!("{:.1}", self.fps_window.stddev()),
                                    "fps" => format!("{:.0}", next)
                                )
                            )
                        );
                        return GearDecision::Upgrade {
                            target: next,
                            perf: (self.perf_index + 0.20).min(0.65),
                            dampen: scale_frames(self.cfg.gear_dampen_frames, next),
                        };
                    }
                } else {
                    self.upgrade_confirm_frames = self.upgrade_confirm_frames.saturating_sub(3);
                }
            } else {
                self.upgrade_confirm_frames = 0;
            }
        } else {
            self.upgrade_confirm_frames = 0;
        }

        // ── 降档判断 ──
        if let Some(prev) = self.prev_gear() {
            // 快升慢降契约（agentsdocs/03-chiri.md「FAS 快升慢降」）：驻留检查**不得**放在原生档识别
            // 与极端卡顿两条兜底之前。驻留门在下方 `avg_fps < target - 10.0` 分支内、两条兜底与
            // recent30 取消分支之后，只挡「慢降」常规确认流与 boost 抢救；载入后守卫在本 if 首支，
            // 原生档/极端卡顿在驻留门之前——驻留期内这四条路径均可达，深谷不会被驻留门吞掉。
            if self.post_loading_downgrade_guard > 0 {
                self.downgrade_confirm_frames = 0;
                self.cancel_boost();
            } else if avg_fps < self.current_target_fps - 10.0 {
                // 原生档位识别优先于「极端卡顿」判定：游戏自己把帧率锁在低档（窗口均值贴住该档、
                // 标准差小）→ 直接把目标档降下去。判据必须与 `detect_native_gear` 自身要求
                // （`|avg - g| < 8`、stddev < 10%）自洽——原实现只放在 `is_extreme`（`avg < tfps*0.40`）
                // 里面，两者交集为空：tfps=120 时需 `avg < 48` 且 `avg ∈ (52,60)`，g=60 这一支恒不可达
                // （只剩 30 档能命中），「游戏原生 60fps」这个主场景永远识别不到原生档。
                if let Some(native) = self.detect_native_gear(avg_fps) {
                    // [silent-rewrite] 原生档识别改写了降档落点：不经验证计数、直接降到游戏自锁档，
                    // 全程静默；与常规确认降档在 status.csv 上同形，事后无法回溯降档依据
                    warn!(
                        "{}",
                        t_with_args(
                            "fas-native-gear-detected",
                            &fluent_args!(
                                "avg" => format!("{:.1}", avg_fps),
                                "from" => format!("{:.0}", self.current_target_fps),
                                "to" => format!("{:.0}", native),
                                "stddev" => format!("{:.1}", self.fps_window.stddev())
                            )
                        )
                    );
                    return GearDecision::Downgrade {
                        target: native,
                        perf: 0.55,
                        dampen: scale_frames(30, native),
                    };
                }
                let is_extreme = avg_fps < tfps * 0.40
                    && self.fps_window.count() >= 10
                    && self.fps_window.stddev() < avg_fps.max(1.0) * 0.25;

                if is_extreme {
                    self.cancel_boost();
                    self.downgrade_confirm_frames += 1;
                    // [silent-rewrite] 极端卡顿判定改写了降档节奏：绕过常规 boost/确认流直接累加
                    // 计数（快车道），全程静默，事后无法区分「兜底生效」与「降档链路失效」。
                    // 只在卡顿段起始帧（计数 0→1）落一行，避免整段每帧刷屏
                    if self.downgrade_confirm_frames == 1 {
                        warn!(
                            "{}",
                            t_with_args(
                                "fas-extreme-jank-fasttrack",
                                &fluent_args!(
                                    "avg" => format!("{:.1}", avg_fps),
                                    "target" => format!("{:.0}", tfps),
                                    "stddev" => format!("{:.1}", self.fps_window.stddev())
                                )
                            )
                        );
                    }
                // 取消阈值与升档判据同源：进入本分支的前提已是 `avg < tfps - 10.0`（**长窗**低），此时若
                // **短窗** `recent30` 也回到 `tfps - 10.0` 以上，说明只是瞬时长窗低谷 → 清零计数、不降档。
                // 原取 `tfps - 5.0` 时 `recent30 ∈ [tfps-10, tfps-5)`（tfps=120 即 110~115fps）**拿不到
                // 取消**、降档计数继续累加 → 稳定跑 110~115fps 的段被误降档（2026-09-28 实测 45s 内 4 轮
                // 120↔60，`降档加速` 记录的瞬时均值全卡在 109.7~110.0）。
                // 注：两界"同源"是指取消阈值 ≤ 升档阈值（`next - 10.0`）——主升一旦成立取消必然同时成立，
                // 故不会出现「同帧既返回升档又返回降档」；但主升的 avg 下界 `tfps*0.9` 在 tfps>100 时
                // **高于** `tfps - 10.0`（120 档：108 < 110），落在 [108,110) 的帧率仍会满足降档的 avg 条件，
                // 只靠上面的取消分支兜住，属已知脆弱点，勿再放宽取消阈值
                } else if recent30 >= tfps - 10.0 {
                    self.cancel_boost();
                    self.downgrade_confirm_frames = 0;
                } else if self.downgrade_dwell_frames > 0 {
                    // [silent-defer] 驻留期暂缓普通降档：升档后给新档位 PID/dampen 收敛时间，短谷被
                    // 驻留吸收。本分支已由外层 `avg_fps < target - 10.0` 保证「确实压住了降档门槛」
                    // （原生档 A1 / 极端卡顿 A2 / recent30 取消都在本分支之前），故此处直接落一行，
                    // 无需再复制门槛判据（原 `dwell_defer_visible` 只读预判已随之删除）。
                    // 每帧走到这里（高频）→ debug! + log_enabled! 门控，INFO 级下无 format! 开销。
                    // 注：dwell>0 时 `downgrade_boost_active` 必为 false（boost 仅在本函数设置，
                    // 而 dwell 仅在 do_gear_switch 升档路径设置、同一处又把 boost 清零），故不存在
                    // 「驻留冻结进行中 boost」的存活窗口；此处 `cancel_boost() + confirm=0` 与
                    // post_loading 守卫分支同形，cancel 为幂等防御。
                    if log::log_enabled!(log::Level::Debug) {
                        debug!(
                            "{}",
                            t_with_args(
                                "fas-downgrade-deferred-dwell",
                                &fluent_args!(
                                    "dwell" => self.downgrade_dwell_frames.to_string(),
                                    "avg" => format!("{:.1}", avg_fps),
                                    "target" => format!("{:.0}", self.current_target_fps),
                                    "threshold" => format!("{:.1}", self.current_target_fps - 10.0)
                                )
                            )
                        );
                    }
                    self.cancel_boost();
                    self.downgrade_confirm_frames = 0;
                } else if !self.downgrade_boost_active && self.downgrade_confirm_frames == 0 {
                    let boost_inc = self.scaled_boost_inc();
                    self.downgrade_boost_active = true;
                    let scaled_duration =
                        scale_frames(self.cfg.downgrade_boost_duration, self.current_target_fps);
                    self.downgrade_boost_remaining = scaled_duration;
                    self.downgrade_boost_perf_saved = self.perf_index;
                    self.perf_index = (self.perf_index + boost_inc).min(0.90);
                    // [level] 降档加速 = 「降级被暂缓」：帧率跌破门槛后先给一次 perf 抢救机会，
                    // 加速期满自动衰减回落（可自愈、可降级）→ warn；每个 boost 周期只落一次
                    warn!(
                        "{}",
                        t_with_args(
                            "fas-downgrade-boost",
                            &fluent_args!(
                                "avg" => format!("{:.1}", avg_fps),
                                "old" => format!("{:.2}", self.downgrade_boost_perf_saved),
                                "new" => format!("{:.2}", self.perf_index),
                                "inc" => format!("{:.3}", boost_inc)
                            )
                        )
                    );
                } else if self.downgrade_boost_active && self.downgrade_boost_remaining > 0 {
                    self.downgrade_boost_remaining -= 1;
                    if self.downgrade_boost_remaining == 0 {
                        // 渐进衰减而非断崖恢复
                        let blended =
                            self.perf_index * 0.70 + self.downgrade_boost_perf_saved * 0.30;
                        self.perf_index = blended.max(self.downgrade_boost_perf_saved);
                        self.downgrade_boost_active = false;
                        self.downgrade_confirm_frames += 10;
                        // [level] 加速期满 = 抢救失败、转入降档快车道（可自愈的降级续段）→ warn
                        warn!(
                            "{}",
                            t_with_args(
                                "fas-boost-expired",
                                &fluent_args!(
                                    "confirm" => self.downgrade_confirm_frames.to_string()
                                )
                            )
                        );
                    }
                } else {
                    self.downgrade_confirm_frames += 1;
                }

                let confirm = scale_frames(self.cfg.downgrade_confirm_frames, tfps);
                if self.downgrade_confirm_frames >= confirm {
                    let old_fps = tfps;
                    // 退避计数只由降档维护，升档不再清零（原实现在三处升档路径清零 → 每次降档后
                    // 紧跟的那次升档都把 backoff 打回 `1 << 1`，指数退避形同虚设：同一档反复降档
                    // 也只会拿到 2 倍冷却，永远到不了 4/8/16 倍，防抖失效）。
                    // 计数增长的两个条件：**同一档位**且**距上次降档在 DOWNGRADE_BACKOFF_RESET 内**
                    // （时间衰减，防 `min(4)` 的 16 倍冷却长期咬住后续孤立降档）
                    let same_gear = (old_fps - self.last_downgrade_from_fps).abs() < 1.0;
                    let fresh_round = self
                        .last_downgrade_at
                        .map(|t| t.elapsed() >= DOWNGRADE_BACKOFF_RESET)
                        .unwrap_or(true);
                    if same_gear && !fresh_round {
                        self.consecutive_downgrade_count += 1;
                    } else {
                        self.consecutive_downgrade_count = 1;
                    }
                    self.last_downgrade_from_fps = old_fps;
                    self.last_downgrade_at = Some(Instant::now());
                    let backoff = 1u32 << self.consecutive_downgrade_count.min(4);
                    // 计数器按帧递减（frame_pipeline 每帧 -1），各档帧预算不同 → 与同族
                    // confirm/dampen/boost 一致过 scale_frames，冷却墙钟时长才不随档位漂移
                    self.upgrade_cooldown =
                        scale_frames(self.cfg.upgrade_cooldown_after_downgrade * backoff, prev);
                    self.stable_gear_frames = 0;
                    return GearDecision::Downgrade {
                        target: prev,
                        perf: self.perf_index,
                        dampen: scale_frames(self.cfg.gear_dampen_frames, prev),
                    };
                }
            } else {
                self.cancel_boost();
                self.downgrade_confirm_frames = 0;
            }
        }

        GearDecision::Hold
    }
}
