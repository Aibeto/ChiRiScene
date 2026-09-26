## [kernel] SM8550 内核机制分析（android_kernel_oneplus_sm8550）

> 2026-09-26 内核分析会话沉淀（8 份报告 + 真机数据包，原件已归档）。基准：OnePlus 11（PHB110）/ 小米 13 Pro（2210132C），内核 5.15.180/194，检出 `oneplus sm8550_b_16.0.0`。真机取证未竟项统一记 `07-todos.md`「内核取证」节。

### 平台基准（8550）

- 拓扑：little A510 cpu0-2→policy0、big A715 cpu3-6→policy3、prime X3 cpu7→policy7（**勿用 8475 的 0-3/4-6/policy4 口径**）；全部 CPU 共享 L3（跨簇迁移丢 L2 不丢 L3）。A510 r1 支持 AArch32（A715/X3 才移除）。
- OPP 频点（与真机 `scaling_available_frequencies` 一档不差，可依赖；电压/功耗参数不可）：little 16 档 307200–2016000；big 20 档 499200–2803200；prime 21 档 595200–3187200（逐档 kHz 表见归档件 `.archive/.codebuddy/docs/2026-09-26-sm8550-opp-ladder.md`，定位是校准基准非运行时输入）。
- capacity：真机 280/855/1024 ≠ mainline DT 326/693/1024 → 厂商板级 DTS 覆盖；**算容量一律读真机 `cpu_capacity`**，mainline 值仅参照。
- floor 对齐取档：内核对 scaling_max_freq 写入**向下 clamp 到最近档**（写 little 1900000 落 1785600）；2026-09-26 起 CLG/tuned/PowerBase 取档 floor 对齐（≤目标的最大档）后再写；FAS 锁频（min=max）与 fast.rs 锁 hw_max 取表内真实档无需对齐。
- idle 深度（µs，residency/exit）：silver 750/6700、gold 1300/8136、goldplus 1350/7480、簇级 2350/9144、4400/10150；prime 睡实需 ≥7.5ms 空闲、唤醒 ~1.35ms → FAS migration_cost_ns=400000（400µs）远低于唤醒 prime 代价，与「prime 空转就压」同向。
- CPU 注册为 thermal cooling device：内核温控限频是独立于 ChiRi thermal_cap 的第二层——「写了 max 却上不去」先查 OPP 对齐、再查 thermal cooling。
- cpu-map 平铺，簇划分靠 capacity + freq-domain + idle 域表达，**勿拿 cpu-map 当簇划分依据**。

### DT 对账

- 本内核检出**无任何厂商 DT 源**：`arch/arm64/boot/dts/vendor` 是 symlink→`qcom/proprietary/devicetree`（仓库外、非 gitlink），`dts/qcom` 止于 sm8350；「280/855/1024 出自厂商 dts」在本检出永远无法验证。需厂商 DT 时另拉 `qcom/proprietary/devicetree` repo。
- mainline `mdocs/sm8550.dtsi` 可依赖块：freq-domain 划分（CPU0-2→domain0 / 3-6→1 / 7→2）、CPU OPP 档位、idle-states；**capacity / dpc / thermal trip 不能当真机基准**。`soc_max` zone 在 mainline 不存在（厂商/Oplus 侧定制）。
- 取证唯一路径：真机 `adb pull /sys/firmware/fdt` + `dtc -I dtb -O dts`（产物放 mdocs/）——见 07 T1。

### 调频路径与竞争者

- qcom cpufreq-hw **写频=投票**：slow/fast path 均只写 `reg_perf_state`；get 读的是软件票非硬件实际频率；频表从硬件 LUT 读出动态注册 OPP（非 DT OPP）。
- `scaling_max_freq` 生效链：store→FREQ_QOS update→refresh_frequency_limits→cpufreq_set_policy，**多条 FREQ_QOS_MAX 取 min**——厂商模块全部走 freq_qos（树内无任何直写 scaling_max_freq/governor 的厂商模块），QoS 优先级高于 policy min/max。
- 压频六条（按可能性）：thermal cooling QoS、vendor hook（`android_vh_cpufreq_fast_switch/target`、`vh_map_util_freq(_new)`）、OMRG（CONFIG_OPLUS_OMRG 覆盖 fast_switch，是否启用未确认）、boost 表切换、resume QoS 重放、硬件 DCVS（不碰 policy->max）。
- 实际 governor=**waltgov**（CONFIG_SCHED_WALT=m）；schedutil busy-hold + rate_limit 使 down 落地要等 util 更新。
- LMh/DCVS：硬件限值读 `reg_domain_state`；`arch_set_thermal_pressure` 注入容量热压（**不改 policy->max**）；`dcvsh_freq_limit` sysfs 可读；三套 LMh 驱动并存（lmh.c 仅 IRQ 桥、msm_lmh_dcvs、cpu_voltage_cooling 直发 FREQ_QOS_MAX）。
- 盖写锁频排序（防篡改排查序）：thermal（Horae + cpufreq_cooling）> msm_performance > input-boost（FREQ_QOS_MIN 抬升）> 树外 OMRG/frame_boost。防篡改盯节点：scaling_{max,min}_freq、cpu.uclamp.{min,max}、msm_performance cpu_{min,max}_freq、core_ctl enable/min_cpus、`/proc/horae_qmi`；比对用「记账+读回」双校验。
- 真机 0926 定案：msmp 节点不可读（无锁频盖写通道）、horae_qmi 存在（活跃性未知）、dcvsh_freq_limit 常规负载恒满频（硬件 DCVS 归因排除，仅剩真热压时段待采样）。

### WALT / EAS / 调度

- 真机选核 = **WALT 劫持**：`select_task_rq_fair` 入口 `android_rvh_select_task_rq_fair`→`walt_find_energy_efficient_cpu` 无条件接管；**WALT 启用时 GKI EAS 是死代码**。
- WALT FEEC 要点：many-wakeup 回 prev_cpu、pipeline_cpu fastpath、能量差 ≥prev/32（≈3.1%，注释写 6% 与代码不符）才留 prev；OPLUS 后处理 `set_frame_group_task_to_perfer_cpu` / `set_ux_task_to_prefer_cpu` 可改写结果。
- capacity 运行时重算：`min(fmax_capacity×cluster.max_freq/max_possible, fmax−thermal_pressure)`，OPLUS FAKE_CAP 再乘 `fake_cap_multiple` → 静态 capacity 会失真。
- uclamp：rq 级 **max 聚合**（任一高 clamp 任务即顶回）；树内 0 个 uclamp 写入者；tg 钳制 `uclamp_tg_restrict` 是 85 钳可作用的 tg 层（评估未做，见 07 T7）。
- 有调用点无注册者的 hook（树外 oplus_cpu，`kernel/oplus_cpu` 为占位）：rvh_find_energy_efficient_cpu、rvh_uclamp_eff_get、rvh_effective_cpu_util、vh_em_cpu_energy、vh_map_util_freq(_new)、vh_setscheduler_uclamp；SCHED_TUNE / SCHED_ASSIST / FRAME_BOOST / PIPELINE / FAKE_CAP 源码均不在树。
- WALT 缓存/恢复 sched_setaffinity（**对 ChiRi 有利**：恢复的就是 ChiRi 最后写的值）；walt_halt 拒绑、walt_lb 迁移、core_ctl 拓扑改变是亲和侧竞争者。
- FAS 冲突五点：QoS 聚合 min 下重写必失败、thermal pressure 期「不兑现」、vendor hook 改写自证读数、busy-hold 观察窗口应 ≥ rate_limit_us、resume 重放窗口 → `qos_clamped` 状态机（policy_controller.rs）应对。

### Thermal（内核侧）

- tsens 单位 = **m°C**（12bit last_temp ×100，thermal_sysfs 直出）。
- `soc_max` 最可能是 `virtual-sensor.c` 聚合温区（VIRT_MAXIMUM 多传感器取 max）——**保守聚合值非纯 CPU 温度**（聚合成员未确认，需真机 DTB）；mainline zone：aoss/cpuss（125°C passive）、cpu 90/95/110、gpuss 绑 GPU。
- CPU 压频不由内核 governor 完成（defconfig 仅 user_space + power_allocator，CPU zone trip 全 passive 无 cooling-maps）→ **userspace thermal-engine** 决策；GPU 例外走内核通道。cooling 全家桶（qti_cpufreq_cdev/thermal_pause/cpu_hotplug/cpu_voltage_cooling/cx_ipeak/ddr/devfreq/userspace_cdev）启用与否取决于厂商 defconfig。
- 四层热链：LMh 硬件限频（最硬）→ userspace thermal-engine（90/95°C trip）→ 内核 governor（CPU 侧不动作）→ ChiRi 软件热保护（最外层）；电池温度走 PMIC（adc-tm5/bcl_soc）独立链路。
- 0926 实测：热压触发量是 **CPU 温度非电池**（batt 30.8℃、cpu 90.7℃ 时 cap=85）——「41℃ 电池触发」旧口径作废。

### cpuidle / core_ctl

- 真机选态 governor = **qcom-cpu-lpm**（rating 50 压过 menu）；`lpm_select` 三道闸：PM QOS latency_req、`target_residency > duration` 即弃、预测器压制 + WALT `sched_lpm_disallowed_time` 强制 last_idx=0 + bias timer——**prime 深睡是下限非充分条件**，深睡率明显低于 menu 语义推算（验证看 `trace_lpm_gov_select` reason 位）。
- 簇级 sleep：domain_governor `next_wakeup_allows_state`；qcom-cluster-lpm 取簇内最小 next_wakeup 写 genpd；offline/halt 的 CPU 不参与聚合。
- core_ctl sysfs：min_cpus/max_cpus/busy_up_thres/busy_down_thres/task_thres/offline_delay_ms/enable；`eval_need` 用 `sched_get_nr_running_avg`；默认 min_cpus=1；OPLUS pipeline scene 锁 max_cpus、`oplus_core_ctl_set_boost` 导出。
- walt_halt：stop_machine + drain RQ；选核掩码全剔 halted；`is_active = cpu_active && !cpu_halted`；系统首核禁 hotplug。
- ChiRi 含义：scenemode 整簇压制走 core_ctl sysfs，**勿硬写 cpu_down**（与 OPLUS boost/halt 状态机互踩）；钉核前应读 `cpu_halt` 状态；诊断可用 `trace_sched_compute_energy` / `trace_sched_task_util`。

### 真机实测档案（8550）

- core_ctl 快照（Alpha07-02）：policy0 min/max/enable=1/3/0；policy3=3/4/1；policy7=0/1/0——**仅 big enable=1**，little/prime 压核时 core_ctl 不可用（已接 enable 感知，见 03）。
- cpu_temp 两路读数：main snap 41.8-90.7℃ vs status.csv 恒 85-100 饱和——两路取自不同 zone，语义未定（07 T2）。
- 触摸地板行为级证据：touch=1 决策下限 ≥1651200（touch0 min 729600）；4 个内核 input_boost 节点全 ENOENT，实际挂载点未知（07 T9）。
- cap85 窗口（0926-162821，n=2217s）：第 15 列 cur_freq_khz（决策）在变、第 16 列 max_freq_khz（上限）恒满档——两列不可混算；free_above 豁免带覆盖 little 5.5 / big 2.3 / prime 3.1%；cap85 2.7844W vs cap100 3.1845W（未场景归一）。clamp_heavy 恒钳已实现、A/B 待做（07 T8）。
- `handle_cpufreq_transition` eBPF 探针挂载失败（内核无 CONFIG_CPU_FREQ_TRACEPOINTS）→ status/devimp 的 freq_trans 恒 0——「看似频率从未切换」实为无探针；ftrace 候选 `trace_dcvsh_freq`（07 T6）。
- 检索口径：daemon.log 是 fluent 本地化文案非 key 字面量；Grep/rg 读不动部分 x_*/daemon.log（编码异常），走显式解码脚本（详见 05）。
