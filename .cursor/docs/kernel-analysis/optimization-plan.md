# 基于内核源码分析的 ChiRi 优化计划

> 编写：2026-09-26。依据：`.cursor/docs/kernel-analysis/` 六份报告（01 DT 对账 / 02 cpufreq·DCVS·LMh / 03 sched·EAS·hooks / 04 thermal / 05 cpuidle·PSCI / 06 vendor 盘点），全部关键引证经三个独立校验代理抽查（约 90 处 file:line，2 处实质性错误已在下文修正后引用）。
> 分析对象：`android_kernel_oneplus_sm8550`（OnePlus 11，SM8550，Linux 5.15.180 GKI + QCOM msm + OPLUS 定制，分支 oneplus/sm8550_b_16.0.0_oneplus_11）。
> **适用范围声明（2026-09-26 追加）：本计划的事实与结论均来自 8550 这一台真机的内核，不同机型/内核版本的节点命名、driver 形态、vendor hook 可能不同，勿直接套用。ChiRi 侧实现已按「不与特定内核强绑定」设计**——所有内核节点依赖（dcvsh_freq_limit / core_ctl / msm_performance / horae_qmi / 温度 zone 名单）均为「存在才用、缺失即降级」，SoC 硬件基线按 `module/config/{soc}/soc.yaml` 分处理器外挂；8475/8998 上这些特性自动走各自的兜底路径（逐核 offline / 通用 cpufreq 语义 / 名单回退）。

---

## 0. 已验证的核心事实（计划的依据）

| # | 事实 | 证据（file:line，相对内核树） |
|---|---|---|
| F1 | 实际 governor 是 **waltgov**（`kernel/sched/walt/cpufreq_walt.c`，`CONFIG_SCHED_WALT=m`），util 语义 = WALT 窗口负载，非 PELT | kalama_GKI.config:344；cpufreq_walt.c:260-267, :890 |
| F2 | **GKI EAS 选核是死代码**：WALT 经 `android_rvh_select_task_rq_fair` 在 `select_task_rq_fair` 入口无条件接管（`*target_cpu = walt_find_energy_efficient_cpu(...)`） | fair.c:7237-7243；walt_cfs.c:1711, :1294-1312 |
| F3 | `uclamp.max=85` 常态下主要走 **限频路径**（`effective_cpu_util` → rq 级 max 聚合 clamp），不是「EAS capacity 视图」；rq 上任一高 clamp 任务会把频率顶回去 | core.c:7307, :7347；fair.c:7120-7124 |
| F4 | 亲和（sched_setaffinity）是硬约束，WALT 不会把任务搬出 `cpus_ptr`；WALT 缓存用户亲和并在 cpuset 变更时恢复（恢复的就是 ChiRi 最后写的值，**有利**）；但 OPLUS prefer-cpu（ux/frame 任务）可在亲和内改写选核 | walt.c:5065-5106, :5212-5214；walt_cfs.c:1265-1279 |
| F5 | **写频 = 投票**：`scaling_cur_freq` 读 EPSS `reg_perf_state`（软件最后写的票），非硬件实际频率；LMh/DCVS 热压不碰 `policy->max`，走 `arch_set_thermal_pressure`；硬件限值读 `dcvsh_freq_limit` / `lmh_freq_limit` | qcom-cpufreq-hw.c:192-210, :385-463 |
| F6 | **压频主通道 = FREQ_QOS 聚合取 min**：thermal cooling（qti_cpufreq_cdev / cpu_voltage_cooling）、msm_performance、input-boost（触摸抬 min）。无内核模块直写 scaling_max_freq | qti_cpufreq_cdev.c:61-75,146；cpu_voltage_cooling.c:273-275,343-344；msm_performance.c:213-214,281,292；input-boost.c:59,73,289-293 |
| F7 | **capacity 动态重算**：WALT `android_rvh_update_cpu_capacity` 按 freq 上限 + 热压重算，OPLUS FAKE_CAP 可乘系数改写——静态 280/855/1024 在 FAKE_CAP 开启时失真；真机 DT 不在本仓库（`qcom/proprietary/devicetree` 全历史零提交），280/855/1024 无 DT 实证 | walt.c:4310-4371, :4339-4344；drivers/base/arch_topology.c:256,284-296 |
| F8 | **idle 选态是 QCOM 私有 `qcom-cpu-lpm` governor**（rating 50 压过 menu 20），prime 深睡 7.48ms 只是下限，还要过 PM QOS / LPM 历史驻留预测 / WALT bias 三道闸；整簇压制有现成原语：core_ctl sysfs（`min_cpus/max_cpus`）+ walt_halt 软下线 | qcom-lpm.c:617-637, :80-88, :834-842；core_ctl.c:444-455, :884-887, :1354 |
| F9 | thermal：tsens 输出 m°C；`soc_max` 是 virtual-sensor 聚合温区（多传感器取 max，保守值，内核树零命中）；CPU 压频由**用户态 thermal-engine**（thermal-genl 事件 + qti userspace cdev）完成，内核 governor 侧 CPU zone 无 cooling-maps | tsens2xxx.c:49-54,79,140,299,922；gki_defconfig:426-436；virtual-sensor.c:42-72,151 |
| F10 | OPLUS 调度套件（frame_boost/eas_opt/OMRG/SUGOV_TL）源码不在本树（`kernel/oplus_cpu` 为占位 symlink），只能确认注入点：OMRG 在 fast_switch 截断（qcom-cpufreq-hw.c:221-223）、frame_boost 旁路 rate limit 并挂 `android_vh_map_util_freq_new`（cpufreq_walt.c:111-175, :1652）。树内**无 uclamp 写入者** | cpufreq_walt.c:21-52, :1511, :1652 |
| F11 | 「全局唯一调度程序」口径需修正为「唯一 userspace sysfs 写频调度程序」——频率实际决策链：waltgov + vendor hook 链 + FREQ_QOS 聚合，ChiRi 写的是 clamp 不是决策 | 综合 02/06 |

## 1. P0 口径与判读修正（文档层，不改代码）

| 项 | 现状问题 | 改法 |
|---|---|---|
| P0-1 | `.cursor/commands/devimp-log-analysis.md` 铁律 3 的「decision=down 但 cur_freq 不动」归因不完备 | 归因排序改为：① cur_freq=票值语义（F5）② thermal pressure 抬底（F5）③ waltgov busy-hold + rate_limit 落地延迟 ④ FREQ_QOS 钳制（F6）⑤ 硬件 DCVS。补充 `dcvsh_freq_limit`/`lmh_freq_limit` 读取指令 |
| P0-2 | TechAnalyze.md / affinity.rs 注释按「85 钳 EAS capacity 视图（85%×1024≈870）」表述（F2/F3 推翻） | 改为「限频路径钳制 + rq max 聚合语义；真机选核走 WALT FEEC，85 不进入能量模型」；保留 affinity.rs:849/2322 的机制描述但补 WALT 语境 |
| P0-3 | 「ChiRi 是设备上全局唯一调度程序」口径（memory/agentsdocs 多处） | 改为「唯一 userspace sysfs 写频调度程序」；防篡改定位同步改写（见 P1-2） |
| P0-4 | devimp 判读口径第 8 条 `@A result=e0/e3/e22` 未含 walt_halt 来源 | 补：绑核目标被 core_ctl pause/walt_halt 时 setaffinity 失败是 e3/e22 的候选来源（F4/F8），归因前先读 core_ctl 状态 |

## 2. P1 代码修正（影响行为，需 A/B 验证）

### P1-1 FAS 防篡改重写与 QoS 聚合的冲突
- **问题**：thermal cooling 的 FREQ_QOS_MAX 低于 ChiRi 写入值时，写 scaling_max_freq 只是在聚合里加更高请求，聚合仍取 min——重写永远「失败」，若按「写后读回不一致→重写」会死循环。
- **改法**：`scheduler/fas/policy_controller.rs` / `policy_mgmt.rs` 的 verify/force 路径：读回 scaling_max_freq 后若值 == 本进程上次写入值（QoS 未钳制）才进入不一致判定；若读回值 < 写入值且 `dcvsh_freq_limit`/thermal cdev 显示有内核钳制，记 `qos_clamped` 状态而非重写。强制重写间隔（30s）语义保留，仅在 QoS 解除后触发。
- **验证**：热压场景（43°C 电池）下 FAS 写频帧数应下降；解除热压后 30s 内恢复目标值。

### P1-2 防篡改兜底盯节点扩展
- 06 报告给出最可能盖写锁频的实体排序：① thermal（Horae + cpufreq_cooling）② msm_performance（用户态直调 QoS）③ input-boost（触摸抬 min）④ 树外 OMRG/frame_boost。
- **改法**：`chiri/scheduler.rs` DOWN 停摆检测 / fast 防篡改处增加只读旁证采集（低频，1s/次、事件驱动优先）：`/sys/module/msm_performance/parameters/cpu_{min,max}_freq`、core_ctl `min_cpus/max_cpus`、`dcvsh_freq_limit`。全部走「值未变不写」的读侧等价物（变化才落 devimp 帧）。
- **验证**：devimp 出现「锁频被压」帧时，能同时看到旁证节点变化，归因不再靠猜。

### P1-3 温度探测名单与语义
- `utils.rs::find_cpu_temp_path` 名单补 `cpuss` 回退项（F9：`soc_max` 是聚合值可能含 GPU/CDSP 热点，作 cpu_temp 偏保守且非纯 CPU 温度）。
- `cpu_temp` 的语义文档化：优先 `soc_max`（保守热保护用）还是 `cpuss*`（纯 CPU 温度）——建议：热保护判定用 soc_max（保守），devimp 记录列加注来源 zone type。
- 同步 `module/config/*/feature.yaml` `cpu_temp_zone_types`（P1 已配置化）注释补真机 zone 实测清单。

### P1-4 scenemode 整簇断电改走 core_ctl
- **问题**：ChiRi scenemode 直接 offline CPU，与 WALT core_ctl（busy 阈值驱动）+ walt_halt（软下线）+ OPLUS pipeline scene 锁 max_cpus（core_ctl.c:847,861,918-919）三套状态机并行，存在恢复后状态残留风险（05 报告 §6-3）。
- **改法**：scenemode 大核压制改写 core_ctl sysfs `max_cpus`（非 pipeline scene 时），退出恢复原值；hotplug 路径保留为 core_ctl 不可用时的兜底。上线前确认 8550 上 core_ctl 管辖簇（big? prime?）。
- **验证**：息屏 scenemode 进入/退出 ×50 次，核对核数与 core_ctl 状态一致、无泄漏。

### P1-5 uclamp.max=85 的效果重估（可选，先验证再改）
- F3 表明 85 的真实效果是限频钳制且受 rq max 聚合稀释。若目标是「压 prime 放置」，需评估 top-app **task_group 层**钳制（`uclamp_tg_restrict`，core.c:1462-1483）——tg max 对任务级值是硬钳，不 rq 稀释；但影响面是整个 top-app 组，需真机 A/B。
- 先做真机验证（P2-3）确认当前 85 在 WALT 路径下的实际效果，再决定是否动。

## 3. P2 真机取证（把「未确认」升级为实证）

| 项 | 命令/动作 | 补齐的报告空洞 |
|---|---|---|
| P2-1 | root 设备 `adb pull /sys/firmware/fdt` + `dtc -I dtb -O dts`，产物放 `mdocs/`（不进内核仓库） | capacity 280/855/1024 出处、OPP 电压、thermal zone 全清单/trip/cooling-map、idle-state 厂商改动、LLCC/interconnect |
| P2-2 | 真机抓 `/sys/class/thermal/thermal_zone*/type`、`cpu*/cpuidle/state*/{latency,residency}`、`cpu*/dcvsh_freq_limit`、`lmh_freq_limit`（若存在）、`/proc/oplus*` `/sys/kernel/oplus*` 节点清单、`/sys/kernel/qcom-cpufreq-hw/print_cpufreq_debug_regs` | 04/05 报告全部「需真机验证」项；OMRG/Horae/msm_performance 活跃性 |
| P2-3 | 高负载场景 ftrace：`trace_sched_task_util`/`trace_dcvsh_freq`（walt_cfs.c:1181,1283、dcvsh trace 事件）验证 WALT 选核决策与 85 的实际作用路径 | P1-5 的决策依据；F2/F3 的行为级确认 |
| P2-4 | soc.yaml 注释补强：capacity 依据改注「真机 cpu_capacity 实测 + /proc/device-tree 待证」，弱化「厂商板级 DTS 覆盖」表述（DT 报告建议） | soc.yaml 8550 注释 |

## 4. P3 观测增强（低优先，配合 P1-2）

- devimp 44 列不动，新增旁证帧（`@X` 类）：`qos_max`(读回 scaling_max 与决策值差)、`hw_limit`(dcvsh_freq_limit)、`msm_perf`(msm_performance min/max)、`corectl`(min/max_cpus)。全部变化才落帧。
- 判读文档同步：新增帧的语义与「QoS 钳制 vs 硬件热压 vs 决策未落地」三态对照表。

## 5. 执行顺序与验收

1. **P0 全部**（纯文档/注释）→ 一次 commit，无行为风险。
2. **P1-1 / P1-2**（防篡改语义修正）→ 真机热压场景 A/B：对比修正前后 FAS 写频帧数与恢复延迟。
3. **P1-3 / P1-4**（温度名单 + scenemode core_ctl）→ 真机验证后合入。
4. **P2 取证**（可与 1-3 并行）→ dtb 反编译结果回填 01 报告「未确认」项，必要时更新 soc.yaml。
5. **P1-5 / P3** 依据 P2 结果决定是否立项。

每步遵循：cargo check 自证（Checking chiri + 0 warning）、值未变不写、事件驱动、A/B 真机数据说话。
