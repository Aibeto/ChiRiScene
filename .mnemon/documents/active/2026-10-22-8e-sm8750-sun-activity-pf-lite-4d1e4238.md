---
id: "4d1e4238-94e7-4ee0-a651-939190c60260"
title: "2026-10-22 8e(SM8750/sun) 调度适配 + activity 级规则 + PF-Lite 落地"
description: "OnePlus 13(PJZ110/SM8750, board=sun) 的 ChiRi 适配：两簇 topology、空 little 区间兜底、activity 级规则首版、PF-Lite（参考能效曲线选点）机制与 CLG 接入。含取证结论、阻塞项与验证边界。"
status: "active"
created_at: "2026-10-21T21:14:07.078Z"
updated_at: "2026-10-21T21:14:07.078Z"
content_hash: "d1d1ace12866a82b482c92029d35aa5ae78dd407c87b4a4f445e084ce1990799"
source_paths:
  - "module/config/sun/soc.yaml"
  - "module/config/sun/feature.yaml"
  - "module/config/sun/meta.yaml"
  - "module/rules.yaml"
  - "src/common.rs"
  - "src/chiri/frontier_lite.rs"
  - "src/chiri/cpu_load_governor.rs"
  - "src/chiri/affinity.rs"
  - "scripts/frontier_policy_lite.py"
  - "scripts/tests/frontier_lite_regression.rs"
session_ids:
  - "session-8e259540-4ed5-4c6e-ad67-daa67abbccb5"
memory_body_ids:
  []
---

# 2026-10-22 8e(SM8750/sun) 适配 + activity 规则 + PF-Lite

## 1. 设备定版

- 机型：OnePlus 13 / **PJZ110** / Qualcomm **SM8750**（骁龙 8 Elite）/ Android 16 / kernel `6.6.89-android15-8-gf4dc45704e54-abogki446052083-4k`（GKI）
- 硬件标识：`ro.board.platform = "sun"`。真机属性里**没有 "sm8750" 字样**（devimp main_ 头 `# soc=- board=sun model=PJZ110`，A02-11 判为不支持机型）。因此探测片段必须用 `sun`，配置目录同名 `module/config/sun/`。
- 原厂内核：`android_kernel_oneplus_sm8750` @ `6028f47fa`，`CPH2649_16.0.9.401 / CPH2653 / PJZ110_16.0.9.401`，基于 QCOM `AU_LINUX_KERNEL.PLATFORM.4.0.R1.00.00.00.061.111`。

### 簇布局（两簇，无小核簇）

| policy | CPU | 频档 | 说明 |
|---|---|---|---|
| policy0 | CPU0-5（6× Oryon perf） | 384000–3532800，16 档 | 共享 `l2_0` / `scmi_dvfs 0` |
| policy6 | CPU6-7（2× Oryon prime） | 1017600–4320000，16 档 | 共享 `l2_1` / `scmi_dvfs 1` |

频表来源 = 真机 devimp 日志 `cpu_cur_khz/cpu_max_khz/cpu_min_khz` 逐档反解（3.2 万行 snap），与原厂 EPSS LUT 一致。

### 内核里有没有 capacity / idle / dpc —— 没有

- 该 checkout **完全没有 vendor DT**：`arch/arm64/boot/dts/vendor/` 不存在，`arch/arm64/boot/dts/qcom/` 里没有任何近期高通平台（kalama/pineapple/volcano 全无），全树 grep `sm8750` **零命中**。`capacity-dmips-mhz` / `dynamic-power-coefficient` / `cpu-idle-states` 只出现在 mainline 老平台文件里。
- 电压不在 DT，在 **EPSS 硬件 LUT**：`drivers/cpufreq/qcom-cpufreq-hw.c:340-342`
  `data = readl_relaxed(base + reg_volt_lut + i*lut_row_size); volt = FIELD_GET(LUT_VOLT, data) * 1000;`
  再经 `qcom_cpufreq_update_opp()` 在 probe 时注入运行时 OPP。驱动里**没有** energy model 注册（grep `em_dev_register` 零命中）。
- 可用的取证途径（尚未执行）：`drivers/cpufreq/qcom-cpufreq-hw-debug.c` 暴露 debugfs 节点 `print_cpufreq_debug_regs`，转储的是原始寄存器（含 volt_lut 行），按 `LUT_VOLT = GENMASK(11,0)`、`reg_volt_lut` 偏移可自行解码出**逐档电压**；容量则读 `/sys/devices/system/cpu/cpu*/cpu_capacity`（ChiRi 本来就优先读它，soc.yaml 只是兜底）。

## 2. 任务1：8e 调度适配（已落地）

- `module/config/sun/{meta,feature,soc}.yaml`（三件套齐全，build.rs 断言要求）。
- topology：`little {0,0}`（**空区间**）、`big {0,6}`、`prime {6,8}`。三槽必须同时给出，否则 `chiri_core_ranges()` 整段回退硬编码（0..3/3..7/7..8，对本机全错）。
- `[capacity]/[idle]/[dpc]` **刻意留空**：三段都是「sysfs 兜底」语义，凭空填推断值会让 PF 需求折算与能耗模型带假基线。
- `src/common.rs`：`CHIRI_SOC_HINTS` 追加 `"sun"`（**末位**）+ 新增 `SOC_HINT_TOKEN_BOUNDARY` 让 `sun` 走**整词匹配**——子串匹配会把 `sunfish`（Pixel 4a / sm7150）误判成本机；末位保证任何具体 SoC 名先命中。
- `src/chiri/affinity.rs` 空区间兜底（真实缺陷）：
  - `little_list()`：`0..0` 下 `end-1` usize 下溢 → `format!` 出非法 cpuset `"6-5"`（写必 EINVAL）；改为空区间退化到 `big_list()`。
  - `little_pair_list()`：空/单核 little 退化到 big 前两颗，`saturating_sub` 防下溢。
  - `pin_background` 调用处的 `format_cpu_list(空)` 返回 `""`，同样退化。
- `feature.yaml` 两簇语义修正：`background_uclamp_max_pct: 0`（无小核簇时压低后台 util 只会让 CLG 少升频、收益不成立）、`normal_fg_exclude_little` 无对象、FDP 保持关闭、`[sched]` 段按 6.6 GKI 口径改写为 TODO。

## 3. 任务2：activity 级规则（首版 8 条）

机制：`module/rules.yaml` 的 `app_modes` 键支持 `pkg/.Activity`，activity 级命中优先于同包名键；未命中/开关关自动回退包名级。

**重要前提**：8e 包的 `activity` 列 **32444 行全为 `-`，且无任何 tick 行** —— 非 ChiRi 机型不接管调度，1s 调度块不跑，activity 采样器从不被请求（采样请求挂在调度 1s 块里）。所以 8e 自身**没有** activity 数据可评估；评估改用 sm8550 包（`logd_1022-031350`，63305 行 / 55926 tick）。

判据：`over_cores` = 组内 util > up_threshold 的核数（性能不足信号）；`under_cores` = 组内 util < down_threshold 的核数（性能充裕信号）；`freq/max` = 决策选频 ÷ 硬件上限。样本 < 30 tick 不入表。

| activity | 证据 | 规则 |
|---|---|---|
| `com.tencent.mobileqq/.activity.QPublicTransFragmentActivity` | big util 0.87、over 2.07 核/tick、freq/max 0.79 | boost |
| `com.tencent.mobileqq/.activity.SplashActivity` | big over 0.72、util 0.70 | boost |
| `com.xingin.xhs/.index.v2.IndexActivityV2` | util 0.87、over 1.3、选频 2.13 GHz | boost |
| `com.taobao.qianniu/com.qianniu.launcher.business.main.MainActivity` | big over 0.48 | boost |
| `com.taobao.taobao/com.taobao.tao.TBMainActivity` | big over 0.45 | boost |
| `com.taobao.taobao/com.taobao.android.detail.alittdetail.TTDetailActivity` | util 0.29、under 2.89/3.84 | reduce |
| `tv.danmaku.bili/.MainActivityV2` | 被包名正则锁进 playback，实为交互信息流 | default |
| `tv.danmaku.bili/com.bilibili.search2.main.BiliMainSearchActivity` | 同上（playback 的 0.35 强平滑拖慢升频） | default |

包名默认规则（`special_tuned.yaml` 的 bili→playback、arknights→akmode）**一条未动**，只做选择性覆盖。副作用：一旦出现含 `/` 的键，`activity_aware` 采样整体启用（3s 节流 dumpsys），这是机制前提也是自耗成本。

## 4. 任务3：PF-Lite

差异（「Lite」的全部含义）：

| | 完整 PF（CLG-PF / FDP） | PF-Lite（CLG-PF-Lite / FDP-Lite） |
|---|---|---|
| 输入 | 逐档实测簇功耗 + 完整能效曲线 | 参考功耗（必）+ 参考电压（可缺） |
| 离线产物 | 46 点前沿 + 需求桶，运行期查表 | 不落桶，运行期按**边际能效**在线选点 |
| 需求轴 | `Σ(util × cpu_capacity)`，需 capacity | **本簇最忙核 util**，不需要 capacity |

不落桶表的理由：参考曲线不是实测，把参考值固化成「需求桶→频点」会让后续拿到真机 energy_model 时必须重挖；Lite 只用参考曲线做**相对排序**，参考值偏差只影响排序边界、不影响机制。

已落地：

- `src/common.rs`：`FrontierPolicyLite` / `FrontierPerClusterLite` / `FrontierClusterLite`（`voltage_mv` 可缺）+ `soc_frontier_policy_lite()`；与既有 `frontier_policy` 并列、互不替代（完整前沿优先）。
- `src/chiri/frontier_lite.rs`：`CurveLite`（入参不变量：频表非空、功耗等长、电压空或等长、频点严格升序）、`marginal_efficiency_lite`、`knee_index_lite`（绝对门槛）、`knee_index_relative_lite` + `LITE_KNEE_RATIO = 0.25`（相对门槛，尺度无关）、`pick_lite`（需求下界 → 转折档 → 硬上限三步）、`LiteStateLite`、`align_down_lite`。
- CLG 接入（`src/chiri/cpu_load_governor.rs`）：
  - 新字段 `pf_lite: Option<LiteStateLite>`，**仅当 `pf_aligned.is_none()`** 时构造（完整前沿优先）；
  - `frontier_lookup_lite()`：非稳态 / 空观测 / 无曲线 → None，静默回退比例路径；
  - flush 落点：`target = min(ratio_freq, pf 或 lite)`，**只允许下调**（与 PF 同一不变式），PF 有落点时不再走 Lite；
  - 一次性日志 `clg-pf-lite-enabled`、tick 摘要 `clg-pf-lite-tick-log`，前缀 `[CLG-PF-Lite]`（zh/en ftl 均已加）。
- `scripts/frontier_policy_lite.py`：锚点校验（频点严格升序、功耗/电压非减、三列等长，坏数据实测被拒）+ `Cx = P/(f·GHz·V²)` 中位数反解 + 受控块写入 / `--check`。

**阻塞**：拿不到 8750 参考电压/功耗，故 `sun/soc.yaml` 的 `frontier_policy_lite.enabled` 保持 **false**，**未填任何编造值**。取证三条路：① 真机 debugfs `print_cpufreq_debug_regs` 解码 LUT；② `opp_summary`；③ 公开补丁（本次会话 web 检索不可用——API 鉴权失败）。数据到手后走生成器写入，勿手改本块。

## 5. 验证边界

| 项 | 结果 |
|---|---|
| `cargo fmt --all --check` | 通过 |
| `rustc --test scripts/tests/frontier_lite_regression.rs` | 4/4 通过 |
| 真实模块在桩 harness 中编译 + 测试（`.cache/litecheck`） | **6/6 通过**，且抓到真实缺陷：`marginal_efficiency_lite` / `pick_lite` 未校验 `power_w` 长度会越界 panic，已加守卫 |
| YAML 解析 + `app_modes` 模式名校验 | 通过 |
| `cargo check --target aarch64-linux-android` | **做不了**：本机无 NDK（`agentsdocs/01-overview.md:89`） |

## 6. 待办

1. `sun` 的 `[capacity]/[idle]/[dpc]` 待真机取证（debugfs LUT 解码 + `cpu_capacity`）。
2. PF-Lite 参考曲线锚点待补；补齐后 `enabled: true` 才会真正接管。
3. `feature.yaml` 数值起点沿用 8650 口径，全部标 TODO，需真机复核。
4. FDP-Lite 尚未实现（现仅有 CLG-PF-Lite）；按用户要求其字段名应带 `-Lite`。
5. 未提交（提交须另获明确授权）。
