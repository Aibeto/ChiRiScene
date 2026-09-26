# devimp 数据挖掘结果 + 真机验证 TODO 清单

> 编写：2026-09-26。数据来源：`devimpbin/` 下两个已解压日志包（由前代理与本次接力共同解压，聚合表 `analyze.txt`/`main.txt`/`status.txt`/`report.md` 由 `scripts/devimp/dvrun.py` 预生成）。
> 目的：为 `optimization-plan.md` P2「真机取证」逐项标注本地已有证据，收窄真机上要做的事。
> **适用范围**：以下 TODO 命令与预期节点均为 **8550 真机（OnePlus 11 / 小米 13 Pro，5.15.194）口径**；8475/8998 内核形态不同（8998 为 4.4，无 WALT/DCVS/core_ctl 系节点），取证命令与结论不可直接套用，ChiRi 侧对这些节点的依赖均已设计为「缺失即降级」，不构成硬绑定。

## 0. 版本与数据包说明（先定版再读数据）

| 包                       | daemon 版本                                                                                                                | 设备                           | 系统 / 内核                                      | schema      | 备注                                                        |
| ------------------------ | -------------------------------------------------------------------------------------------------------------------------- | ------------------------------ | ------------------------------------------------ | ----------- | ----------------------------------------------------------- |
| `devimpbin/0926-030102/` | **ChiRi Canary Alpha06-18 (versionCode 10618)**（`x_0926-030100/daemon.log` 首行）                                         | 8550 / 2210132C（小米 13 Pro） | Android 17 (sdk 37) / 5.15.194-android13-8-00019 | 44 列拆分版 | 25min 会话，无 FAS 接管（fps 非空 0）；含 cap=85 短暂热压段 |
| `devimpbin/0925-143934/` | **ChiRi Canary Alpha06-16 (versionCode 10616)**（`x_0925-132525/daemon.log`、`x_0925-143931/daemon.log` 首行，两段同版本） | 同上                           | 同上                                             | 44 列拆分版 | 两个批次共 4782 行 status，全程放电、无 FAS                 |

**两个包的 daemon 版本（Alpha06-16/06-18）均旧于当前代码（Alpha07-02）**，行为差异以下述「对应计划项」为线索、最终以真机当前版本复测为准。

## 1. 本地挖掘发现（带证据位置）

### 1.1 【P1-3 关键输入】cpu_temp 存在两路读数，status.csv 一路在 100℃ 饱和

- main 日志 snap 行 `cpu_temp`（44 列第 34 列）：0926 全包 n=1502，min 41.8 / p50 52.3 / max 90.7 ℃（本轮统计，脚本遍历 70 个 main\_\*.log）。
- 同一会话 `x_0926-030100/status.csv` 的 `cpu_temp` 列：min 85.0 / **p50=100.0 / max=100.0**（1502 行，p90/p95 均 100）；0925 两个 csv（341+4441 行）**全部恒 100.0**。
- 两路 batt_temp 一致（27~35℃），排除时间错位；唯一解释是**两个读数取自不同 thermal zone（或不同换算）**，status.csv 来源是偏热且会在 100 饱和的 zone。
- daemon.log 中 **无 `soc_max`/`cpuss`/任何 zone 名线索**（关键词扫描零命中，`utils.rs::find_cpu_temp_path` 用了哪个路径日志不落盘）。
- **结论**：temperature 来源语义未定，必须真机枚举 zone；这与 P1-3（`find_cpu_temp_path` 名单补 `cpuss` 回退 + devimp 记录 zone type）直接对应。

### 1.2 热压触发特征与软限幅的「空转」嫌疑

- 0926 包仅 2 条 `thermal_change` 事件（`main_com.miui.home_0926-023557.log` / `main_com.tencent.mobileqq_0926-023603.log`）：
  - `02:36:01 batt=30.8 cpu=90.7 cap=85 free=95`（触发）
  - `02:36:05 batt=30.9 cpu=86.9 cap=100 free=95`（解除，滞后 ~4s）
  - **触发时电池仅 30.8℃**，与 cpu 90.7 强相关 → 触发量是 CPU 侧温度而非「41℃ 电池」旧口径（判读文档的 41℃ 口径需按此修正或标注版本差异）。
- cap=85 期间全部 34 个 tick 的决策 `max_freq` 仍为各簇硬件满档（little 2016000 / big 2803200 / prime 3187200），压制带 `(85,95)` 内 tick=0（`main.txt [5]`：in_band=0）→ **本地日志看不到软限幅实际降低决策值，无法证明 cap 生效路径**；0925 包全程 cap=95/100 无事件。
- 本地无 `dcvsh_freq_limit`/`lmh_freq_limit` 列，无法区分「QoS 钳制 vs 硬件热压」（铁律 3 五层归因的 ②④ 只能真机判别）。

### 1.3 「decision=down 但实际频率不动」的量化（0926 全包 25401 tick，按簇配对最近 snap）

| 簇               | down tick | snap 实际频不变 | 降  | 升  |
| ---------------- | --------- | --------------- | --- | --- |
| little (policy0) | 897       | 316 (35%)       | 306 | 273 |
| big (policy3)    | 3218      | **2308 (72%)**  | 463 | 445 |
| prime (policy7)  | 810       | 479 (59%)       | 167 | 162 |

- 典型样本（big 簇）：`03:00:37.162 决策cur=1651200 snap_p3=1651200(前snap=614400)`——决策降档后票值随后跟上；`03:00:38.284 决策cur=1651200 snap_p3=614400`——空闲核钉在 min，票值不动但那是「本来就没频可降」。
- 解读：多数「不动」是**票值语义（归因①）+ 空闲核贴 min**，不构成写频失败证据；但要排除 LMh/DCVS 热压（归因②）必须读 `dcvsh_freq_limit`，本地无数据。
- 附：aweme 单包（5 文件 1114 tick）同口径：down 160 帧、policy7 不变 129（81%，prime 全程贴 864000 地板）。

### 1.4 touch=1 抬频（ChiRi 触摸地板的直接证据）

- aweme（0926）：tick `touch=1` 164 帧 vs `touch=0` 950 帧，决策 `cur_freq`：
  - touch=1：min **1651200** / p50 1785600 / max 2707200
  - touch=0：min **729600** / p50 1555200 / max 3187200
- → touch 期间决策下限被抬到 ≥1.65GHz（低段被裁掉），是 ChiRi 自身触摸地板生效的行为级证据。
- 同时 daemon.log 每轮启动报 `[utils] [touch-boost-disable] all 4 node(s) unavailable (first: /sys/module/cpu_boost/parameters/input_boost_enabled … ENOENT)` → **本机内核不存在这 4 个 input_boost 节点**，ChiRi 想禁用厂商 input boost 但无从下手；真机需确认 qcom `input-boost` 模块（F6 提到其走 FREQ_QOS_MIN）是否以别的形态存在。

### 1.5 core_ctl 存在且已被 ChiRi 使用；msm_performance/Horae/OMRG 零痕迹

- `0925-143934/x_0925-143931/daemon.log` 有 8 条 `[chiri::core_ctl] [CoreCtl]`：`boost: 3 个 cluster 的 min_cpus 已抬到全组常在线` ↔ `已恢复 core_ctl min_cpus 快照`（14:08:06/14:08:31、14:08:42/14:11:26 等成对出现）→ **core_ctl 三簇节点在本机可用、boost/恢复闭环正常**。
- msm_performance / Horae / OMRG / frame_boost / `dcvsh` / `lmh` / `verify` / `clamp` / `重写`：两包 daemon.log **全部零命中**（关键词扫描，0926 37 行、0925 31+3388 行）。如实记录：本地日志无这些实体痕迹，活跃性只能真机查。

### 1.6 FAS verify/重写：无数据

- 两包 FAS 从未接管（`status.txt`：fps 非空 0；daemon.log 无 `fas-gear-switch`/policy_controller 痕迹）→ 无 verify/防篡改重写日志可挖。P1-1 的热压场景 A/B 只能真机做（且要先造出 FAS 激活场景）。

### 1.7 其他观测

- `handle_cpufreq_transition` eBPF 探针挂载失败（内核无该 tracepoint，daemon.log WARN）→ snap `freq_trans` 恒 0，ftrace 取证需换 `trace_dcvsh_freq` 等事件。
- snap 的 `cpu_min` 列未随 touch=1 抬升（aweme 63 个 snap 均无 touch=1 标记，样本缺失）——touch 地板只体现在 tick 决策值，未落到 FREQ_QOS_MIN 读数（或 snap 恰好没采样到触摸秒），真机可补验。

## 2. 真机验证 TODO 表

> 框架抄自 `optimization-plan.md` §3（P2-1~P2-4），按本地证据本地化。「对应计划项」列指向该文档。

| #   | 项                                                                 | 目的                                                                     | 命令（root adb shell）                                                                                                                                                                                                                                                       | 本地已有线索                                                                           | 对应计划项         |
| --- | ------------------------------------------------------------------ | ------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- | ------------------ |
| T1  | 导出 DT 并反编译                                                   | capacity/OPP 电压/thermal trip/cooling-map/idle-state 全实证             | `adb pull /sys/firmware/fdt fdt.img && dtc -I dtb -O dts fdt.img > sm8550.dts`（产物放 `mdocs/`）                                                                                                                                                                            | 本地日志无 DT 痕迹；280/855/1024 无 DT 实证                                            | P2-1               |
| T2  | thermal zone 全清单 + 确认 status.csv 的 cpu_temp 来源             | 解释「两路 cpu_temp、status.csv 恒 100 饱和」；为 P1-3 名单定案          | `for z in /sys/class/thermal/thermal_zone*; do echo "$z $(cat $z/type)"; done`；对候选 zone 逐个 `cat $z/temp` 与 ChiRi 两路读数同秒比对                                                                                                                                     | §1.1 双路读数差异（41.8~90.7 vs 85~100 饱和）；日志零 zone 名线索                      | P1-3 / P2-2        |
| T3  | cpuidle state latency/residency                                    | 05 报告空洞；qcom-cpu-lpm 三道闸实证                                     | `for c in /sys/devices/system/cpu/cpu*; do for s in $c/cpuidle/state*; do echo "$s $(cat $s/name) $(cat $s/latency) $(cat $s/residency)"; done; done`                                                                                                                        | 本地无 idle 数据                                                                       | P2-2               |
| T4  | `dcvsh_freq_limit` / `lmh_freq_limit` / `print_cpufreq_debug_regs` | 把铁律 3 归因②（硬件热压）从猜测变实证                                   | `cat /sys/devices/system/cpu/cpu7/cpufreq/*dcvsh_freq_limit`（每 policy 同理）、`cat /sys/kernel/qcom-cpufreq-hw/print_cpufreq_debug_regs`；热压时段连续采样。**devimp 已内置临时 `clamp_change` 事件行自动采集 dcvsh/core*ctl 旁证（变化才落行），直接读 main*\*.log 即可** | §1.3：72% big down 帧「实际不动」无法归因；本地无该列                                  | P0-1 / P2-2        |
| T5  | msm_performance / core_ctl / Horae 活跃性清点                      | 确认锁频盖写者与 core_ctl 管辖簇（P1-4 前置）                            | `cat /sys/module/msm_performance/parameters/cpu_{min,max}_freq`；`cat /sys/devices/system/cpu/cpu7/core_ctl/min_cpus`（big/prime 同理，确认管辖簇）；`ls /proc/oplus* /sys/kernel/oplus*`；Horae：`ls /sys/kernel/horae* 2>/dev/null`                                        | §1.5：core_ctl 三簇 boost/恢复闭环已实证；msm_performance/Horae/OMRG 零痕迹            | P1-2 / P1-4 / P2-2 |
| T6  | ftrace：WALT 选核 + 85 限频路径                                    | F2/F3 行为级确认；uclamp.max=85 究竟走哪条路                             | `tracefs` 挂载后 `echo 1 > events/sched/trace_sched_task_util/enable`（及 `trace_dcvsh_freq`），抓 30s 高负载段                                                                                                                                                              | 本地无 FAS 段、无 tracepoint（cpufreq_transition 缺失，§1.7）                          | P2-3 / P1-5        |
| T7  | uclamp.max=85 的 A/B 实验                                          | 决定 P1-5 是否动 tg 层钳制                                               | 同一游戏/视频场景，`uclamp.max=85` 与关闭各跑 15min，对比帧率、`scaling_cur_freq`、`dcvsh_freq_limit`、功耗（status.csv 口径）                                                                                                                                               | 85 走限频路径（F3）仅有内核侧推论，无行为数据                                          | P1-5               |
| T8  | 软限幅 cap 生效性 A/B（本地新增）                                  | 复现 §1.2「cap=85 决策仍满档」：确认压制带逻辑在当前版本是否真的降决策值 | 造热压（烤机至 CLG 触发），记录 `thermal_change` 事件前后 tick 决策值与 `cpu_cur_khz`；对照 feature.yaml `soft_perf_cap`/`free_above`                                                                                                                                        | cap=85 窗口 34 tick 决策全满档、压制带内 0 tick；`clamp_change` 行（临时）可作同秒旁证 | P1-1 关联          |
| T9  | 内核 input boost 节点探测（本地新增）                              | §1.4 的 4 节点全 ENOENT：确认厂商 boost 实际挂在哪                       | `ls /sys/module/cpu_boost/parameters/ 2>/dev/null; ls /sys/module/input_boost/ 2>/dev/null; grep -r input_boost /sys/kernel/qcom* 2>/dev/null`；触摸时读 FREQ_QOS_MIN 变化                                                                                                   | touch=1 决策下限 ≥1.65GHz（ChiRi 地板生效）；内核节点不存在                            | P1-2 旁证名单      |
| T10 | 热压场景 FAS verify 行为（P1-1 验证）                              | 防篡改重写 vs QoS 钳制死循环风险                                         | 热压 + FAS 白名单应用前台，抓 daemon.log `policy_controller`/verify 行与 scaling_max 读回值；`clamp_change` 行（临时）的 smax 段可作同秒读回佐证                                                                                                                             | 本地 FAS 未激活，无数据（§1.6）；clamp_change 已内置 smax 采集                         | P1-1               |

执行顺序建议：T2/T4/T5 一批（纯读取，成本最低、消歧最多）→ T8/T10（热压场景）→ T1/T3/T6/T7 按计划排。

## 3. 本地数据不足以回答、明确记「无数据」的项

- FAS verify / 防篡改重写日志（两包 FAS 从未激活）。
- `dcvsh_freq_limit`/`lmh_freq_limit` 数值与 LMh 触发记录。
- thermal zone type / trip / cooling-map、cpuidle latency、capacity 的 DT 实证。
- msm_performance、Horae、OMRG、frame_boost 的活跃性。
- tg 层 uclamp 钳制（`uclamp_tg_restrict`）的行为差异。
