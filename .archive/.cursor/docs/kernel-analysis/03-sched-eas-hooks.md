# 内核调度 EAS / capacity 路径与厂商改动分析（OnePlus 11, SM8550, Linux 5.15.180 GKI + QCOM msm）

> 2026-09-26。内核源码：`android_kernel_oneplus_sm8550`（只读分析）。
> git 顶部提交：`b9bdf47513b9`（OnePlus 16.0.5.702 / QCOM AU_LINUX_KERNEL.PLATFORM.2.0.R1.00.00.00.004.146）。
> 结论先行：**真机唤醒选核走的是 QCOM WALT 的 `walt_find_energy_efficient_cpu`，不是 GKI 的 `find_energy_efficient_cpu`**——WALT 通过 `android_rvh_select_task_rq_fair` vendor hook 在 GKI EAS 之前整体劫持了 fair 类选核。因此 ChiRi 的「uclamp.max=85 钳 EAS capacity 视图」在 GKI FEEC 路径成立，但在真机默认路径上主要是**限频效应**（schedutil effective_cpu_util）而非选核效应。

---

## 1. EAS 选核

### 1.1 GKI EAS（`kernel/sched/fair.c`）存在且基本为主线 5.15 形态

- `find_energy_efficient_cpu()` 存在于 `fair.c:7030`，含主线 5.15 的扩展（`latency_sensitive` / `boosted` 分支，`fair.c:7080-7081`、`fair.c:7146` 用 `capacity_orig_of` 找 idle 目标簇）。
- **未发现厂商对 GKI FEEC 函数体的直接改写**；厂商影响全部通过 vendor hook 注入（见 §4）。
- 关键内部逻辑（与 ChiRi 相关）：
  - 入口先查 `rd->overutilized`，**overutilized 时 EAS 直接放弃**（`fair.c:7061`）。
  - 候选过滤 `util_fits_cpu()`（`fair.c:4121`）：`fits_capacity()` 带 ~20% 余量 `cap*1280 < max*1024`（`fair.c:123`），且用 `max(rq_util_max, p_util_max)` 做 uclamp.max 检查（`fair.c:7113-7146`）——**rq 级 max 聚合：任务自身 uclamp.max=85 不会把该 rq 的聚合值压到 85 以下**，只在 idle rq 上用任务自己的值。
  - uclamp.max 通过 `uclamp_eff_value(p, UCLAMP_MAX)`（`fair.c:7035`）进入 `p_util_max`，再经 `compute_energy()`/`em_cpu_energy` 的 util 钳制影响能量估计；能量估计可被 `android_vh_em_cpu_energy` 整体改写（`fair.c:6984-6986`，本树内无注册者）。
  - 只有当 best 比 prev 省 ≥ 1/16（~6%）能量才迁移（`fair.c:7201-7205`）。

### 1.2 真机实际路径：WALT 劫持 select_task_rq_fair

- `select_task_rq_fair()` 开头调 `trace_android_rvh_select_task_rq_fair(p, prev_cpu, sd_flag, wake_flags, &target_cpu)`，`target_cpu >= 0` 即直接返回（`fair.c:7237-7243`）。
- WALT 注册了该 hook：`walt_cfs.c:1711` `register_trace_android_rvh_select_task_rq_fair(walt_select_task_rq_fair, NULL)`；handler 在 `walt_cfs.c:1295-1312`，无条件 `*target_cpu = walt_find_energy_efficient_cpu(...)`。
- 即 **WALT 启用时（默认启用，`walt_disabled` 才走 GKI 路径）GKI FEEC 是死代码**。WALT 选核（`walt_cfs.c:1047-1290`）要点：
  - fastpath：many-wakeup 回 prev_cpu（`walt_cfs.c:1060`）；pipeline/低延迟任务直取 `pipeline_cpu`（`walt_cfs.c:1091-1108`）；sync 唤醒 bias 当前 CPU（`walt_cfs.c:1147`）。
  - `walt_get_indicies()` 按 task_boost / `uclamp_boost`（= uclamp.min>0，`walt_cfs.c:1062`）选候选簇顺序（order_index/end_index）；`need_idle` 含 `uclamp_latency_sensitive(p)`（`walt_cfs.c:1137`）。
  - `walt_find_best_target()` 选候选 CPU，prev_cpu 不 overutilized 则参与能量比较；能量差 < prev/32 才留在 prev（`walt_cfs.c:1257-1263`，注释写 6%，代码是 `>>5`≈3.1%）。
  - **OPLUS 后处理可整体改写结果**：`set_frame_group_task_to_perfer_cpu`（`walt_cfs.c:1267`）、`set_ux_task_to_prefer_cpu`（`walt_cfs.c:1274`）。

## 2. capacity 计算

- **DT 来源**：`drivers/base/arch_topology.c:271-296` `topology_parse_cpu_capacity()` 解析 `capacity-dmips-mhz`；`arch_topology.c:250-267` 按每簇 max freq（`freq_factor`）归一化；`arch_topology.c:328-357` cpufreq policy 注册通知后再补算。真机 280/855/1024 来自厂商 DT（本树内核 arch/arm64/kernel/topology.c 无 capacity 逻辑，QCOM 把它留在 arch_topology.c / DT；280/855/1024 与 mainline 326/693/1024 的覆盖关系已知，未在内核树内逐节点复核）。
- **freq-invariance**：`drivers/base/arch_topology.c:26-118` 的 `scale_freq_data` / `topology_set_scale_freq_source` / `topology_scale_freq_tick()`（AMU 计数器），`util` 侧不变性由此提供。
- **运行时重算（WALT 接管）**：
  - GKI 侧触发点在 `fair.c:8752-8762`（负载均衡路径的 `update_cpu_capacity()`），内部调 `trace_android_rvh_update_cpu_capacity(cpu, &capacity)`。
  - WALT 注册该 hook（`walt.c:5195`）并在 handler（`walt.c:4362-4369`）里调 `update_cpu_capacity_helper()`（`walt.c:4310-4356`）：
    `cpu_capacity_orig = min(fmax_capacity × cluster->max_freq/max_possible_freq, fmax − thermal_pressure)`
    —— 即 capacity 随 cpufreq max 限制和热压力动态收缩，**再写回 `*capacity`**，保证 `capacity_of() ≤ capacity_orig_of()`。
  - **OPLUS FAKE_CAP**：`CONFIG_OPLUS_FEATURE_FAKE_CAP` + `eas_opt_enable` 时，`cpu_capacity_orig` 再乘 `fake_cap_multiple[cluster_id]`（百分比，`walt.c:4339-4344`），同时 WALT FBT 的 util 阈值也被缩放（`walt_cfs.c:917-959`）——运行时改写各簇 capacity 视图，静态 capacity 数字会失真。

## 3. uclamp 聚合（`kernel/sched/core.c`）

- 任务有效值链：`uclamp_eff_get()`（`core.c:1498`）= clamp(任务请求值, tg 的 [tg_min, tg_max], 系统默认)。tg 约束在 `uclamp_tg_restrict()`（`core.c:1462-1483`）：**任务请求先被 task_group 的 uclamp 钳住**（root/autogroup 除外）。hook `android_rvh_uclamp_eff_get`（`core.c:1506`）可整体改写，本树内无注册者。
- `uclamp_eff_value()`（`core.c:1517`）：task 处于 refcounted 状态用回填的 effective 值。
- rq 级聚合是 **max 语义**（bucket 内取最大，`core.c:1655-1745` enqueue/dequeue 重算）；FEEC 里显式 `util_max = max(rq_util_max, p_util_max)`（`fair.c:7125-7145`）。
- `sched_setattr` 写 uclamp 的点挂了 `android_vh_setscheduler_uclamp`（`core.c:1923,1930`）。
- 频率路径：`effective_cpu_util()`（`core.c:7307`）内 `uclamp_rq_util_with(rq, util, p)`（`core.c:7347`）——rq 聚合 clamp 直接决定 schedutil 请求的 util→freq；`android_rvh_effective_cpu_util` hook（`core.c:7315`）可改写，本树内无注册者。
- **top-app cgroup 与任务级叠加**：Android userspace 给 top-app tg 设 uclamp 时，任务级 sched_setattr 的值被 tg 区间再钳制（`core.c:1462-1483`）；若 tg 上限 ≥85，ChiRi 的 85 生效；若 tg max 更低则被压到 tg 值。top-app/foreground/background 的 cgroup/rc 配置属 userspace（本内核树内无 init rc，未确认具体 tg uclamp 值）。

## 4. vendor hooks 清单（调度相关）

hook 声明集中在 `include/trace/hooks/sched.h`；`kernel/sched/vendor_hooks.c` 定义。**本树内唯一注册者是 `kernel/sched/walt` 模块**（QCOM WALT）；drivers/ 下未见调度 hook 注册。标注「无注册者」的 hook 其注册方在树外（OPLUS `kernel/oplus_cpu`，本树缺源码）或无人注册。

### 4.1 WALT 已注册（kernel/sched/walt/）

| hook | 注册处 | 作用 |
| --- | --- | --- |
| `android_rvh_select_task_rq_fair` | walt_cfs.c:1711 | **整体接管 fair 选核**（WALT FEEC），GKI EAS 变死代码 |
| `android_rvh_select_task_rq_rt` | walt_rt.c | RT 选核接管（halt/lowest_mask 处理，walt_rt.c:384-426） |
| `android_rvh_update_cpu_capacity` | walt.c:5195 | 重算 cpu_capacity_orig（freq/thermal/FAKE_CAP）并写回 |
| `android_rvh_update_misfit_status` | walt.c:5203 | misfit 判定接管（WALT demand） |
| `android_rvh_sched_setaffinity` | walt.c:5213 | 缓存用户态请求的亲和掩码（`wts->cpus_requested`，非内核线程，walt.c:5087-5106） |
| `android_rvh_update_cpus_allowed` | walt.c:5212 | **cpuset 变化时恢复缓存亲和**（walt.c:5065-5075） |
| `android_rvh_sched_getaffinity` | walt.c:5214 | 返回掩码中剔除 halted CPU（walt.c:5077-5085） |
| `android_rvh_build_perf_domains` | walt.c:5218 | EAS 可用性检查接管（walt.c:5131） |
| `android_rvh_cpu_cgroup_online/attach` | walt.c:5210-5211 | cgroup 上线/attach 时 WALT 侧初始化（rtg 等，walt.c:3424-3436） |
| `android_rvh_check_preempt_wakeup` / `rvh_replace_next_task_fair` | walt_cfs.c:1718-1719 | MVP 任务抢占/挑 next 接管 |
| `android_rvh_after_enqueue_task` / `after_dequeue_task` / `rvh_enqueue_task` / `dequeue_task` | walt.c:5204-5205 | WALT 窗口统计 |
| `android_rvh_try_to_wake_up` / `rvh_wake_up_new_task` / `rvh_ttwu_cond` / `rvh_sched_exec` | walt.c:5194/5196/5216-5217 | 唤醒路径统计与 many-wakeup 判定 |
| `android_rvh_set_task_cpu` / `rvh_sched_cpu_starting/dying` / `rvh_tick_entry` / `rvh_schedule` / `rvh_new_task_stats` / `rvh_flush_task` / `rvh_account_irq_start/end` / `rvh_sched_fork_init` / `rvh_do_sched_yield` / `rvh_update_thermal_stats` / `vh_scheduler_tick` | walt.c:5196-5221 | 统计、迁移、热、tick |
| `android_vh_binder_wakeup_ilocked` / `vh_binder_set_priority` / `restore_priority` | walt_cfs.c:1713-1716 | binder 事务低延迟/优先级继承 |
| `android_rvh_show_max_freq` | fixup.c:91 | 针对 sched_lib 应用伪造 cpuinfo max freq（游戏场景） |
| `android_vh_update_topology_flags_workfn` | walt.c:5379 | topology 更新联动 |

### 4.2 有调用点、本树内无注册者（注册方在树外或未注册）

| hook | 调用点 | 潜在作用 |
| --- | --- | --- |
| `android_rvh_find_energy_efficient_cpu` | fair.c:7048 | GKI FEEC 入口整体替换 |
| `android_rvh_uclamp_eff_get` | core.c:1506 | 任务有效 uclamp 改写（可废掉 85 钳） |
| `android_rvh_effective_cpu_util` | core.c:7315 | schedutil util 改写 |
| `android_vh_em_cpu_energy` | fair.c:6984 | 能量模型整体替换 |
| `android_rvh_place_entity` | fair.c:4381 | vruntime 放置改写 |
| `android_vh_map_util_freq` / `map_util_freq_new` | cpufreq_schedutil.c:163-164 | util→freq 映射改写（`walt/cpufreq_walt.c` 含相关调用，具体注册者未确认） |
| `android_rvh_cpu_overutilized` | fair.c（overutil 判定） | 可改写 overutilized 翻转 |
| `android_rvh_set_cpus_allowed_by_task` / `rvh_set_cpus_allowed_ptr_locked` / `rvh_sched_setaffinity_early` | core.c | 亲和设置拦截 |
| `android_vh_setscheduler_uclamp` | core.c:1923,1930 | uclamp 写入拦截 |

## 5. OPLUS / OnePlus 定制摘要

- **`kernel/oplus_cpu/` 目录在本树中为空**（只有目录名，`dir /s` 无任何文件），但 `kernel/sched/*.c`、`kernel/sched/walt/*.c` 里保留大量 `#include <../kernel/oplus_cpu/sched/...>` 与 `CONFIG_OPLUS_*` 代码块。即 OPLUS 模块源码未随此同步放出，代码以 ifdef 形式留在 GKI 侧； OnePlus 官方构建里这些 CONFIG 是开着的（缺源码，**各 CONFIG 的实际开关状态未确认**）。
- 保留在主线文件里的 OPLUS 定制点：
  - `CONFIG_OPLUS_SCHED_TUNE`：schedtune 式 cgroup（`core.c:10146` schedtune_attach、`core.c:10191-10244` cgroup alloc/release、`fair.c:5832` schedtune_enqueue_task）——影响组级 boost/限制。
  - `CONFIG_OPLUS_FEATURE_SCHED_ASSIST`：ux 任务识别与 prefer-cpu 改写选核结果（`walt_cfs.c:1273-1279`）、spread_tasks（`walt_cfs.c:489-497`）、MVP 逻辑（`walt_cfs.c:1378-1655`）。
  - `CONFIG_OPLUS_FEATURE_FRAME_BOOST`：帧同步任务直接指定 prefer cpu（`walt_cfs.c:1266`）。
  - `CONFIG_OPLUS_FEATURE_PIPELINE`：pipeline_cpu fastpath（`walt_cfs.c:1091`）。
  - `CONFIG_OPLUS_FEATURE_FAKE_CAP`：eas_opt 运行时改写 capacity（`walt.c:4339`）与 FBT util 阈值（`walt_cfs.c:917-959`）。
  - 其它：`WINDOW_POLICY`（WALT 窗口策略）、`GKI_CPUFREQ_BOUNCING`、`CPUFREQ_IOWAIT_PROTECT`、`SCHED_GROUP_OPT`、`ABNORMAL_FLAG`、`SUGOV_TL`、`CPU_AUDIO_PERF`。
- WALT 本体（QCOM）：`kernel/sched/walt/`（walt.o boost.o walt_halt.o core_ctl.o input-boost.o cpufreq_walt.o walt_lb/rt/cfs.o 等，Makefile:7）。**cpu_halt / core_ctl**：`core_ctl.c:1354` 可 pause CPU（`walt_halt_cpus`），halted CPU 被 getaffinity 隐藏、被所有选核路径避开（walt.h:1017-1018）。

## 6. 对 ChiRi 的含义

1. **85 钳制的真实效果路径**：真机默认路径下 GKI FEEC 不运行，`uclamp.max=85` 的「EAS capacity 视图钳制」（85%×1024≈870，让 prime 看起来过大）**只在 `walt_disabled` 或 hook 未注册时成立**。常态生效的是：
   - schedutil 限频：`effective_cpu_util` 的 rq 级 clamp（max 聚合——**只有当该 CPU 上所有 runnable 任务的有效 uclamp.max 都 ≤85 时，rq 值才被压到 85**；任何高 clamp 任务在同一 rq 上会把频率顶回去）。这与日志里「cap 列 - / cur_freq 不动」的观察一致。
   - WALT 内只影响 `walt_uclamp_boosted`（min）与 `uclamp_latency_sensitive` 的分类，未见用 uclamp.max 钳 WALT demand（未确认）。
   - 若ChiRi 想让 85 真正参与选核，可评估走 `uclamp_tg_restrict`（top-app tg 层）或确认设备上 `android_rvh_uclamp_eff_get` 是否被 OPLUS 模块注册后改写。
2. **亲和（sched_setaffinity）比 uclamp 可靠**：affinity 是硬约束，WALT FEEC 只在 `p->cpus_ptr` 内挑核，**把线程钉在 little(0-2) 后不会被 EAS/WALT 搬出该簇**。但注意 WALT 会缓存用户亲和（`rvh_sched_setaffinity`）并在 cpuset attach 时恢复它（`rvh_update_cpus_allowed`）——ChiRi 的写入同样被缓存，行为可预期；反过来 ChiRi 若依赖 cpuset 变化收紧掩码，WALT 可能按旧缓存恢复更宽的掩码。
3. **会被改写/失效的点**：`set_ux_task_to_prefer_cpu` / `set_frame_group_task_to_perfer_cpu` 可对 ux/帧任务直接改写选核结果（仍在 affinity 内）；OPLUS `FAKE_CAP` 若启用会运行时改写各簇 capacity，ChiRi 按 280/855/1024 静态换算的 cap% 会失真；`android_rvh_uclamp_eff_get` / `effective_cpu_util` / `em_cpu_energy` 若被树外模块注册，85 与频率计算都会被劫持（注册方在缺失的 oplus_cpu 源码里，未确认）。
4. **halt/core_ctl 风险**：被钉 CPU 若被 core_ctl pause，任务将无法在该 CPU 运行且 getaffinity 会隐藏该位——ChiRi 钉 little 前/后应读 `cpu_halt` 状态（8550 上 core_ctl 管哪些簇未确认）。
5. **诊断面**：`android_rvh_tick_entry` / `vh_scheduler_tick` / `rvh_after_enqueue_task` 等都是 WALT 侧挂点，ChiRi devimp 若做内核侧打点可复用同类 hook；`trace_sched_compute_energy` / `trace_sched_task_util`（walt_cfs.c:1181,1283）可用来验证 WALT 选核决策。
