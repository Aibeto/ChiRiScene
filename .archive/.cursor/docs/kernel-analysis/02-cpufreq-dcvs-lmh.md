# SM8550 内核 CPU 频率控制与硬件 DCVS/LMh 路径分析

> 分析对象：`android_kernel_oneplus_sm8550`（OnePlus 11，SM8550/Kalama，Linux 5.15 GKI + QCOM msm）。
> 结论先行，证据为 `file:line`。标注「未确认」者为本仓库内无直接证据、需实测验证。

## 0. 一句话结论

SM8550 上**软件写频只是向 EPSS 硬件投一票**（写 `reg_perf_state` 寄存器），硬件 DCVS/LMh 保留最终裁决权；
`scaling_max_freq` 的生效链是 PM QoS 聚合，thermal cooling（qti_cpufreq_cdev / cpu_voltage_cooling /
cpufreq_cooling）以 **FREQ_QOS_MAX 请求**参与聚合，是「ChiRi 写入后被压低」的直接机制；
且 vendor hook（`android_vh_cpufreq_fast_switch/target`）允许厂商模块在**下发瞬间改写目标频率**。

---

## 1. qcom-cpufreq-hw 驱动机制

`drivers/cpufreq/qcom-cpufreq-hw.c`，SM8550 匹配 `qcom,epss`（epss_soc_data，:508-519）。
DT 确认（`mdocs/sm8550.dtsi:5875-5887`）：`cpufreq@17d91000`，compatible
`qcom,sm8550-cpufreq-epss`，3 个 freq-domain，中断 `dcvsh-irq-0/1/2`。注意 **SM8550 的 DTS 不在本内核树内**
（`arch/arm64/boot/dts/qcom/` 止于 sm8350，Kalama DTS 由 vendor 分支提供），以上来自仓库根 `mdocs/sm8550.dtsi` 转储。

- **probe/init**：`qcom_cpufreq_hw_cpu_init`（:596-703）按 CPU 的 `qcom,freq-domain` phandle 归并 policy
  （`qcom_get_related_cpus`，:342），检查 `reg_enable` 后读 LUT 建频表（`qcom_cpufreq_hw_read_lut`，:229-340）。
  频率不是来自 DT OPP，而是**从硬件 LUT 读出**并动态注册 OPP（:240-246、:289-296）。
- **下发路径**：
  - slow path `qcom_cpufreq_hw_target_index`（:178-188）：直接 `writel_relaxed(index, base + reg_perf_state)`，
    即向 EPSS perf-state 寄存器投硬件票；若 DT 有 OPP 表则同时投票 DDR 带宽（`icc_scaling_enabled`，:238-247）。
  - fast path `qcom_cpufreq_hw_fast_switch`（:202-221）：同样只写 `reg_perf_state`。**两条路径都没有与硬件实际频率
    的反馈比较**。fast_switch 是否启用取决于 DT 是否带 OPP 表（:234-246），SM8550 量产 DT 是否带 → 未确认。
- **get 的语义陷阱**：`qcom_cpufreq_hw_get`（:187-200）读的是 `reg_perf_state`——**软件最后一次写的票**，
  不是硬件实际运行频率。LMh/DCVS 把硬件压低时，`scaling_cur_freq`/`cpuinfo_cur_freq` 照样显示票值。
- **OPLUS 覆盖**：fast_switch 里插了 `omrg_cpufreq_check_limit`（:214-218，CONFIG_OPLUS_OMRG）——
  厂商模块可在 fast switch 落寄存器前再次限制。OMRG 是否在 kalama 配置启用 → 未确认。
- **调试入口**：`qcom-cpufreq-hw-debug.c`（gen4auto =y）挂 `/sys/kernel/qcom-cpufreq-hw/print_cpufreq_debug_regs`
  （:186-190），可直读 EPSS 寄存器（含 current-vote），panic 时自动 dump（:89-121）。**排障首选工具**。

## 2. 硬件 DCVS 与 DCVSh 中断

- 每个 freq-domain 一个 `dcvsh-irq`。`qcom_cpufreq_hw_lmh_init`（:528-569）在 policy init 时注册
  `qcom_lmh_dcvs_handle_irq`（:472-495）：硬件触发限频 → 关中断、转 10ms 周期轮询
  （`qcom_lmh_dcvs_notify`，:385-463）。
- 轮询里做两件事（:415-460）：
  1. 读硬件被压后的频率（`qcom_lmh_get_throttle_freq`，:373-383；EPSS 用 `reg_domain_state` 0x20）；
  2. `arch_set_thermal_pressure()` 给调度器注入**容量热压**——它不改 `policy->max`，而是让调度器
     认为 CPU 容量变小，schedutil/walt 由此自发降频；`dcvsh_freq_limit` sysfs（0444）记录当前硬件限制（:556-564）。
- **退出条件**（:421-431）：硬件恢复（throttled_freq >= 票值）→ 清中断位、回到纯中断模式。
- **关键结论：DCVS/LMh 全程不碰 policy->max，也不写 scaling_max_freq**。软件侧唯一可见变化是
  调度容量热压 + `dcvsh_freq_limit` 文件 + 硬件实际频率下降，而 `scaling_cur_freq` 不变（见 §1 get 语义）。

## 3. LMh 相关三套驱动

| 驱动 | 作用 | SM8550 状态 |
|---|---|---|
| `drivers/thermal/qcom/lmh.c`（QCOM_LMH） | mainline LMh，只支持 cpu0/4（:117-124），仅做 IRQ domain 桥接到 cpufreq-hw 的 dcvsh irq（:30-38） | 本 DT 转储无 lmh 节点；SM8550 dcvsh-irq 直连 cpufreq_hw（dtsi:5886），**大概率未用，未确认** |
| `msm_lmh_dcvs.c`（QTI_THERMAL_LIMITS_DCVS，kalama_GKI=m） | vendor LMh limits：轮询/中断驱动，读硬件限制，注入 thermal pressure（:116-186），暴露 `lmh_freq_limit`（:415-416），compatible `qcom,msm-hw-limits`（:429-432） | 配置 =m 但本转储 DT 无 `msm-hw-limits` 节点 → 是否 probe 未确认 |
| `lmh_cpu_vdd_cdev.c` / `cpu_voltage_cooling.c`（QTI_*，kalama_GKI=m） | LMh/电压 cooling device，**直接发 FREQ_QOS_MAX 请求**（cpu_voltage_cooling.c:273-275、:343-344） | =m，thermal 策略触发时参与钳制 |

**软件可见现象**：LMh/DCVS 触发时 scaling_max_freq 不变（除非 voltage/thermal cdev 走 QoS），cur_freq（票值）
不变，实际频率下降，`/sys/.../cpu*/dcvsh_freq_limit`（或 `lmh_freq_limit`）出现小于 max 的值，调度器容量热压上升。

## 4. scaling_max_freq 完整生效链与「盖写/钳制」路径枚举

写入链（`drivers/cpufreq/cpufreq.c`）：

1. `store_scaling_max_freq`（:753）→ `freq_qos_update_request(policy->max_freq_req, val)`（:748）。
   **用户写 scaling_max_freq 本质是发一条 FREQ_QOS_MAX 请求**，不是直接赋值。
2. QoS 变更通知（:1250-1258 注册 notifier）→ `refresh_frequency_limits`（:1148）→
   `cpufreq_set_policy`（:2564-2586）：`policy->max = freq_qos_read_value(FREQ_QOS_MAX)`（**多条请求取最小值**），
   再 `__resolve_freq` 对齐频表。
3. governor `limits` 回调（walt/schedutil）→ 下一次 `__cpufreq_driver_target`（:2289-2315，`__resolve_freq`
   按 policy->min/max 解析）或 `cpufreq_driver_fast_switch`（:2133-2136，先 `clamp_val(target, policy->min, policy->max)`）。

**会在 ChiRi 写入后压低/改写 policy max 的路径**（按可能性排序）：

| # | 路径 | 证据 |
|---|---|---|
| 1 | **thermal cooling → FREQ_QOS_MAX**：qti_cpufreq_cdev（冷却态直接更新 QoS max，qti_cpufreq_cdev.c:61-75、:146）、cpu_voltage_cooling（:60、:273）、通用 cpufreq_cooling.c（drivers/thermal/cpufreq_cooling.c 存在）。thermal zone/cdev 由 vendor DT+用户态 thermal-engine 驱动 | kalama_GKI.config:287/291 |
| 2 | **vendor hook 改目标频率**：`trace_android_vh_cpufreq_fast_switch`（cpufreq.c:2136）、`android_vh_cpufreq_target`（:2296）、`android_vh_map_util_freq(_new)`（kernel/sched/cpufreq_schedutil.c:163-165；waltgov 同类）——OPLUS frame_boost/wltm 等模块可在下发瞬间改 target | cpufreq.c:2133-2136、2289-2298 |
| 3 | **OMRG fast-switch 限制**（qcom-cpufreq-hw.c:214-218） | 是否启用未确认 |
| 4 | **boost 表切换**（cpufreq.c:2705 `freq_qos_update_request(policy->max_freq_req, policy->max)`）：boost 开关会重算 cpuinfo.max/FREQ_QOS_MAX；LUT turbo 段为 BOOST_FREQ（qcom-cpufreq-hw.c:280-283） | 常规不触发 |
| 5 | resume 后 `cpufreq_update_policy`（:2651-2667）重放 QoS——若 thermal 在 suspend 期间留了 QoS 请求，恢复即钳制 | — |
| 6 | **LMh/DCVS 硬件压频**：不动 policy->max，压的是硬件实际频率（§2），表现为「max 没变但频上不去」 | qcom-cpufreq-hw.c:385-463 |

## 5. governor：schedutil 与 walt（OnePlus 实际用的是 waltgov）

- 本树 schedutil 在 `kernel/sched/cpufreq_schedutil.c`（非 drivers/）。util 来自 `effective_cpu_util`
  （CFS+RT+DL+IRQ 聚合，:185），`get_next_freq = 1.25 × max_freq × util/max`（:141-174）；
  `rate_limit_us` 限频（:522-547）；**busy-hold**：CPU 繁忙时降频提议被忽略、保持当前 next_freq（:367-374）；
  共享 policy 取 max util（:423-445）。
- 但 OnePlus 配了 `CONFIG_SCHED_WALT=m`（kalama_GKI.config:344），实际 governor 是
  `kernel/sched/walt/cpufreq_walt.c` 的 **waltgov**（schedutil 改造）：util 改用 **WALT 窗口负载**
  （`walt_load`），叠加 hispeed_freq/rtg_boost/ed_boost（:534-570）、OPLUS `SUGOV_TL`/`SUGOV_POWER_EFFIENCY`
  特性（:20、:39、:417-432）。ChiRi 日志里 tuned 行的「决策负载」语义应按 WALT 窗口均值理解。
- 与「decision=down 但 cur_freq 不动」直接相关的 governor 行为：busy-hold（:367-374，waltgov 有对应
  实现）与 rate_limit_us——**down 决策只改 next_freq，落地还要等下一次 util 更新且不被 hold**。

## 6. 对 ChiRi 的含义

### 6.1 「decision=down 但 cur_freq 不动」候选解释排序

1. **票值语义**：`cur_freq_khz`（scaling_cur_freq）= EPSS perf-state 寄存器里 ChiRi/调度器最后写的票
   （qcom-cpufreq-hw.c:187-200），不是实际频率。「decision=down 不动」若观测源是 scaling_cur_freq，
   它反映的是**软件票**，锁频窗口内票值本来就不变。
2. **thermal pressure 抬底**：LMh/DCVS 热压让调度容量下降，walt 按「实际容量」算 util，
   down 决策后的目标频率可能仍高于硬件热压后的实际频率——看起来「不动」。
3. **governor busy-hold + rate_limit**：繁忙期降频提议被 hold（schedutil :367-374；waltgov 同构），
   降到目标要等负载真正回落后多次 util 更新。
4. **热 clamp 在 flush 层钉住**：thermal QoS max 低于 ChiRi 写的 max 时，down 决策的目标频率
   被 policy->max 钳住，票值停在 QoS max 上（此情况下 scaling_max_freq 读数也会被拉低，§4-1）。
5. 硬件 DCVS 自主票：EPSS 侧硬件保留高频段的自主裁决，软件 down 票不必然立刻兑现 → 未确认（需
   `print_cpufreq_debug_regs` 对比 current-vote 与 perf-state）。

### 6.2 锁频被压的排障顺序

1. 读 `/sys/kernel/qcom-cpufreq-hw/print_cpufreq_debug_regs`：区分「票被改」还是「硬件压频」。
2. 读 `/sys/devices/system/cpu/cpu*/cpufreq/scaling_max_freq` 与 `cpuinfo_cur_freq`：
   scaling_max_freq 被拉低 = QoS 钳制（thermal cdev / boost / 其他请求者）；max 正常但频上不去 = 热压/DCVS。
3. 读 `dcvsh_freq_limit`（cpu dev）与 `lmh_freq_limit`（若 msm_lmh_dcvs probe）拿到硬件限制值。
4. 对照 devimp 的 `deb_up/deb_down` 与 snap 的 `cpu_cur_khz`：凡 max 正常而实际低 → 走 §6.1-2/5，不要归因「写频失败」。
5. 需要区分 QoS 请求者时：`/sys/kernel/debug/pm_qos`（若有）或逐个临时 disable thermal cdev 验证 → 未确认
   （本树 debugfs 布局未核实）。

### 6.3 FAS 防篡改重写与内核机制的冲突点

- **与 thermal QoS 对打**：FAS 60s 强制重断言 max 时，若 thermal cdev 的 FREQ_QOS_MAX 更低，
  ChiRi 的写只是往聚合里加一条更高请求，**聚合结果仍是 min**——重写永远「写不进」并触发死循环重写。
  正确姿势：把 thermal 钳制视为合法态，重写前先读回 scaling_max_freq 判断（值未变不写规则天然兼容）。
- **与 thermal pressure 对打**：DCVS 热压期 walt 会自发把目标频率压下去；FAS 在此期间每帧比较
  decision 与 cur_freq 会持续看到「不兑现」，应按 §6.1-2 归因而非判定锁频失效。
- **与 vendor hook 对打**：`android_vh_*`/OMRG 可在下发瞬间改频率，任何「写入后立刻回读验证」的
  自证逻辑都可能读到被 hook 改过的值，误判为内核不受理。
- **与 busy-hold 对打**：FAS 写完 scaling_min/max 后期望立刻生效，但 down 方向落地由 governor
  下一次 util 更新决定（rate_limit 窗口内），防篡改重写的观察窗口应 ≥ rate_limit_us + 一个 util 更新周期。
- **suspend/resume**：suspend 期间 governor 停摆、目标不落地；resume 时 QoS 重放（cpufreq.c:2651-2667）。
  FAS 在息屏恢复瞬间的重写可能与 resume 重放窗口重叠，出现一次性「写后读不一致」。

---
*终端调用：3 次（目录清单 + findstr 批量检索 ×2），scratch `devimpbin/tmp_kern_cpufreq.txt` 已删除。*
