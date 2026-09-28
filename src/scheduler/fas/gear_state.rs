//! gear_state.rs: [decision] [gear]

use std::time::{Duration, Instant};

use log::info;

use crate::fluent_args;
use crate::i18n::t_with_args;

use super::FasController;
use super::pid::scale_frames;

/// 降档退避计数的**时间衰减**窗口：距上次降档超过该时长即视为「新一轮降档」，计数归 1。
/// 必要性：退避计数不再被升档清零后（见下），若没有时间衰减，`min(4)` 会让一次抖动期的
/// 16 倍冷却（≈24s）永久咬住后续的孤立降档——24s 不许升档对真实场景过重
const DOWNGRADE_BACKOFF_RESET: Duration = Duration::from_secs(60);

// [decision]
// GearDecision

pub(super) enum GearDecision {
    Hold,
    Upgrade { target: f32, perf: f32, dampen: u32 },
    Downgrade { target: f32, perf: f32, dampen: u32 },
}

impl FasController {
    pub(super) fn detect_native_gear(&self, avg_fps: f32) -> Option<f32> {
        if self.fps_window.count() < 20 {
            return None;
        }
        if avg_fps > 5.0 && self.fps_window.stddev() < avg_fps * 0.10 {
            self.fps_gears
                .iter()
                .rev()
                .copied()
                .find(|&g| g < self.current_target_fps - 0.5 && (avg_fps - g).abs() < 8.0)
        } else {
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
            let min_upgrade_perf = (new_fps / 144.0).clamp(0.45, 0.70);
            perf.max(min_upgrade_perf)
        } else {
            let max_downgrade_perf = (new_fps / 144.0 + 0.30).clamp(0.45, 0.75);
            perf.min(max_downgrade_perf)
        };
        self.perf_index = final_perf;
        self.gear_dampen_frames = dampen;
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
                } else if !self.downgrade_boost_active && self.downgrade_confirm_frames == 0 {
                    let boost_inc = self.scaled_boost_inc();
                    self.downgrade_boost_active = true;
                    let scaled_duration =
                        scale_frames(self.cfg.downgrade_boost_duration, self.current_target_fps);
                    self.downgrade_boost_remaining = scaled_duration;
                    self.downgrade_boost_perf_saved = self.perf_index;
                    self.perf_index = (self.perf_index + boost_inc).min(0.90);
                    info!(
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
                        info!(
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
