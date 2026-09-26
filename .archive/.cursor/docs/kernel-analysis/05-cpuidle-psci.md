# SM8550（OnePlus 11）真机内核 CPU 空闲路径分析

> 对象：`android_kernel_oneplus_sm8550`（OnePlusOSS，Linux 5.15 GKI + QCOM msm，分支 oneplus/sm8550_b_16.0.0_oneplus_11）
> 日期：2026-09-26。结论先行，file:line 为仓库内证据。

## 结论速览

1. **真机选态路径不是 menu/teo，而是 QCOM 私有 governor `qcom-cpu-lpm`**（rating 50 压过 menu 的 20），树内同时还有一套更激进的 `qcom-simple-lpm`（默认关闭）。ChiRi 之前按 menu 语义推 idle 行为需要换基准。
2. **设备 DT 不在这个内核仓库里**（`arch/arm64/boot/dts/vendor/qcom` 目录不存在，OnePlus 把设备 DT 放独立仓库）。mainline DT 数字如何被消费已验证，但真机数值本身**未确认**，需从设备 DT 仓库或真机 sysfs 对账。
3. prime 最深层是否进得去 = 三道闸串联：QOS/exit-latency → target_residency vs 预测空闲 → LPM 预测器/WALT bias。「≥7.5ms 才睡实」的方向成立，但实际门槛还要叠加预测器历史驻留压制，7.48ms 是下限不是充分条件。
4. 簇级 sleep（APSS domain idle）由 qcom-cluster-lpm 驱动 genpd 完成，条件是簇内所有 CPU 的 next_wakeup 聚合后满足 domain state 的 `power_off_latency + residency`。
5. 内核侧有两条与 ChiRi scenemode 同类的机制：**WALT core_ctl**（busy 阈值驱动 offline，sysfs 可控）和 **walt_halt**（drain RQ + stop_machine 的"软下线"，比 hotplug 轻快）。OPLUS 在 core_ctl 里加了 pipeline scene 锁 max_cpus 的钩子。

---

## 1. DT 实况对账

**核心事实：本仓库不含真机设备 DT。**

- `arch/arm64/boot/dts/vendor/qcom/` 目录不存在（dir 报"系统找不到指定的路径"）；mainline 的 `arch/arm64/boot/dts/qcom/` 只到 sm8350，无 kalama（SM8550 的平台代号）文件。
- 内核配置侧明确面向 kalama：`arch/arm64/configs/vendor/kalama_GKI.config`、`kalama_consolidate.config`、顶层 `kalama_le_gki.fragment` 都在。
- 推论：设备 DT（含 idle-states 实际数值）在 OnePlus 的独立 devicetree 仓库，本仓库无法直接对账。**真机 DT 数值未确认。**

框架消费链已验证（即 mainline 数字如果与真机 DT 相同，语义如下）：

- `drivers/cpuidle/dt_idle_states.c:46-71`：逐节点读 `entry-latency-us` / `exit-latency-us` / `min-residency-us`，原样填进 `cpuidle_state`。mainline 参考值（本地副本 `mdocs/sm8550.dtsi:335-386`）：silver 550/750/6700µs，gold 600/1300/8136µs，prime 500/1350/7480µs；簇级两档 750/2350/9144µs、2800/4400/10150µs（`mdocs/sm8550.dtsi:363-379`）。
- CPU 级状态经 `CONFIG_ARM_PSCI_CPUIDLE`（`gki_defconfig`）走 `drivers/cpuidle/cpuidle-psci.c`；state[0] 被覆写为 exit_latency=1、target_residency=1 的占位 WFI（`cpuidle-psci.c:368-369`）。
- 簇级 `domain-idle-state` 挂到 genpd，`power_off_latency_ns` / `residency_ns` 由 DT 换算。

## 2. cpuidle governor

- 配置：`gki_defconfig` 开 `CPU_IDLE_GOV_MENU=y`、`CPU_IDLE_GOV_TEO=y`、`ARM_PSCI_CPUIDLE=y`；vendor 侧 `kalama_GKI.config` 加 **`CONFIG_CPU_IDLE_GOV_QCOM_LPM=m`**，`kalama_consolidate.config` 加 `CONFIG_CPU_IDLE_SIMPLE_GOV_QCOM_LPM=m`。选项定义在 `drivers/cpuidle/Kconfig:47-63`。
- 生效优先级：`qcom-lpm.c:834-842` 注册 governor `"qcom-cpu-lpm"`，**rating = 50**；menu 只有 rating 20（`governors/menu.c:565`）。模块加载后即胜出。所以真机运行时选态走 LPM，menu/teo 是摆设。
- `qcom-lpm.c` 的 `lpm_select()`（585-663 行）逻辑：
  1. `latency_req` 来自 PM QOS（`get_cpus_qos`，590 行）；
  2. `duration_ns = tick_nohz_get_sleep_length(&delta_tick)`（603 行）；
  3. 从最深态往下扫，逐层检查：被 disable 跳过 → `latency_req < s->exit_latency` 跳过（617 行）→ **`s->target_residency_ns > duration_ns` 跳过（621-624 行）** → 预测器给出 `predicted` 且 `s->target_residency > predicted` 也跳过（633-637 行）。
  4. 选中后 `start_prediction_timer`（551-556 行）：空闲时长超过 `predicted + PRED_TIMER_ADD` 且逼近下一档 residency 时，用 hrtimer 把 CPU 拉回浅层（`histtimer_fn` 97-104 行作废历史）。
- 预测器（`cpu_predict` 204-262 行、`find_deviation` 167-202 行）：按最近 MAXSAMPLES 次真实驻留（`last_residency_ns - exit_latency`，368-369 行）算均值/方差，历史抖动大时按"哪档被过早退出最多"压档。**min-residency 在这里既是门槛也是学习目标。**
- 前置闸 `lpm_disallowed()`（63-88 行）：system suspend 中 → 全禁；`sleep_disabled || sleep_ns < 0` → 禁；**WALT `sched_lpm_disallowed_time()` 拒绝时强制 last_idx=0 并起 bias timer（81-88 行）**——即 WALT/boost 可以把 CPU 钉在浅 idle。`sleep_disabled` 默认 `true`（39 行），由 sysfs/初始化打开（`qcom-lpm-sysfs.c`）。`check_cpu_isactive` 就是 `cpu_active()`（58-60 行）。
- `qcom-simple-lpm.c`：更激进的简化版（直接把 DT 延迟/驻留除以 `cur_div` 缩水后判断，143-155 行），但 `simple_sleep_disabled = true` 默认关闭（20 行）。

## 3. PSCI 域空闲（簇级 sleep）

- CPU 级：`cpuidle-psci.c` 最后一个状态接入 genpd domain（251-252 行 `psci_enter_domain_idle_state`），进态时走 `cpu_pm` + PSCI CPU_SUSPEND，携带 domain state（58-103 行）。进出各有一个 vendor hook：`trace_android_vh_cpuidle_psci_enter/exit`（76、96 行）。
- 簇级决策在 genpd governor：`drivers/base/power/domain_governor.c`
  - `next_wakeup_allows_state()`（157-164 行）：domain state 可选 ⇔ `next_wakeup - now ≥ power_off_latency_ns + residency_ns`；
  - QCOM 打开了 `GENPD_FLAG_MIN_RESIDENCY` 分支（279-282 行）按聚合 next_wakeup 挑最深可行态。
- next_wakeup 的来源：`qcom-cluster-lpm.c`
  - `update_cluster_next_wakeup()`（365-375 行）取"簇内所有 CPU next_wakeup 的最小值"与"cluster 预测唤醒"的更早者，`dev_pm_genpd_set_next_wakeup()` 写进 genpd；
  - `cluster_predict()`（88-170 行）与 CPU 级同构：按历史驻留均值为簇预测唤醒时间，同时校验 `genpd->states[j].residency_ns`（149-166 行）。
- **簇睡实的条件**：簇内每个 CPU 都进 idle（且没被 core_ctl offline，offline 的 CPU 不聚合），且聚合空闲 ≥ 所选 domain state 的 `power_off_latency + residency`（即 mainline 数字下至少 2.35+0.75ms 或 4.4+2.8ms 起步，加 residency 门槛）。

## 4. 空闲与调度器交互 / vendor 拦截

- 标准入口未动：`kernel/sched/idle.c:232` `cpuidle_select(drv, dev, &stop_tick)` → governor。
- **vendor hook 可改选态结果**：`drivers/cpuidle/cpuidle.c:214` `trace_android_vh_cpu_idle_enter(&index, dev)`（注释明说 hook 可改 index，target_state/broadcast 都在其后赋值），257 行对称的 exit hook。`cpuidle-psci.c:76/96` 另有两个 PSCI 进出 hook。即任何 vendor 模块都能拦截/改写空闲档位。
- **OPLUS 定制 idle 注入**：`kernel/sched/idle.c:351-383` 有自制的 `idle_inject_timer_fn` + 初始化（351、383 行），不是 mainline 的 idle_inject 框架形态——OPLUS 侧可以在空闲中插定时唤醒。
- WALT 侧的浅 idle 强制：`qcom-lpm.c:81-88` 的 `sched_lpm_disallowed_time()` bias 路径（boost/输入响应期间禁深睡）。其定义在 WALT 子系统内（具体文件未确认）。
- `drivers/oplus_inject/` 目录树内只有一个 Kconfig，无源码——注入类逻辑以 vendor 模块形式在别处发布。

## 5. hotplug / cpuhp 上的 OPLUS 定制

没有独立的 `arch_cpuhp` 定制，定制备在 WALT 里：

- **core_ctl**（`kernel/sched/walt/core_ctl.c`）：按簇 sysfs 暴露 `min_cpus` / `max_cpus` / `busy_up_thres` / `busy_down_thres` / `task_thres` / `offline_delay_ms` / `enable`（455-467 行）。`eval_need()`（907-964 行）用 WALT `sched_get_nr_running_avg()`（764 行）+ `busy_pct` 阈值 + `sched_cpu_high_irqload` 判每核 busy，`apply_limits` 夹在 min/max 之间。默认 `min_cpus=1, max_cpus=num_cpus`（1505-1506 行）。
- **OPLUS 钩子**：boost 期间若处于 pipeline scene，`apply_limits`/`apply_limits_32bit` 直接放行 max_cpus（847、861、918-919 行 `oplus_is_pipeline_scene()`）；并把 `core_ctl_set_boost` 以 `oplus_core_ctl_set_boost` 导出（1563 行）供 OPLUS 框架调用。
- **walt_halt**（`kernel/sched/walt/walt_halt.c`）：第三条路——"halt"：`stop_machine` + drain runqueue（191-224 行）把 CPU 移出调度（`__cpu_halt_mask`，26 行），**不真 offline**，恢复比 hotplug 快；调度器选核掩码全部 `&~cpu_halt_mask`（545-584 行）。`core_ctl.c:884-887` 的 `is_active = cpu_active && !cpu_halted` 表明 core_ctl 与 halt 配合使用。623 行还有"对称系统首核禁 hotplug"约束。
- 与 idle 的关系：被 offline/halt 的 CPU 不参与簇 next_wakeup 聚合，簇更容易整体入睡；core_ctl 的 busy 判定本身又依赖 CPU 醒着采样——整簇长睡时 core_ctl 不会误回填。

## 6. 对 ChiRi 的含义

1. **mainline idle 数字在真机是否成立：未确认，但消费语义已锁定。** 设备 DT 不在本仓库；`dt_idle_states.c` 的解析表明数字是直通的。建议对账源：真机 `/sys/devices/system/cpu/cpuN/cpuidle/state*/{latency,residency,time}`，或 OnePlusOSS 的 devicetree 仓库。注意 QCOM 常在 vendor DT 里缩延迟/驻留（`qcom-simple-lpm` 的存在就是为这种缩参场景服务的另一证据）。
2. **「prime 睡实要 ≥7.5ms 空闲、唤醒 1.35ms」要不要修正：方向对，语义要加细。** 7.48ms 是 `lpm_select` 进最深 CPU 态的**下限**（`target_residency_ns > duration_ns` 即弃，qcom-lpm.c:621-624），不是充分条件：还要过 PM QOS（QOS < 1.35ms 时被 617 行挡）、LPM 预测器历史驻留压制（633-637 行）、WALT bias（81-88 行）三道闸。1.35ms 唤醒延迟只在真进了该态或 QOS 紧时才支付。ChiRi 对 prime 空转压制的时间尺度按 ~8ms 量级规划成立，但**深睡率可能明显低于 menu 语义下的推算**（预测器 + bias 会显著压档）；验证时看 `trace_lpm_gov_select` 的 reason 位而不是猜。
3. **scenemode 整簇断电与内核机制的关系：有现成原语，别硬写 hotplug。** 内核已有 core_ctl sysfs（`min_cpus/max_cpus`，0-4 可控）和 walt_halt（快速恢复的软下线）两条正路；OPLUS 在 pipeline scene 时还会锁 `max_cpus` 并暴露 `oplus_core_ctl_set_boost`。ChiRi scenemode 若想整簇压制大核，走 core_ctl sysfs 或在非 pipeline scene 下设 `max_cpus` 更稳——直接 `cpu_down` 可能与 OPLUS boost/halt 状态机互相踩（halt 掩码与 hotplug 掩码是两套，混用会有唤醒后状态残留风险）。且整簇断电后簇级 domain idle 才最容易达成——这与 ChiRi 想要的省电方向同向。
4. **idle 相关假设要按 `qcom-cpu-lpm` 重算**：ChiRi 若有基于「menu governor 用 next_timer_ns 做 predicted」的推断，应改按 LPM 语义（预测器学习真实驻留 + prediction timer 中途升级/降级）。另外 `android_vh_cpu_idle_enter` 允许 vendor 模块改最终档位——ChiRi 从 sysfs 观测到的 idle 行为可能被 OPLUS 组件二次改写，归因时留这个变量。

## 附：本轮工具口径

本仓库根 `.gitignore` 未忽略内核目录（"参考仓库改为 git submodule 记录"注释），但本环境下 Glob 与目录级 Grep 对该目录均失效（Grep 指到具体文件才可靠），目录清单靠 `dir /b` 补齐。`sched_lpm_disallowed_time` 的定义文件、真机设备 DT 数值、`android_vh_cpu_idle_*` 的树内消费者（OPLUS 注册大概率在 vendor 模块）——三者未确认。
