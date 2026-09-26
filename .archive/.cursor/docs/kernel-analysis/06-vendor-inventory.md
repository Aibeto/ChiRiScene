# 厂商调度模块盘点：潜在竞争者清单（OnePlus 11 / SM8550 / 5.15.180）

> 生成：2026-09-26。方法：全树递归检索（rg -uu 绕过子模块忽略规则）+ 逐文件核读。
> 证据格式 `file:line`，相对 `android_kernel_oneplus_sm8550/`。

## 结论（先行）

1. **「ChiRi 是全局唯一调度程序」的口径不成立**：内核侧至少有 4 类常驻实体在 ChiRi 之外改频率/亲和/调度参数——WALT 调度器本体（含调度类替换、负载均衡、选核接管）、waltgov governor、freq_qos 约束体系（thermal / msm_performance / input-boost）、core_ctl+walt_halt（核在线/隔离）。
2. **最大意外发现：OPLUS 调度套件源码不在这棵树里**。`kernel/oplus_cpu` 是一个占位文件而非目录；`include/linux/oplus_omrg.h` 整个文件只有一行重定向 `../../kernel/oplus_cpu/oplus_omrg/oplus_omrg.h`。frame_boost、eas_opt（iowait/fake_cap）、OMRG、cpufreq_health、cpufreq_bouncing、SUGOV_TL、OCH 等在 `cpufreq_walt.c` 以 `CONFIG_OPLUS_*` 门控 include（`kernel/sched/walt/cpufreq_walt.c:21-52`），实现在完整编译树的 `kernel/oplus_cpu/`（另仓）。本报告只能给出其在 GKI 侧的挂接点，模块内部逻辑**未确认**。
3. **没有任何厂商模块直接写 `scaling_max_freq`/`scaling_governor` sysfs**——竞争都走 **freq_qos**（PM QoS 频率约束），它叠加在 policy->min/max 之上且优先级高于 userspace 写 sysfs 的语义（policy->max = min(用户值, QoS max)）。ChiRi 的 sysfs 写频模型需要正视 QoS 这一层。
4. **亲和性方面 WALT 是主动竞争者**：`android_rvh_sched_setaffinity` 缓存用户态请求的亲和掩码，cpuset 变更时 `android_rvh_update_cpus_allowed` 会**恢复**缓存掩码（`kernel/sched/walt/walt.c:5066-5106`）；`sched_getaffinity` 还会把 halted CPU 从返回掩码里扣掉（`walt.c:5077-5086`）。ChiRi 写 affinity 后若发生 cpuset 迁移，WALT 可能盖回。
5. userspace 暗示：`/proc/horae_qmi`（OPLUS Horae 热守护的 QMI 通道，`drivers/thermal/qcom/qmi_cooling.c:709`）与 `/sys/module/msm_performance/parameters/*`（性能锁守护接口）是最可能的外部盖写方。

## 目录清单（厂商代码分布）

| 位置 | 内容 |
| --- | --- |
| `kernel/sched/walt/` | WALT 调度器全家：walt.c（窗口统计/钩子注册）、walt_cfs.c（选核/preempt 接管）、walt_lb.c（负载均衡）、walt_rt.c（RT 选核）、walt_halt.c（CPU halt）、core_ctl.c（大核在线数）、boost.c（全局 boost）、input-boost.c（触摸提频）、cpufreq_walt.c（waltgov governor）、fixup.c、sysctl.c、sched_avg.c、walt_tp.c（thermal pressure）、oem_sched/（**空目录**） |
| `kernel/oplus_cpu` | **单个占位文件**，OPLUS 调度套件（frame_boost/eas_opt/OMRG/cpufreq_health 等）源码不在此树 |
| `drivers/soc/qcom/` | `msm_performance.c`（min/max 频率 QoS + PLH 性能锁）、`dcvs/memlat.c`（DDR 侧）、`hyp_core_ctl.c`、`rq_stats.c`、`core_hang_detect.c` 等 |
| `drivers/cpufreq/` | `cpufreq.c`（android_vh 频率钩子调用点）、`qcom-cpufreq-hw*`（实际驱动）、`qcom-cpufreq.c`（旧 clk 驱动，SM8550 上应为未启用，未确认） |
| `drivers/thermal/` | `cpufreq_cooling.c`（热限频）、`qcom/qmi_cooling.c`（horae_qmi）、`qcom/thermal_zone_internal.h` |
| `drivers/oplus_inject/` | 仅 `Kconfig`（故障注入），与调度无关 |
| `drivers/misc/` | `oplus_rf_cable_monitor.c`、`oplus_gpio.c`（/proc 节点，非调度） |
| `mm/oplus_mm`、`kernel/locking/oplus_locking`、`net/oplus_modules` | 同样疑似占位/非调度（未逐一确认） |
| `techpack/`、`vendor/` | **不存在** |

## 竞争者表（按能力）

### A. 频率（写 policy min/max 或绕过决策）

| 能力 | 模块 | file:line | 触发条件 | 与 ChiRi 冲突面 |
| --- | --- | --- | --- | --- |
| 频率决策本体 | waltgov governor | `kernel/sched/walt/cpufreq_walt.c:260-266`（fast_switch→`cpufreq_driver_fast_switch`）、`:890`（slow path `__cpufreq_driver_target`） | 每 util 更新 tick | ChiRi 写 scaling_max_freq 只是改 clamp，实际频率由 waltgov 按 WALT util 连续决策 |
| 下发瞬间改频钩子 | `android_vh_cpufreq_fast_switch` / `android_vh_cpufreq_target` / `android_vh_cpufreq_resolve_freq` | `drivers/cpufreq/cpufreq.c:2141 / :2300 / :542` | 每次频率下发 | 注册方在树外 OPLUS 模块（OMRG 等），可在 ChiRi 的 scaling_max 写入后仍改 target |
| OMRG 频率限制 | `omrg_cpufreq_register(policy)`（check_limit 在树外实现） | `kernel/sched/walt/cpufreq_walt.c:1511, :52` | waltgov 初始化时挂入 | 游戏场景（OMRG=游戏限帧/限频联动）可能压低 waltgov 实际频率 |
| min 频率 QoS（触摸升频） | input-boost | `kernel/sched/walt/input-boost.c:59, :289-293`（per-CPU `FREQ_QOS_MIN` 请求）、`:73`（boost 时 `freq_qos_update_request`） | 触摸事件（sysctl `input_boost_freq`） | ChiRi 的 scaling_min_freq 写入被 QoS min 抬高；devimp 读到的 cur_freq 可能是 boost 结果 |
| min/max 频率 QoS（性能锁） | msm_performance | `drivers/soc/qcom/msm_performance.c:112-115`（`cpu_min_freq`/`cpu_max_freq` 节点）、`:213-214, :281, :292`（per-CPU QoS 请求） | userspace 写 `/sys/module/msm_performance/parameters/cpu_{min,max}_freq`（性能守护/HAL） | 与 ChiRi 的 min/max 双向竞争；PLH 滚动/启动加速 IPC 频表（`:1093-1170`）走 aDSP 旁路 |
| 热限频 | cpufreq_cooling | `drivers/thermal/cpufreq_cooling.c`（freq_qos 限 max）+ 热压力 `kernel/sched/walt/walt_tp.c` | 温度越限 | 压制区间与 ChiRi cap 语义重叠；地板（policy->min）也会被 thermal 提升 |
| SUGOV_TL 目标负载 | waltgov 内 `update_util_tl`（`android_vh_map_util_freq_new`） | `kernel/sched/walt/cpufreq_walt.c:1652`（注册） | CONFIG_OPLUS_FEATURE_SUGOV_TL 时 map_util_freq 处改频率 | 直接改「util→freq」映射，ChiRi 读到的 util 语义与实际频率脱钩 |
| 其它 OPLUS 频率插件 | IOWAIT_PROTECT / FAKE_CAP / BOUNCING / OCH / POWER_EFFIENCY | `kernel/sched/walt/cpufreq_walt.c:21-52`（include 门控） | CONFIG_* 开启时 | 实现 in 树外，行为未确认 |

### B. 亲和性

| 能力 | 模块 | file:line | 触发条件 | 与 ChiRi 冲突面 |
| --- | --- | --- | --- | --- |
| affinity 缓存+恢复 | walt.c `android_rvh_sched_setaffinity` / `android_rvh_update_cpus_allowed` | `kernel/sched/walt/walt.c:5088-5106`（缓存）、`:5066-5075`（cpuset 变更时 `set_cpus_allowed_ptr` 恢复）、`:5212-5213`（注册） | sched_setaffinity 后再发生 cpuset attach/迁移 | **ChiRi 最需警惕的亲和竞争者**：ChiRi 写的亲和被 WALT 缓存视为「用户请求」，cpuset 事件后被恢复 |
| getaffinity 掩码过滤 | walt.c `android_rvh_sched_getaffinity` | `kernel/sched/walt/walt.c:5077-5086` | 每次 getaffinity | 返回掩码扣掉 cpu_halt_mask，ChiRi 读到的可用掩码与真实 allowed 不同 |
| CPU halt/隔离 | walt_halt.c | `kernel/sched/walt/walt_halt.c:632-635`（`set_cpus_allowed_by_task`/`is_cpu_allowed` 钩子） | walt_halt 激活（功耗/热） | ChiRi 想绑的核可能被 halt 拒绝（EINVAL 路径） |
| 选核接管 | walt_cfs.c / walt_rt.c | `kernel/sched/walt/walt_cfs.c:1711, :1718-1719`、`walt_rt.c:441-442` | CFS/RT 任务唤醒与抢占 | ChiRi 不做选核，无直接冲突；但影响 ChiRi 亲和策略的效果预期 |
| 负载均衡 | walt_lb.c | `kernel/sched/walt/walt_lb.c:1196-1199`（nohz kick/can_migrate/busiest_queue/newidle） | 均衡 tick | 可能迁移 ChiRi 绑定的任务（受 cpuset/cpus_allowed 约束，但不受 ChiRi 的软绑定约束） |
| 大核在线控制 | core_ctl | `kernel/sched/walt/core_ctl.c:444-455`（min_cpus/max_cpus/enable 等 sysfs） | 负载阈值 | 低端核压制/大核下线改变拓扑，ChiRi 的核编号假设会失效 |

### C. uclamp / 调度参数

| 能力 | 模块 | file:line | 触发条件 | 与 ChiRi 冲突面 |
| --- | --- | --- | --- | --- |
| uclamp 写入 | 树内厂商模块 | **无命中**（全树 rg 检索 `uclamp_set|uclamp_rq_*|cpu.uclamp` 在 kernel/oplus_cpu、walt、thermal、devfreq、soc/qcom、power 均零匹配） | — | 树内无竞争；树外 frame_boost 大概率写 uclamp（同类设备已知），**未确认** |
| cgroup cpu.uclamp 节点 | GKI core.c 原生 | `kernel/sched/core.c`（cgroup cpu controller） | userspace 写 `cpu.uclamp.min/max` | 与 ChiRi 的 cpu.uclamp 写共享同一命名空间，OPLUS 框架层（树外）若写即冲突 |
| 全局 boost | walt boost.c | `kernel/sched/walt/boost.c:250-272`（`sched_set_boost`/`sysctl_sched_boost`）、`:312`（FULL_THROTTLE） | userspace 经 sysctl/接口 | boost 会改调度的容量/频率预期，与 ChiRi 的 tuned 决策叠加不可预测 |
| RTG boost | walt rtg 机制 | `kernel/sched/walt/walt.h:246, :759-898`（`sysctl_walt_rtg_cfs_boost_prio`、coloc 组） | frame_boost（树外）把任务归组 | ChiRi devimp 读到的 util 语义受 rtg boost 影响 |
| sched sysctl | walt sysctl.c + walt.h | `kernel/sched/walt/sysctl.c` | userspace/树外模块 | `/proc/sys/kernel/sched_*` 上 ChiRi 与厂商共用 |

### D. /proc 与 /sys 自建节点（调度相关）

| 节点 | 位置 | 用途 |
| --- | --- | --- |
| `/proc/horae_qmi`（0666） | `drivers/thermal/qcom/qmi_cooling.c:709` | OPLUS Horae 热守护 QMI 通道（热限频协商） |
| `/sys/module/msm_performance/parameters/cpu_{min,max}_freq`、`…/notify`、`…/events` | `drivers/soc/qcom/msm_performance.c:193-199, :816-842` | 性能锁/频率约束 |
| `/sys/devices/system/cpu/core_ctl/*`（min_cpus/max_cpus/enable/…） | `kernel/sched/walt/core_ctl.c:444-455` | 大核在线数 |
| `/sys/module/waltgov/...`（hispeed_freq 等 tunables） | `kernel/sched/walt/cpufreq_walt.c:997-1020, :1335` 附近 | governor 参数 |
| `/proc/uid_cputime`、`/proc/uid_io`、`/proc/uid_procstat` | `drivers/misc/uid_sys_stats.c:540-569` | 统计（只读，无竞争） |
| `/proc/oplus_rf/rf_cable` 等 | `drivers/misc/oplus_rf_cable_monitor.c:437-448` | 非调度 |
| frame_boost/OMRG 的节点 | 树外（`kernel/oplus_cpu/...`） | 名单不全，**未确认** |

## WALT 本体概览

`kernel/sched/walt/` 以窗口化（ravg）任务负载统计替代 PELT 作为全调度器的 util 语义源：walt.c 负责窗口滚动/任务统计并通过约 30 个 vendor hook 注入 GKI 调度器（注册清单 `walt.c:5194-5221`）；walt_cfs.c/walt_rt.c 接管 CFS/RT 选核与抢占，walt_lb.c 接管负载均衡；cpufreq_walt.c（waltgov）用同一 util 做频率决策；RTG（related thread group，`walt.h`）把同组任务聚合需求并提供 boost 优先级（`sysctl_walt_rtg_cfs_boost_prio`）。**对 ChiRi 的「util 定义权」含义**：devimp 读到的 util/决策负载是 WALT 窗口均值再经 rtg/boost/iowait（树外）修正后的结果，ChiRi 不能假设它等于任何标准 CFS 语义；树外插件（SUGOV_TL、fake_cap、iowait）还会直接改 util→freq 映射。

## userspace 暗示

- 内核树无 .rc/配置文件（检索仅命中 tools/Documentation 的无关文件）——userspace 证据只能从节点反推。
- `horae_qmi`（0666）→ OPLUS Horae 热调度守护常驻。
- `msm_performance/parameters/cpu_{min,max}_freq` + PLH IPC 频表 → 存在性能锁服务（OnePlus 性能引擎/游戏模式 HAL）。
- `sysctl_sched_boost`、`input_boost_freq`、core_ctl sysfs → 需要一个常驻写入者（framework/perfd 类），设备上大概率活跃。

## 对 ChiRi 的含义

1. **口径修正**：「全局唯一调度程序」应改为「唯一 userspace **sysfs 写频**调度程序」。频率的实际决定权在 waltgov + vendor hook 链（OMRG/SUGOV_TL 等），ChiRi 的 scaling_max/min 是 clamp 不是决策；devimp 的 `cur_freq_khz` 比率（cap%）要考虑 QoS 层的额外 clamp。
2. **最可能盖写锁频的实体**（按概率）：① thermal（cpufreq_cooling + Horae，持续、事件驱动）；② msm_performance 的 QoS（性能锁/游戏模式期间）；③ input-boost（触摸瞬间，min 抬升，与「decision=down 但 cur_freq 不动」的地板现象吻合——见 devimp 判读口径第 3 条）；④ 树外 OMRG/frame_boost（游戏场景）。
3. **最可能盖写亲和的实体**：walt.c 的 cpuset 恢复路径（`walt.c:5066-5075`）——ChiRi 的 sched_setaffinity 会被缓存进 `wts->cpus_requested`，cpuset 变更时被「恢复」成 ChiRi 最后一次写的值，这其实是**对 ChiRi 有利**的行为；但 walt_halt/core_ctl 下线核会使 ChiRi 的绑定目标失效（e3/e22 errno 来源候选）。
4. **防篡改兜底该盯的节点**：`scaling_{max,min}_freq`、`cpu.uclamp.{min,max}`、`/sys/module/msm_performance/parameters/cpu_{min,max}_freq`、core_ctl `enable/min_cpus`、`/proc/horae_qmi` 存在性（Horae 活跃标志）。比对策略建议「记账+读回」双重校验（同 current_mode.chr 口径），对 QoS 类无文件句柄可看的约束，用 snap 行 `cpu_cur_khz` 与 `max_freq_khz` 比率的异常检测兜底。
5. **未决项**：树外 `kernel/oplus_cpu`（frame_boost/eas_opt/OMRG/cpufreq_health）的行为全部未确认；若后续拿到完整厂商仓，应补一份对 frame_boost 的 uclamp/亲和写入路径专项分析。

## 附：本次检索方法备注

Grep/Glob 工具对该子模块目录的目录遍历失效（外层仓库把 `android_kernel_oneplus_sm8550` 记为 gitlink，遍历整体被跳过），但**显式文件路径可用**。全树检索通过 `rg -uu --no-ignore` 完成（绕过忽略规则）。终端调用 3 次：目录清单×2、全树扫描×1；scratch 已删除。
