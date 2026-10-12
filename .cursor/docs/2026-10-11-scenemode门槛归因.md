# scenemode 进入/撤销门槛归因（logd_1011-000909）

> **命名称谓**：本文的「doze」= 灭屏立即生效的 0.30 段，「scenemode」= `scene_mode_delay_secs` 到期后的 0.12 段（沿用代码与 `agentsdocs/03-chiri.md:144` 的既有叫法）。两者配置同源（`module/config/normal/scenemode.yaml`），所以本文「scenemode 相对 doze 的净省电」是**两段之间的边际**，不是「开不开 scenemode」的差别。

> **⚠ 本段设计已定变更**：2026-10-11 决定**取消 doze 段**，见方案 **P1**。

- 版本：ChiRi #3 Transition 相变 A02-04 (versionCode 30204)，soc=8550 board=kalama model=PHB110，android16 sdk36，kernel 5.15.180-android13-8-o-01178-gfc2079499f31
- 数据包：devimpbin/1011-000909/（8 批次 daemon.log + status.csv + x_devimp_*/main_*.log）
- 复跑脚本（项目根）：tmp_anb11.py 进入状态快照 / tmp_anb16.py 门槛反事实 / tmp_anb17-19.py CLG P→频率映射与逐簇对照 / tmp_anb20.py 逐段配对不变性 / tmp_anb21.py 解耦反事实 / tmp_anb22.py 撤销后 CLG 状态 / tmp_anb24a-b.py 段内位置与冷却窗 / tmp_anb25.py 汇报总表 / tmp_anb29.py long_off 反事实 / tmp_anb30.py 功耗配对分解 / tmp_anb31b.py 恢复延迟 / tmp_anb32-33.py 天花板反事实 / tmp_anb34.py 权威并集墙钟
- 墙钟分母口径：**daemon「屏幕状态变更」事件并集 = 148699s = 41.31h**（status.csv 时间戳含假值，不可作分母）

---

## 〇、结论摘要（TL;DR）

1. **scenemode 不是"没生效"，而是"进去了也白进"**：25 次进入全部 delay_hit、全部 mode=default，18 次（72%）在 10s 饱和判据下被撤销。撤销不是"真的负载高"，而是 **perf_ceil=0.12 把 little 钉在 307.2MHz（簇最低 OPP，占 scenemode 期间 96%）→ util 反比抬满 → 撞上共用同一 0.75 常量的退出门槛**，即 **自锁**。
2. **冷却不是主要损失**：18×300s 名义 5400s = 整包 3.63%；实际落进 status 覆盖的"被挡住的可重进机会"仅 1448s = 0.97%。真正吃掉省电机会的是 **scenemode 只覆盖息屏墙钟的 30.58%**，以及 **被撤销后累计 22.18h 困在 doze(P=0.20)**。
3. **long_off 用过 11 次（44%），但它绕过的是一道形同虚设的门**：11/11 次在门前 60s 内本身就存在 util<0.75 的更新；long_off 组撤销率反而更高（9/11=82% vs 9/14=64%）。
4. **scenemode 相对 doze 的净省电在噪声内**：discharging 16 段配对 Δbatt 中位 +8mW、95%CI ±71mW，折算 ±0.3Wh 量级；原因是 **big/prime 两簇在 doze 与 scenemode 下的决策频率上限本来就相同（499200/595200 kHz），scenemode 的全部边际只在 little 簇天花板 441600→307200**。

| 指标 | 值 | 口径 |
|---|---|---|
| 整包墙钟 | 148699s = 41.31h | daemon 屏幕事件并集 |
| 息屏墙钟 | 69025s = 19.17h = 46.42% | 同上 |
| scenemode 驻留（并集） | 21105s = 5.86h = **14.19% 整包 / 30.58% 息屏** | 进入→(撤销|亮屏) 事件并集 |
| 冷却窗口（并集） | 5400s = 1.50h = **3.63% 整包 / 7.82% 息屏** | 18×300s |
| 冷却∩息屏 | 4826s = 1.34h | 同上 |
| 冷却内实际被挡机会 | 1448s = 0.40h = **0.97% 整包** | status 1Hz，default+discharging |
| 撤销后仍息屏累计 | 85099s = **23.64h**（冷却内 5233s，冷却外 79866s = 22.18h） | 逐次撤销→亮屏 |
| 息屏 ≥300s 段 | 27 个（占息屏 14.79h），真正进入 13/27 = **48%** | daemon 事件 |

---

## 一、证据

### 1.1 源码锚点（src/chiri/mod.rs，逐行核对）

| 位置 | 内容 |
|---|---|
| :26 | `const SCENEMODE_SAT_UTIL: f32 = 0.75;` |
| :28 | `SCENEMODE_SAT_SECS = Duration::from_secs(10)` |
| :30 | `SCENEMODE_COOLDOWN = Duration::from_secs(300)` |
| :2597/:2599 | `scenemode_cooldown_until: Option<Instant>` / `scenemode_sat_since: Option<Instant>` |
| :4237 | 进入分支条件 `!is_screen_on && scenemode_enabled() && !scene_mode_active && scenemode_cooldown_ok` |
| :4202-4203 | `scenemode_cooldown_ok = scenemode_cooldown_until.map_or(true, \|u\| Instant::now() >= u)` |
| :4243-4244 | `delay_hit = screen_off_at.elapsed().as_secs() >= 60` |
| :4252 | `let delay = config_read.scene_mode_delay_secs.max(60)` |
| :4248-4249 | 跳过条件 `is_special_mode(&current_mode) \|\| fas_mgr.has_any_instance()` |
| :4261-4267 | `standby_max = ranges.little.chain(ranges.big).filter_map(\|c\| last_core_utils.get(c)).fold(0.0, f32::max)` |
| :4270-4272 | `long_off = screen_off_at.elapsed().as_secs() >= delay.saturating_mul(4)`（=1200s） |
| :4273 | `if standby_max >= SCENEMODE_SAT_UTIL && !long_off` → :4274-4281 `scene_hold`（`scene_hold_logged` 去重，每息屏段最多 1 条 `main_event("scene_hold","-","util=NN")`） |
| :4282-4310 | 否则进入：scene_cfg reload/init + `scene_mode_active = true` + `apply_affinity_and_corectl` |
| :4316-4318 | 注释原文：「util 是忙时占比（与频率无关），饱和即真饱和，与 perf_ceil 数值无耦合」 |
| :4329-4336 | 退出：`standby_max >= SCENEMODE_SAT_UTIL` → `sustained = scenemode_sat_since.elapsed() >= 10s`，首次置 `sat_since` |
| :4337-4382 | sustained：`scene_mode_active=false; scene_hold_logged=false; scenemode_cooldown_until=Some(now+300s)`，先 `apply_affinity_and_corectl` 恢复全核、再 `get_clg_cfg(cfg,"reduce")` reload，log `scheduler-scene-mode-saturation`(带 util) + `main_event("scene_exit","-","saturation->reduce+300s")` |
| :3748-3825 | 亮屏：`screen_off_at=None; scene_mode_active=false; scene_hold_logged=false`；**未重置 `scenemode_cooldown_until`**；:3814-3823 走 `reload_config`（CLG 仍持 writer，热切换，非重新 init） |
| :3703-3747 | 息屏 doze：`doze_cfg = get_clg_cfg(cfg,"reduce")`，`perf_floor=0.0; perf_ceil = perf_ceil.min(0.30); smoothing_up=0.10; touch_boost_enabled=false` |
| :4518-4531 | SCHEDULER_IPC_RESTART 兜底路径重置 `scene_mode_active/scene_hold_logged/scenemode_sat_since/is_screen_on/screen_off_at/last_core_utils` |
| cpu_load_governor.rs:482-494 | `target_perf = clamp(util × headroom, perf_floor, perf_ceil)`，注释「与 up_threshold 无关」 |
| cpu_load_governor.rs:600-640 | `let write_perf = (eff_perf - decay).max(perf_floor); let ratio_freq = self.cluster.find_floor_freq(write_perf);` |
| cpu_monitor.rs:272-312 | `core_utils[idx] = (busy_diff / total_diff).clamp(0,1)` —— **与频率无关的忙时占比**（但需求固定时压低天花板会挤高窗口内忙占比） |
| logger.rs:1477-1553 | tick 行去重签名只含 tgt_perf/频率/thermal 等**决策结果字段**，max_util/cur_perf 不参与去重 → 离线可见 tick 行数是真实负载更新次数的**下界** |

**关键结构性缺陷**：进入门槛（:4273）与退出门槛（:4329）**共用同一个 `SCENEMODE_SAT_UTIL = 0.75`**，且进入侧是**单 tick 无去抖快照**；源码 :4259-4273 的注释自己也写了「与饱和退出共用 SCENEMODE_SAT_UTIL——常驻簇仍顶满时进入必然 ~10s 后饱和退出，整夜『进→退→300s 冷却』拉锯」。

### 1.2 配置

- `module/config/normal/scenemode.yaml`：`enabled true`、`up_threshold 1.0`、`down_threshold 0.01`、`smoothing_up 0.01`、`down_rate_limit_ticks 1`、`up_rate_limit_ticks 10`、`headroom_factor 1.0`、`headroom_ramp 0.15`、`perf_floor 0.0`、**`perf_ceil 0.12`**、`perf_init 0.12`、`up_jump_threshold 0.50`、`slow_up_scale 0.02`、`down_fast_threshold 0.30`、`spike_jump_threshold 0.30`、`spike_decay 0.10`、`touch_boost_enabled false`
- `module/config/8550/feature.yaml`：`scene_mode_delay_secs 300`（:20）；reduce{up 0.90/down 0.50/smoothing_up 0.50/perf_floor 0.05/perf_ceil 1.00/steady 0.80}；default{up 0.88/down 0.62/smoothing_up 0.60/perf_ceil 1.00/steady 0.90}；`per_cluster.little.up_threshold 0.95`；`touch_boost_tiers 2`/`ms 600`；Thermal 四级 1.0/0.85/0.60/0.40、batt 41/43/45、hysteresis_c3/mid1.5/hard2、`clamp_heavy true`、`free_above 0.95`、`tuned_thermal_floor 0.55`；`CoreCtl.scenemode_offline false`；`Affinity.background_uclamp_max_pct 25`、`fdp_enabled true`
- `module/config/8550/meta.yaml`：`scenemode_enabled true`
- feature.yaml:102-112 的历史注释：2026-10-01「修法3」把 `scenemode_offline` 置 false，理由是 logd_1001-045147 实测 09-28 下线 little(CPU1-2) 把负载挤到唯一在线引导核 CPU0 → CPU0 util 100% → saturation，26 次进入 20 次撤销（最短 11s）

### 1.3 事件计数（8 批次 daemon.log 全量）

`screen_off=336`、`screen_on=336`、`special_on=82`、`special_off=80`、`special_keep=47`、**scenemode 进入=25**、**饱和撤销=18**。

devimp main `event` 行实测：`clamp_change 46138`、`playback_fps 19382`、`clamp_apply 2272`、`overload_hold 775`、`screen 672`、`mode_change 165`、`thermal_change 36`、**`scene_exit 18`（全部 `reason="saturation->reduce+300s"`）**、**`scene_hold 6`**。
> `scene_hold` 只有 6 条是**日志去重假象**：源码 :4280 每息屏段最多记一次（`scene_hold_logged`），真实"被门槛挡住"次数远高于 6。

### 1.4 CLG P → 决策频率映射（daemon.log `[CLG] P0 初始化 | 核心=[0,1,2] | 频率=307.2-2016 MHz | P=0.35 -> 902 kHz`）

| CLG P | 场景 | little | big | prime | 出现次数 |
|---|---|---|---|---|---|
| 0.35 | 亮屏 default | 902 kHz | 1286 kHz | 1478 kHz | 378 |
| 0.20 | 息屏 doze | 556 kHz | 940 kHz | 998 kHz | 307 |
| **0.12** | **scenemode** | **441 kHz** | **729 kHz** | **864 kHz** | **25（与 25 次进入一一对应）** |

簇 OPP 边界：little 307.2–2016 MHz、big 499.2–2803.2 MHz、prime 595.2–3187.2 MHz。

### 1.5 逐簇实测：`cur_freq_khz`（CLG 写入 scaling_max_freq 的决策值）分布

- **P=0.12（scenemode）**：little 行 44470 → **307200 kHz 占 96%**、441600 占 4%；`cur_perf` p50 ≈ 0.05–0.08
- **P=0.20（doze）**：little 行 88516 → 441600 27% / 556800 27% / 672000 20% / 787200 7%，**最低 OPP 仅 3%**
- P=0.35（亮屏 default）：little 行 332876 → 556800 15% / 902400 12% / 672000 12% / 787200 11% / 1228800 10%

逐簇 P0.12 vs P0.20：

| 簇 | scenemode(P0.12) | doze(P0.20) | 结论 |
|---|---|---|---|
| little | 行 44470，307200(96%)，util p50 0.65 / p90 1.00 / mean 0.66 | 行 88516，最低 OPP 3%，util p50 0.36 / mean 0.47 | **scenemode 唯一边际：441600→307200** |
| big | 行 53487，499200(91%)，util p50 0.26 / mean 0.35 | 行 109599，499200(51%)，util p50 0.11 / mean 0.20 | 上限相同（499200） |
| prime | 行 42220，595200(**100%**)，util p50 0.12 | 行 77833，595200(91%)，util p50 0.00 | 上限相同（595200） |

→ **scenemode 相对 doze 的全部边际只在 little 簇（441.6→307.2MHz）**；big/prime 两簇上限完全一致。

### 1.6 25 次进入明细

| # | 批次 | 进入 | 息屏(s) | 路径 | 退出 | 驻留(s) | util |
|---|---|---|---|---|---|---|---|
| 1 | 1009-144818 | 10-09 05:34:52 | 715 | 单tick | sat | 1029 | 100 |
| 2 | 〃 | 07:21:22 | 7105 | **long_off** | screen_on | 873 | − |
| 3 | 〃 | 07:47:12 | 676 | 单tick | sat | 2402 | 100 |
| 4 | 〃 | 08:39:10 | 3794 | **long_off** | sat | 1066 | 95 |
| 5 | 〃 | 09:09:00 | 5584 | **long_off** | sat | 211 | 100 |
| 6 | 〃 | 09:25:59 | 6603 | **long_off** | sat | 356 | 100 |
| 7 | 〃 | 09:43:40 | 7664 | **long_off** | sat | 6036 | 93 |
| 8 | 1009-193158 | 19:08:21 | 363 | 单tick | screen_on | 200 | − |
| 9 | 1010-000959 | 21:57:51 | 363 | 单tick | screen_on | 24 | − |
| 10 | 〃 | 23:10:40 | 301 | 单tick | sat | 238 | 97 |
| 11 | 〃 | 23:25:29 | 300 | 单tick | sat | 93 | 100 |
| 12 | 〃 | 23:49:13 | 307 | 单tick | sat | 55 | 100 |
| 13 | 1010-050713 | 01:58:17 | 302 | 单tick | sat | 159 | 100 |
| 14 | 〃 | 02:05:55 | 760 | 单tick | sat | 112 | 100 |
| 15 | 〃 | 02:12:47 | 1172 | 单tick | screen_on | 113 | − |
| 16 | 1010-141248 | 05:56:26 | 1342 | **long_off** | sat | 337 | 100 |
| 17 | 〃 | 07:28:50 | 6886 | **long_off** | sat | 4173 | 100 |
| 18 | 〃 | 08:50:36 | 11792 | **long_off** | sat | 1056 | 100 |
| 19 | 〃 | 09:22:19 | 13695 | **long_off** | sat | 1020 | 100 |
| 20 | 〃 | 09:51:51 | 15467 | **long_off** | sat | 289 | 100 |
| 21 | 〃 | 10:09:28 | 16524 | **long_off** | screen_on | 1047 | − |
| 22 | 〃 | 11:45:19 | 401 | 单tick | sat | 102 | 78 |
| 23 | 〃 | 14:03:42 | 466 | 单tick | screen_on | 73 | − |
| 24 | 1010-185652 | 17:01:49 | 467 | 单tick | screen_on | 6 | − |
| 25 | 〃 | 17:22:35 | 359 | 单tick | sat | 35 | 100 |

- 驻留（饱和段 18 次）：min 35 / p25 112 / **p50 337** / p75 1056 / max 6036s；**<300s 占 9/18 = 50%**；<60s 2 次
- 25 次驻留合计 21105s；`sat_util` 分布：100 共 14 次，93/95/97/78 各 1 → **util ≥ 95% 占 16/18**
- 25 次全部 `delay_hit=Y`（息屏 ≥60s）、全部 `mode=default`、`screen=0`

### 1.7 25 次进入的状态快照（Q1 场景/前台包/温度）

- **前台包**：`com.android.launcher` **19 次**；其余各 1 次：`com.android.incallui`、`com.deepseek.chat`、`bin.mt.plus.canary`、`me.weishu.kernelsu`、`com.tencent.mobileqq`、`com.microsoft.office.outlook`
- **charge**：discharging 16 次（#1-9、#16-25），charging 4 次（#10 deepseek.chat 8.183W、#11 mt.plus 4.669W、#13 launcher 4.554W），full 3 次（#12/#14/#15）
- **thermal_cap_pct 全部 = 100（从未降档）**；`batt_temp`：min 26.3 / p25 28.0 / p50 32.7 / p75 33.8 / max 37.1 ℃
- → **无温度/热限触发场景**；scenemode 的进入与被撤都与温度无关

### 1.8 决定性判据 A：自锁（Q1 的核心证据）

**(a) 同段配对，天花板压低 → util 反比抬升**（tmp_anb20：doze 窗 = 进入前 210~30s 且 CLG P=0.20；scenemode 窗 = 进入后 15s~退出前 15s 且 P=0.12；23/25 段有样本）

| 量 | 比值 p50 | 备注 |
|---|---|---|
| little util (sc/doze) | **1.54** | min 0.96 / max 5.56 |
| little 决策频率上限 (doze/sc) | 1.44~3.62（多为 2.94） | |
| **util × 频率上限 (sc/doze)** | **0.75** | min 0.40 / max 3.86 |

典型段：#4 doze 0.40@1017600 → sc 0.97@307200（util×2.42，freq×3.31）；#14 doze 0.18@441600 → sc 1.00@307200（util×5.56）；#22 doze 0.29@556800 → sc 0.85@307200。
→ **需求（util×上限）大体不变，天花板被砍 2.9 倍后 util 反比顶满**。同样一份负载在 doze(P0.20) 下饱和条件仅占 **133s/4724s = 2.8%**（25 段中仅 6 段曾出现 `util>=0.75 持续 10s`），在 P=0.12 下 10s 内必然饱和。

**(b) 进入后 standby_max 轨迹**：进入 +0s p50 0.94、+10s 起 p50 = 1.00 直到 +230s 未回落（该"持续"含离线可见性偏差，但 +0s 即 0.94 说明进入瞬间已接近阈值）。

**(c) 反事实：把 SAT_SECS 从 10s 抬到 30s**（同一 util≥0.75 判据，进入次数按实测 25 次不变，只改退出）

| 退出判据 | 总驻留 | 撤销次数 |
|---|---|---|
| 实测：util≥0.75 持续 10s | 21105s | 18 |
| util≥0.75 持续 **30s** | **21101s（≈0%）** | **4** |
| util≥0.75 持续 60s | ~21101s | 4~6 |
| util≥0.85/0.90/0.95 持续 30s | ~21101s | 4~6 |
| util≥0.95 持续 10s | 15298s（−28%） | 5 |
| util≥0.65 持续 10s | 14202s（−33%） | 8 |

→ **18 次撤销里 14 次是"短时突顶即撤"，10s 判据太短，不是持续真饱和。**

**(d) 单纯改 `SCENEMODE_SAT_UTIL` 不是解**：进入门槛（:4273）与退出门槛（:4329）共用同一常量，抬高阈值会同时让"进入"更难。实测反事实中 `thr 0.85/0.95/1.00` 的总驻留反而 **降低 28%**（15298s）。**必须先把进入门槛与退出判据解耦。**

**(e) 需求折算独立验证**：用 doze 段实测 `util × doze 天花板` 反推负载需求，再除以 scenemode 最低 OPP(307.2/499.2MHz) 预测 util → 13 段可算，中位 **0.79**、最小 0.46、**8/13 ≥ 0.75**、5 次 ≥ 1.0。→ 即便用"需求不变"的最保守外推，多数段在 0.12 天花板下**必然**在 10s 内越过 0.75。

### 1.9 判据 B：进入侧是"碰运气"（单 tick 无去抖）

进入 #1（10-09 05:34:52）门前 25s 内 49 个 tick 中 **37 个 max < 0.75（76%）**，可见 util 在 0.1~1.0 间剧烈抖动（相邻 tick 从 0.06 跳到 1.00）。
按"同一次负载更新"聚合（同 ts 多核取 max，严格 little∪big 口径）后重算：25 次真进入门前 60s 的更新中 `max<0.75` 占 **59%~78%**，**0 段是"从不 <0.75"**。
→ 进入门槛"任一瞬间 < 0.75 即放行"等价于**随机放行**；而**退出**却用"≥0.75 持续 10s"的严格判据 —— 进入/退出判据严重不对称。

### 1.10 long_off 实测（Q3）

- 25 次进入中 **11 次（44%）走 long_off 路径**（息屏 ≥1200s = 4×300s），所在段时长：1342 / 3794 / 5584 / 6603 / 6886 / 7105 / 7664 / 11792 / 13695 / 15467 / 16524s
- 其余 14 次靠"单 tick util < 0.75"放行
- **11/11 次 long_off 进入在门前 60s 内本身就存在 <0.75 的更新（占比 64%~84%）** → long_off **不是必要条件**，它绕过的是一个本就形同虚设的门槛
- long_off 组饱和撤销率 **9/11 = 82%**；非 long_off 组 **9/14 = 64%** → **long_off 反而更容易立刻自锁**（它无条件把必然过载的天花板压上去，且不经"当前 util 是否低"的瞬时筛选）
- 关掉 long_off 的保守反事实：进入次数 25→25（**不减少**）；若这些段不再进入，驻留减少 21105−4641 = 16464s（**但驻留本身几乎不省电，见 1.12**）

### 1.11 冷却态占比（Q2）

| 口径 | 数值 | 占整包 41.31h | 占息屏 19.17h |
|---|---|---|---|
| 名义 18×300s 并集 | 5400s = 1.50h | **3.63%** | 7.82% |
| 冷却 ∩ 息屏 | 4826s = 1.34h | 3.24% | 6.99% |
| status 1Hz 内实际"default+discharging 且息屏"的被挡时间 | **1448s = 0.40h** | **0.97%** | 2.10% |

- 18 次撤销后 300s 内 status 息屏行数：58/142/130/114/142/95/0/0/0/301/300/55/140/109/104/123/162/74 → 合计 2049s 息屏（另有亮屏 256s；三次充电中撤销窗内无 status 行）
- **撤销后立刻回到 doze，不是回到完整 reduce**：18/18 次撤销后 600s 内首个 CLG 事件都是 `P0=0.20`（+0s）。原文：`[2026-10-09 05:52:01] [INFO] [chiri] [Scheduler] scenemode 持续顶满性能上限（little util 100%），退回 reduce 并进入 300s 冷却`，同秒三行 `[CLG] P0/P3/P7 初始化 ... P=0.20`
- **结论**：冷却态**不是**大半时间，只占整包 3.63%（名义）/ 0.97%（实际被挡）。**真正的时间损失在别处**：
  - 撤销后仍息屏累计 **85099s = 23.64h**（冷却内仅 5233s，**冷却外 79866s = 22.18h**）
  - 典型 #16（05:56 进入）段总长 17570s、撤销后仍息屏 **15892s**；#3（07:47）段总长 14498s、撤销后仍息屏 **11422s**
  - → **被撤销后长期困在 doze(P=0.20) 而非 scenemode(P=0.12)**，这才是主要机会损失

机会成本分解（息屏已过 300s 且 default+discharging，共 13349s = 3.71h）：已在 scenemode 8743s(65.5%)、被 300s 冷却挡 1413s(**10.6%**)、被特调/FAS 挡 0s、被 util 门槛挡或未到复评 3193s(23.9%)。

### 1.12 功耗实测（净收益）

status.csv 1Hz，pre = 进入前 130~10s，in = 驻留中段 ±60s，取中位：

- 19 段逐段 Δbatt(mW)：`102, −47, −161, 189, 172, −185, −93, −30, −835, −283, −164, −241, −90, 23, 91, 163, 121, 59, −8`
- 全部 19 段中位：batt 0.437→0.447（**Δ+9 mW**）、cpu_dyn 0.108→0.096（Δ−12 mW）、resid 0.293→0.320（Δ+27 mW）；省电段 11/19、中位 Δ=−30 mW、p25=−164 / p75=+102 mW
- **仅 discharging 16 段**：Δbatt 中位 **+8 mW**、均值 **+4 mW**、**95%CI ±71 mW**、省电段 8/16、min −241 / max +189 mW；Δcpu_dyn 中位 −18 mW / 均值 −33 mW
- 另一种"紧贴边界 120s 窗口"配对（19 段 932s）：pre 0.538W → sc 0.528W，**Δ省仅 10 mW**；同息屏段 600s doze 配对：doze 0.515W → scene 0.470W，Δ省 45 mW
- thrash 对照（status 1Hz，wakeups/migrations 是 2s 增量需 ÷2；样本 9121s vs 84810s）：SCENEMODE batt 0.443W / cpu_dyn 0.099W / resid 0.314W / wake 2599/s / mig 2244/s；SCREEN_ON batt 2.737W / cpu_dyn 0.557W / resid 1.948W / wake 4074/s / mig 3339/s
- **折算**：21105s（5.86h）驻留下，均 Δ=−50 mW → 省 **0.293 Wh**；Δ=−30 mW → 省 0.176 Wh；Δ=+30 mW → 多耗 0.176 Wh。→ **scenemode 相对 doze 的净省电在 ±0.3 Wh 量级（噪声内）**，占包内约 44.2Wh 的 <0.7%

### 1.13 息屏段与未进入归因

- 息屏段 281 个（daemon 事件口径）/ 336 个（另一口径）；时长 p50 43s / p90 290s；<60s 64%、60-300s 28%、**≥300s 27 个（8%，合计 14.80h）**
- **≥300s 段中真正进入 scenemode 的 13/27 = 48%**
- 336 段中 289 段(86%) 进 doze(P=0.20)、47 段(14%) 特调保持跳过 doze、**0 段两者皆无**
- 未进入段中 **6 段被特调/FAS 挡住**（13:03:59 段 1673s、12:50:53 段 55s、15:20:00 段 37s、12:58:18 段 31s、22:42:17 段 20s、15:11:09 段 17s）；其余 8 段合格窗口仅剩 11~298s（过 300s 后剩余窗口），属单 tick 门槛的边界漏判
- 有 scenemode 前史的 13 段 p50 587s / p90 14500s（总 12.9h）；无 scenemode 的 323 段 p50 28s / p90 205s（总 6.4h）→ **scenemode 只发生在长息屏段**

### 1.14 亮屏恢复延迟（回归风险的直接检验）

25 次 scenemode 结束后，`screen_on` → CLG `P0=0.35` 恢复延迟 = `[0,0,1,1,1,1,1,1,1,1,0,1,2,2,0,1,1,1,1,1,1,0,0,0,1]` 秒，**全部 ≤2s** → 亮屏恢复路径（`reload_config` 热切换，:3814-3823）**无延迟风险**。

---

## 二、归因

### Q1 — 25 次进入的场景/包/温度；18 次撤销是"真负载高"还是"自锁"？

**场景/包/温度**：25 次全部发生在**息屏**且 `mode=default`，全部 `delay_hit=Y`（息屏 ≥60s）；前台包 19/25（76%）是 `com.android.launcher`，其余为 incallui / deepseek.chat / mt.plus / kernelsu / mobileqq / outlook 各 1 次；charge 以 discharging 为主（16/25）；**`thermal_cap_pct` 恒为 100（从未降档）、`batt_temp` p50 32.7℃ / max 37.1℃** → **与温度无关**。

**判据（四条独立证据，全部指向自锁，而非"真的负载高"）**：

| 判据 | 证据 | 指向 |
|---|---|---|
| ① 天花板-util 反比 | 逐段配对 `util×频率上限` 比 p50 = **0.75**（需求不变），util 比 **1.54**、频率比 **2.94** | 自锁 |
| ② 同负载跨配置对照 | 同一份负载在 doze(P0.20) 下饱和条件仅占 **133s/4724s = 2.8%**（25 段中仅 6 段出现）；在 sc(P0.12) 下必然 10s 内饱和 | 自锁 |
| ③ 判据时长反事实 | SAT_SECS 10s→30s：驻留 **21105→21101s（零损失）**、撤销 **18→4** | 14/18 次是"短时突顶即撤"，非持续饱和 |
| ④ 需求折算 | 以 doze 段反推需求 / scenemode 最低 OPP 预测 util：中位 **0.79**、**8/13 ≥ 0.75**、5 次 ≥1.0 | 0.12 天花板下必然过 0.75 |

**结论**：`up_threshold=1.0` **不是**自锁原因（CLG 的 `target_perf = clamp(util × headroom, perf_floor, perf_ceil)` 与 up_threshold 无关，见 cpu_load_governor.rs:482-494 —— up_threshold 只管 headroom ramp 与升频速度）。真正的自锁闭环是：

    perf_ceil = 0.12 → little 钉死 307.2MHz（簇最低 OPP，占 96%，仅 2 核 × 307MHz 的算力）
       → 同一份后台需求下，busy 占比被挤到 p50 0.65 / ≥0.75 占 46.1%
       → 退出门槛 standby_max ≥ 0.75（同一常量、与进入门槛共用）
       → 10s 后撤销 → 冷却 300s → 回到 doze(0.20) → 直到亮屏/重启才有下一次机会

源码 :4316-4318 的注释「util 是忙时占比（与频率无关），饱和即真饱和，与 perf_ceil 数值无耦合」**与实测直接矛盾**：`core_utils = busy_diff / total_diff`（cpu_monitor.rs:272-312）在计算上确实无频率项，但**负载需求固定时，降低频率上限会延长每个任务占用 CPU 的墙钟时间**，窗口内 busy 占比因此被挤高 —— "与频率无耦合"只在"需求与执行时间无关"的假设下成立，而该假设不成立。

### Q2 — 冷却态是否占大半时间？

**否。** 冷却态占整包墙钟 **3.63%**（名义 5400s）/ **0.97%**（status 1Hz 实测被挡 1448s），占息屏 6.99~7.82%。
真正的时间损失结构：
1. **scenemode 只覆盖息屏墙钟的 30.58%**（5.86h / 19.17h）；「息屏已过 300s」的潜在可进入墙上界 12.54h，实进 5.86h（47%）。
2. **被撤销后累计 22.18h 在冷却窗口之外仍困在 doze(P=0.20)**，而非 scenemode(P=0.12)。这才是最大的一块机会损失。
3. 撤销后的 300s 冷却只贡献 1448s（0.40h）被挡机会，其中约 60% 可被"缩短冷却/抬高门槛"回收。

### Q3 — long_off 用过几次、有没有生效？

**用过 11 次（11/25 = 44%），但"生效"只是名义上的。**
- 11/11 次在门前 60s 内本身就存在 `max<0.75` 的更新（64%~84% 占比）→ 即使删掉 long_off，这 11 次里绝大多数仍会被单 tick 门槛放行（进入次数 25→25 不变）
- long_off 组撤销率 **82%** vs 非 long_off 组 **64%** → long_off **更容易立刻自锁**（它无条件绕过瞬时筛选，把必然过载的天花板压到一个长息屏段的完整负载上）
- 唯一实际作用：让"本该被门槛挡住的段"也能进入，但因为 0.12 天花板注定 10s 内被撤，**净效果是白做一次 enter→exit→cooldown 抖动**

### Q4 — 改进项与收益/回归风险

见下节。

---

## 三、可动参数改动建议

| # | 文件 / 位置 | 从 → 到 | 理由 |
|---|---|---|---|
| **A1** | `src/chiri/mod.rs:28` | `SCENEMODE_SAT_SECS` **10s → 30s** | 零驻留损失（21101s vs 21105s），撤销 18→4；冷却从 5400s→1200s。**最小风险、最大确定性收益** |
| **A2** | `src/chiri/mod.rs:26` + 新增 `SCENEMODE_ENTER_UTIL` | 把进入门槛与退出门槛**解耦**：进入用独立常量（建议 0.60，或保持 0.75 但**加去抖**：连续 N tick < 阈值 / 取近 3s 的 p50），退出保持 `SCENEMODE_SAT_UTIL=0.75` | 当前两者共用同一常量（:4273 与 :4329），任何单侧调值都同时影响另一侧；实测抬高阈值反使驻留 −28% |
| **A3** | `src/chiri/mod.rs:30` | `SCENEMODE_COOLDOWN` **300s → 120s** | 回收 1448s 被挡机会中的约 60%（≈870s = 0.24h）；冷却本身名义仅占 3.63%，缩短对热行为无影响 |
| **A4** | `src/chiri/mod.rs:4270-4272` | 删除 long_off，或 `delay.saturating_mul(4)` → `saturating_mul(8)`（2400s） | 实测无收益（11/11 本可通过单 tick 门槛）且撤销率更高（82% vs 64%）；删除可减少 enter→exit 抖动 |
| **A5** | `module/config/normal/scenemode.yaml` | `perf_ceil 0.12 → 0.20`（并把 `perf_init 0.12 → 0.20`） | 对齐 doze：big/prime 上限本就相同（499200/595200），little 抬到 441.6MHz 后 scenemode 与 doze 在频率维度**完全等价** → 不再有自锁（见 1.8②：doze 下饱和条件仅 2.8%） |
| **A6** | `module/config/8550/feature.yaml` + 源码 | 更彻底：**废弃 scenemode 阶段**（息屏直接 doze），或让 scenemode 只做 `CoreCtl.scenemode_offline` / Affinity 而不压 CLG 天花板 | 若采纳 A5，scenemode 已无独立省电意义（见 1.12：净省电 ±0.3Wh 噪声内、相对 doze 边际仅 little 441.6→307.2MHz）；保留一个 25 次进入/18 次撤销的状态机只增加抖动与代码风险 |
| **A7（可选）** | `module/config/8550/feature.yaml:20` `scene_mode_delay_secs 300 → 120` | 与 A3 配套；注意 `src/chiri/mod.rs:4252` 有 `max(60)` 下限保护 | 让长息屏段更早进入。**注意：单独做 A7 而不做 A5 会放大自锁**（更多段进入 → 更多撤销 → 更多冷却） |

**不建议做的**：单独调 `SCENEMODE_SAT_UTIL`（0.85/0.95/1.00）——实测总驻留反降 28%，进入与退出共用常量的耦合决定了它不可能单独生效。

---

## 四、折算收益区间与不确定性

### 4.1 收益折算

| 改动 | 收益（机制 + 数字） | 折算 |
|---|---|---|
| A1（SAT_SECS 10→30s） | 撤销 18→4；冷却 5400→1200s；**驻留不变（21105→21101s）** | 回收被挡机会 ~1000s（0.28h）；**减少 14 次 enter→exit 抖动**；因驻留本身不省电，掉电侧净收益 ≈ 0，真实收益是抖动与冷却内可重进机会 |
| A2（解耦 + 进入去抖） | 进入更确定性：仅当负载真低时进入，减少"进→10s 撤"的空转 | 若把 9 段驻留 <300s 的段挡在门外，省去 ~9 次抖动；**不改变总驻留量级**（受息屏负载约束） |
| A3（COOLDOWN 300→120s） | 被挡机会 1448s → 约 580s | 回收 **~870s = 0.24h** |
| A4（删 long_off） | 减少 11 段中"本不该进"的部分（long_off 组 82% 撤销率） | 抖动减少；**驻留 −16.5h 的保守上界**，但因驻留不省电，掉电侧影响在噪声内 |
| **A5（perf_ceil 0.12→0.20）** | 消除自锁：scenemode 与 doze 频率上限等同 → 撤销率应与 doze 的饱和率（2.8%）同量级，驻留从 p50 337s 提升到"整个息屏段" | **核心收益是恢复息屏覆盖**：scenemode 覆盖息屏 30.58% → 理论上限 12.54h/19.17h；但**因驻留本身不省电（Δbatt +8mW, CI ±71mW），恢复覆盖的省电收益为 ±0.3Wh 量级**。A5 的真实价值在于**让状态机稳定、消去抖动**，而非省电 |
| A6（废弃 scenemode） | 去掉 25 次进入/18 次撤销/5400s 冷却的全部复杂度 | 掉电侧 ±0.3Wh；**收益是确定性与可维护性**；需先验证 doze(P0.20) 已足够 |

**总收益区间**：以掉电为唯一口径，**全部改动合计在 ±0.5Wh（占包内 44.2Wh 的 ~1%）以内，即噪声内**。真实可交付的收益是**状态机抖动（18 次撤销）与 22.18h"撤销后困在 doze"的机会结构改善**，而这两者对掉电的影响取决于"scenemode 相对 doze 是否真省电"——**实测答案是：不省**。

### 4.2 回归风险（重点：误进 scenemode 会不会伤前台流畅度）

| 风险 | 评估 | 依据 |
|---|---|---|
| **误进 scenemode 伤前台流畅度** | **机制上不会**：进入条件硬编码 `!is_screen_on`（:4237），只在息屏生效；亮屏恢复延迟实测 **全 ≤2s**（1.14） | 无 fps 直接证据（见下） |
| scenemode 期间后台任务超时（sync / JobScheduler / 后台下载被压到 307.2MHz） | **这是 A5 之前最真实的风险**：little 96% 时间在 307.2MHz、big 91% 在 499.2MHz | 建议 A5 落地后观测后台 job 完成时延 |
| A5 抬高天花板导致息屏掉电变差 | 中低：doze 的 0.20 已是同量级 | 1.12 显示 doze 段 resid_w 与 scenemode 相近 |
| A1 延长 SAT_SECS 导致"真高载也不撤" | 低：30s 判据下仍撤 4 次；且撤销后回 doze，天花板并未失控 | 反事实 1.8(c) |
| A3 缩短冷却导致抖动增加 | **中**：撤得快、回得快，可能形成更频繁的 enter/exit 循环 | 必须与 A1/A2 同时落地（A1 已把撤销降到 4 次） |
| A4 删 long_off 导致长息屏段永不进入 | 低：11/11 本可经单 tick 门槛进入 | 1.10 |
| 亮屏后 `scenemode_cooldown_until` 未重置（:3751-3753） | **观察项**：亮屏不清冷却，但因 `!is_screen_on` 无重进，实际只影响"息屏→亮屏→再息屏"的快速循环 | 源码复核 |

---

## 五、不确定性声明

1. **离线 tick 行数是下界**：`src/logger.rs:1477-1553` 的 tick 去重签名只含决策结果字段，`max_util` 不参与去重 → 同一签名的重复更新不落行。所有"门前 60s 有多少 tick < 0.75"的统计都是**下界**（真实合格更新次数 ≥ 可见行数）；"同 ts 多行聚合"口径下门槛判定只能给区间。**这不会改变结论方向**（下界已有 59%~78% 合格）。
2. **fps 不可用于直接量化前台流畅度**：status.csv 117994 行中 `fps` 非 `-` 仅 **2854 行**，只出现在 `x_1009-193158`(1224) 与 `x_1010-050713`(1630)，时间跨度 10-09 16:48 → 10-10 01:28；这期间 4 次 scenemode（19:08、01:58、02:05、02:12）**周边 fps 行数为 0**。故"误进 scenemode 伤不伤前台流畅度"只能给机制性论证（:4237 `!is_screen_on` + 亮屏恢复 ≤2s），加间接证据（scenemode mig 2244/s vs 亮屏 3339/s）。
3. **功耗配对样本小**：仅 19 段 932s（discharging 16 段），Δbatt 中位 +8mW、均值 +4mW、**95%CI ±71mW**，落在噪声内；"scenemode 不省电"的结论有统计支撑（区间含 0），但**具体省多少 mW 不可靠**。
4. **status.csv 时间戳含假值**（1010-000959 批给 00:00:00→23:59:59 跨全天，实际只有傍晚至夜间），逐批跨度合计 42.68h（含重复）→ **不可作墙钟分母**；本报告统一用 daemon「屏幕状态变更」事件并集 41.31h。
5. **scenemode 窗内的饱和占用率统计（0.1%，17/20347s）是幸存者偏差产物**：段一定在 10s 内被撤销，窗内只剩末尾几行可见，**不能据此说"scenemode 下不饱和"**。真正的证据是 1.8(a)(c)(e)。
6. **反事实模型假设"进入次数按实测 25 次不变"**：改退出判据时进入侧条件不动。这也是为什么"单独改 SAT_UTIL"的反事实会得出"驻留反而下降 28%"——该反事实模型里改值同时收紧了进入侧，恰好**定量证明了耦合的存在**，但不能当作"抬高阈值会恶化"的独立结论。
7. **scene_hold 计数（6 条）严重低估**："被门槛挡住"的真实次数不可从日志恢复（`scene_hold_logged` 每息屏段去重一次，源码 :4280）。
