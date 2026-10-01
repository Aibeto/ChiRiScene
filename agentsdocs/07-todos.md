## [todos] 未竟事项台账（2026-09-26 汇总）

> 历史计划/日志归档后的**唯一 TODO 权威**。开工前先查本文件；完成一项删一项。2026-09-17 审查提出的旧项标注「先核实」——动手前先确认现行代码是否已修。
>
> **口径变更（2026-09-28）**：R3「**永不再新增 A/B 测试**」生效（详见 `04-hard-lessons.md` 的 `[hard]` 首条）。下列以 A/B 为手段的条目**不再按 A/B 执行**，改为「离线重放 + 同版本观察」：[C3] 构建剖面、真机 A/B 整体、T7 uclamp.max=85 / T8 clamp_heavy 恒钳、telemetry gpu_busy 采集成本——逐条待裁决去留，其中涉及"采集开关对照"的直接作废。

### 性能与探针（perf-report backlog）

- [K3] 摘除纯遥测探针需 meta 开关：wakeups/migrations 被 WebUI 消费（status.csv 第 19/20 列 wakeups/migrations + devimp snap + telemetry-summary），属数据面变更——需显式开关 + 关时写 `-` + WebUI null 展示同步。
- [C3] 构建剖面（opt-level z/s/3 + thin LTO，比体积与 8550 实测功耗）——原 A/B 手段作废，待裁决：改同版本观察或直接作废。
- [E3] logdr events buffer（liblog 协议 150-250 行 + SELinux 可达性验证）；[E4] pid_watcher 并入 app_detect（时延 500ms→1.5s 取舍待拍板）。
  - **[E1] 内容比对短路已定案不做（2026-10-01）**：`app_detect.rs` 显式注明「刻意不做内容未变则复用」——冷启动期间 pid 先入组、exec 后 cmdline 才可读，缓存会漏检这类前台切换。
- 自测量基线：**status.csv 末尾两列 `daemon_utime_ms`/`daemon_stime_ms` 已落地（2026-10-01，1s 采样读 `/proc/self/stat`）**；`cargo bench` 三纯函数仍未做（需先定哪三个 + 新建 benches/ 基础设施）。
  - **「核秒」口径开销评估（2026-10-01，只评估未实现）**：核秒**不能**从 aff 的 `t` 行积分——`t` 行是槽级差分 +
    冷长尾降采样 + 每 30 帧刷新帧（`full=1`）放大行数，缺失行 = 未变，只能证明采样状态占比、不是核秒。
    最省的来源是 `cpu_monitor` 每 tick **已在算**的 `CoreState.busy_diff`（ns）：加一个累计量只需**每核每 tick
    一次整数加 + 一次存储**（160 ms × 8 核 ≈ 50 次加/秒，**无新 syscall、无新文件读**）。持久化二选一：
    ① status.csv 末尾**追加一列累计核秒**（≈ +1 MB/天，推荐，遵守「列只在末尾追加」惯例）；② 在 `@S` 帧加逐核行
    （≈ 150 B/s ≈ 13 MB/天，会部分抵消差分省字设计）。**结论：开销可忽略，真要落地走 ① 追加列**。
- 真机 A/B 整体未做（perf 16 项优化 + K1/K2 eBPF 同场景 status.csv 逐核 util 对比）；K1/K2 CI 构建确认（本机 stub 编译盲区，「CI 通过才算落地」）。
- **已定案不做（勿重评）**：U3 cpu_monitor 侧、A5b、panic=abort、换 hasher/parking_lot、热路径值缓存、事件化否决四项（cgroup.events poll / PSI / thermal uevent / timerfd 全量合并）、换语言与双进程拆分。

### 内核取证（详见 06-kernel.md）

- **T1** DT 反编译（capacity 出处 / OPP 电压 / trip / cooling-map / idle 厂商改动全实证）——最大单点缺口；**T2** thermal zone 全清单 + cpu_temp 两路定案；**T3** cpuidle state 全量抓取；**T4** 真热压时段 dcvsh/lmh 采样（常规负载已排除）；**T5** msm_performance / Horae 活跃性（部分定案：msmp 不可读、horae=1）；**T6** ftrace（trace_sched_task_util / trace_dcvsh_freq）；**T7** uclamp.max=85 A/B → P1-5 tg 层钳制立项与否；**T8** clamp_heavy 恒钳 A/B（cap85 2.78 vs 3.18W 基线）；**T9** input boost 实际挂载点（**2026-10-01 已定案：非字符串写错，是节点本体不存在**——见 `06-kernel.md` 真机实测档案）；**T10** 热压场景 FAS verify 行为。
- 树外 oplus_cpu 专项（frame_boost / eas_opt / OMRG / cpufreq_health 的 uclamp/亲和写入路径）。
- 次级未确认：OMRG/kalama 启用与否、msm_lmh_dcvs/lmh.c 是否 probe、厂商 defconfig cdev 清单、`sched_lpm_disallowed_time` 定义文件、WALT busy-hold 对应 waltgov 行号、cpuset 收紧被 WALT 恢复宽掩码的实测。

### 调度 / 机制验证

- **热档中档解除点修复（`hysteresis_mid_c`，2026-09-28 落地）——已实测生效（2026-10-01 结案）**：
  原中档与软档共用 `hysteresis_c=3` → 中档解除点 `43−3=40°C` **低于软限跳闸 41°C**，中档一咬住即跳过
  软档、须一路冷过 40°C 才恢复。8550 取 `hysteresis_mid_c: 1.5` → 解除点 `41.5°C`（> 软限 41）。
  **本包实证**（`devimpbin/1001-190148` `power.txt [5][6]`）：cap=60 温度带 **min 41℃**（0928 为 40，
  含 2556 s 落 40~41 桶）、驻留 **1251 s**（0928 5152 s），解除后直接交软档 cap=85（不再跨档跳过）。
  正文与 normalize 不变量见 `03-chiri.md` 四级判定条。
- ~~**息屏离核策略改为「下线 little」的真机验证（2026-09-28 落地，最高优先）**~~ → **2026-10-01 已结案（结论：不成立，已按修法3 改为不下线）**。结论来自 `logd_1001-045147`（A08-08/8550/PHB110/A16，9 批次、息屏 7.4 h）：**26 次进入 scenemode，20 次（77%）在 11~414 s 内被 `scenemode 持续顶满性能上限（little util 100%）→ 退回 reduce + 300 s 冷却` 撤销**，有效驻留 ≈3.6 h、另有 ≈1.7 h 处于禁入冷却（≈20×300 s）→ 下线 little 并没消除抖动，只是把瓶颈从「3 个小核」搬到了唯一在线的引导核 CPU0（判据 ①② 均命中，② 的「预期仍会触发」被证实，但对象是 little 引导核而非 big）。
  处置：`CoreCtl.scenemode_offline` 该字段此前是**死字段**（`mod.rs` 未接线，下线由 `enabled` 无条件驱动），已接线为真门控并在 8550 置 **false**（不下线任何核、保留全核，仅靠 `perf_ceil` 0.12 + uclamp 压制）。**待下包复核**：① 进入 scenemode 后是否仍出 saturation 撤销（目标：显著减少）；② 息屏段 `batt_power_w` 走向（全核在线的漏电 vs 免冷却的净收益）；③ 亮屏唤醒/电源键响应与 `corectl` 恢复是否干净（此时 core_ctl 不再介入 scenemode，`max_cpus`/`min_cpus` 应保持快照值不动）。
  其余子项（⑥⑦⑧⑨⑩）随「不下线」直接失效，无需再验；**软回退** = 8550/feature.yaml 的 `CoreCtl.scenemode_offline` 改回 true。
- **息屏期 little 被顶满 100%（2026-09-28 包实锤）**：**2026-10-01 更新**——本包给出定量结论（见上条结案），根因确认为「下线 little → CPU0 单核承压」；`logd_0928-170828` 里 7 次撤销属同一机制。以下原始取证线索保留备用：
  由 `ranges.prime` 改为 `ranges.little`（引导核 CPU0 无法热拔出，8550 实际下线 CPU1-2），core_ctl `max_cpus`
  写 **1**（`scenemode_keep_cpus()`，**不是 0**——0 是 walt_halt_cpus 整簇停摆语义，对含引导核的簇写 0 会把
  CPU0 一起停摆），big / prime 改常驻低频，调度服务独占核由「编号最大 little」改为「编号最大 big」。
  判据（同版本长时观察，不做对照）：① `status.csv` 息屏段 `batt_power_w` 与 `dvpower [5][6]` 的档位/温度带
  ——**大核最低频能效低于小核，息屏底噪可能不降反升**；② 是否仍出 `scheduler-scene-mode-saturation`
  （判据取 little∪big 每核 util 最大值，换常驻核不影响它生效，**预期仍会触发**，只是对象从 little 变 big）；
  ③ `corectl-reassert-offline` / `corectl-vendor-override` 是否刷屏（= core_ctl 把核拉回、在拉锯）；
  ④ 亮屏唤醒/电源键响应延迟、`corectl-scenemode-off` 恢复是否干净（含 max_cpus 快照回写）；
  ⑤ 独占大核后业务少 1 颗 big 的影响（亮屏恢复时应释放）。**软备份**：把 `scenemode_targets()` 换回
  `ranges.prime`（core_ctl 路径会自动回到 halt 语义）。
  ⑥（独立审查后补的护栏，需真机确认其是否触发）真机确认 little 簇快照 `min_cpus` 是否 >1——>1 时实现会先把它
  压到 `keep`(=1)、退出再按快照写回；日志出现 `corectl-reserved-core-halted` 说明该内核把保留档也当停摆档
  （引导核掉线），必须放弃 core_ctl 路径只走逐核 offline。
  ⑦（元审查后补）`min_cpus` 的压低/写回是**独立记账**（`min_cpus_lowered`），不受 `max_cpus` 是否写回成功影响；
  真机看退出后 `min_cpus` 是否回到快照（`cat .../core_ctl/min_cpus`）——若长期停在 1，说明写回一直被内核拒
  （`max_cpus` 尚未解除或节点被锁），日志会持续 `corectl-write-failed`（debug 级）。
  ⑧（元审查后如实记录，非缺陷）进程在 scenemode 中途被杀时，被独占大核的 **cpuset 排除会残留在磁盘**上，
  而该排除是 ChiRi 用快照恢复的、快照随进程丢失 → 能否自愈**依赖系统 CpusetManager 重写组 cpus**（代码注释
  既有观察）与「之后进入 boost 时覆盖写回」，ChiRi 自身不保证；真机可 `cat /dev/cpuset/top-app/cpus` 复核
  编号最大的大核是否在列。
  ⑨（第二轮审查发现并已修的 **P0**）目标簇定位原按 `first_cpu == scenemode_targets().first()` 比对——目标已
  retain 掉引导核 CPU0，其 `first()` 是 CPU1，而 little 簇首核恒为 0 → **恒不命中**，core_ctl 路径整体静默失效
  （max_cpus 钳制 / 引导核护栏 / min_cpus 压低与两处记账全是死代码）。现已改按「首核号 + 簇内核数」区间定位。
  真机验证：进 scenemode 后 `cpu0/core_ctl/max_cpus` 应为 **1**（little 簇，`min_cpus` 同为 1），退出后两值都
  回到快照，daemon.log 有对应 @A 帧（`max_cpus=1 ok`）；若仍无 corectl 打点 → 定位或节点仍不可用。
  ⑩（第二轮审查记录的边界，**刻意不做自动修复**）`min_cpus` 残留无自愈路径：崩溃残留的
  `min_cpus == max_cpus == keep` 会让 `discover()` 的残留判据（`max < min`）恒假，而 `force_online_all()`
  只修 max_cpus / online——**不能**盲写 `cluster_size`（厂商把 little 的 min_cpus 配成小于簇规模是正常省电，
  盲写会破坏它）。触发需「快照 min_cpus > keep」+「崩溃」同时成立；彻底解决需持久化「scenemode 进行中」标记。
- **[已落地 2026-09-28] 分析侧「建账」三视图**（只读日志，不改调度行为）：`dvpower.py` 的 `[5]` cap 档迁移时间线 /
  `[6]` 档位温度带 / `[7]` 按 mode 能量汇总（gpu/psi 能量加权）；`dvmain.py` 的 `[7]` 播放态帧间隔直方图
  （活跃窗 `n>=15` + 长尾率 >52ms/>100ms）。**先有账再动参数**：本项目已禁 A/B，没有这三张表就只能盲调。
- **播放态 fps 旁路的存续条件（2026-09-28 评估）**：该旁路唯一产物是离线直方图，立项理由「热压制是否伤播放」
  已在 `logd_0928-170828` 上答完（无差异）→ **只在要复核 `tuned_profiles.yaml` 的 playback 参数
  （hysteresis/down_hold）或 `tuned_thermal_floor` 时才需要它**；`:31`、`:34` 两条关闭后按仓库惯例用
  `// [PAUSED]` 注释掉旁路入口（勿删代码）。已知瑕疵：同 app 多进程交替占坑（实测同秒 `9913→18144→9913`
  反复 detach/attach + `PID 切换失败: perf_event_open failed`）→ 直方图只覆盖当前挂载的进程；候选修法是
  同包多探针（`states`/`links` 已是 HashMap，但 `current_pid` 是单值、`switch_pid` 必经 detach），**需真机验证再动**。
- **播放态直方图实际只覆盖到 200ms（2026-09-28 第二轮审查发现）**：`fps_monitor.rs` 的 `MAX_FRAME_NS=200ms`
  把 >200ms 的帧间隔**整帧丢弃**（连 `n` 都不计）→ 档号 51+ 恒空，长尾率的分子分母都不含 >200ms 的严重卡顿，
  「无长尾」只能读作「无 52~200ms 级卡顿」（命令文档与 `dvmain [7]` 输出已注明口径）。若要覆盖到档表上限
  1020ms，需给直方图**单列**一个上限，**不能**直接抬 `MAX_FRAME_NS`——它同时喂 FAS 的 `frametimes`，把极端
  间隔算进窗口会拉低 avg_fps、引发过度降档。
- **息屏期 little 被顶满 100%（2026-09-28 包实锤，能量价值最高的待查项）**：`logd_0928-170828` 的 batch1
  息屏段（05:59–09:39、09:51–12:25 等）里，daemon.log **7 次**报
  `scenemode 持续顶满性能上限（little util 100%），退回 reduce 并进入 300s 冷却`，且每次都在息屏窗口内
  （前后最近的界标是「息屏已超过阈值，切换到 scenemode」与「亮屏触发事件」）→ **息屏省电在反复自我撤销**。
  配套数字：息屏+放电行 8345（占全部放电行 40%）均值 **0.42 W**（分桶 0.1~0.6 W 连续分布）、迁移 ≈2186/s、
  `psi_cpu 23.8`；这些秒数的 `package` 是息屏前残留的 `me.weishu.kernelsu`（95% screen*on=0），
  故 `[1]` 表里 kernelsu 的 0.59 W 是「息屏 0.42 W 段 + 少量亮屏段」的混合值，**不要当亮屏空转读**。
  **下一步判据（全部可用本包现成数据）**：① 按息屏时间窗过滤 `aff*\_.log`，看 little（core 0-2）上的
`u=`与 comm 是谁；②`main\_\_.log`息屏段 tick 行的`max*util`/`cur_perf`看负载形态；③ status.csv 息屏段的`gpu_busy`判别 CPU 侧还是 GPU 侧。**候选嫌疑**：dev_record 采集本身（息屏 2.3 h 写了 19 MB main + aff 每秒
全线程采样 + 每帧 eBPF，属观测者效应）/ 第三方后台（little 上高频出现`V8_DefaultWorke`、`Chrome_ProcessL`、
`CookieMonsterCl`、`TracingMuxer`、`perfetto_hprof*` 等 Chromium 系线程名）/ ChiRi 自身巡检。
  **若指向采集开销 → 结论是「诊断期不可测待机基线」（写判读口径，不改调度）；若指向第三方 → 才是压制策略问题。**

- 亮屏 gap①：电源键亮屏无 touch 事件 → 20% 稳态 3 tick 降到地板；候选 = 亮屏复用 `on_touch()` 做一次性 400ms ceiling 窗口（待用户批准）。
- 亮屏 gap②：亮屏恢复「任一权威节点报亮即恢复」不对称判定（0926 换属性轮询后需在新机制下重评；息屏保留多数票语义）。
- FAS 无帧降级（退出 FAS 交回 CLG 或固定档保底）——会改调度行为，先定取向。
- FAS 白名单真机验证：speedmobile / lolm / 国服 sgame 首刷看 devimp fps 是否贴 target 档（玩家开 90 帧则 target_fps 调 [60,90,120]）。
- playback 参数复核（tuned*profiles.yaml 内 TODO）：hysteresis 0.06 / down_hold 150、`steady_decay*\*` 实际效果（headroom 0.85 与三簇 per_cluster 天花板已于 2026-09-28 按天花板全拆删除）。
- **播放态掉帧验证（P0-3 收尾）【已答 2026-09-28】**：`logd_0928-170828` 已按「占优簇反推基线 + 超基线长尾档」
  判读完成——中/硬档窗口（272 / 246 / 793 个活跃窗）长尾率 **0.0~0.4%**、未压制基线 **0.0%**，**基线没动**。
  **遗留缺口**：本包只有常规码率视频（占优簇 12~16ms 档），无 4K/高码率样本；**在线判据不做**（无差异 = 不值得在线化）。
- **播放态「边充边放」温度曲线【本包无样本，待采集】**：`charge ∈ {charging, full}` ∩ `mode=playback` = **0 行**
  （本包充电时段没有在播视频），下次采集需特意覆盖该场景。**嫌疑与判据**：`tuned.rs` 的
  `profile_ceil.min(cap.max(floor))` 在硬档（cap 0.40）下由 `tuned_thermal_floor`（**0.55**，见
  `src/chiri/config.rs::d_tuned_thermal_floor` + 各 `module/config/<device>/feature.yaml`）把大核抬到 0.55
  ——tuned 接管的播放态在 ≥45°C 时**可比 CLG 跑得更热**（播放态不走 CLG，此时 CLG 已压到 0.40），
  这是为「视频解码需要稳定档位」有意保留的（见 `tuned.rs [thermal_ceil]` 与 03-chiri.md「热态 tuned 响应」）。
  若边充边放时整机/壳温**持续爬升不止**，`0.55` 地板即首要嫌疑（次选：充电态额外降 cap）。
  判据 = `tuned` tick 行 `cur_max` + status.csv `batt_power_w`（按 package 均值）+ 电池温度列。
- `background_uclamp_max_pct` 现值 **25**（8550；2026-09-28 由 35 下调，A1 降权）待真机复核后台同步 / 消息推送时延；回退 = 改回 35。
- **PowerBase 档位体系与触摸升档的真机验证（2026-09-28 实现，8550 = 标准 #13 / 触摸 #20 / 上限 #25）**：① 触摸时打 `powerbase-touch-tier`（debug 级）且频率瞬抬到对应桶落点；② 连续触摸逐级升档（#13→#20→#25），已在 #25 时只续窗口、不调频；③ 窗口（`touch_break_ms`，8550 未覆盖该键 → 取代码默认 400ms）过后回落 #13；④ 非 PF SoC / 桶越界时的回退路径与不启用逐位一致。8998 已移出 CHIRI_SOC_HINTS（恢复支持需补 [powerbase] 显式段与 hints 片段，现兜底 3.0W）。
- **PowerBase 无周期调用点（既有缺口，档位体系把它放大）**：`on_load_update` 只在**负载事件分支**被调用（`mod.rs:2696`），内部转调 PowerBase 后它的返回值 `Option<Duration>`（5s 防篡改重写的剩余时间）被丢弃、未纳入 `wait`——与 `fast_lock.tick()`（`mod.rs:2243`，返回值参与动态超时）不同形。后果：eBPF 负载源一旦停摆，`on_load_update` 不再执行 → **触摸升档窗口不回落**、防篡改重写也不发生，直到 `CLG_STALE_MAX` 看门狗（`mod.rs:2245` 起）释放 cpu_governor——它内部连带释放 PowerBase（旧记的「不含 power_base」已随后端改造失效），代价是整机退回系统调频，而不是「一直锁在 #25」。修法二选一（待用户定，未动）：接收返回值纳入 `wait`（但主循环只在 load 事件分支调用，还需另开超时路径），或给 PowerBase 单开周期调用点。
- ~~feature.yaml 无 SoC→根 的字段级合并~~ **2026-10-01 已落地**：`embedded_feature_str()` 改为「根 `feature.yaml` 为基底 + `{soc}/feature.yaml` 按字段深合并覆盖」（`common.rs::merge_yaml`，map 递归、标量/序列整体替换）。**注意**：全量合并下 SoC 未写的**整段**也会从根继承——8550/8998 无 `vector` 段，现已继承根的 vector（极速档）配置，**待真机复核 8550 极速行为**。
- 线程写观测：sched_setaffinity / cpuset / uclamp 写失败无观测（event 行 kind=thread_write 未实施；相关 i18n 键已删、要用需从 git 历史找回）。
- **DOWN 实测从未做过**：写 down.chr → down-boot-halted + 60s 心跳（注意实际周期 300s）+ devimp mode=down + current_mode.chr 停在 down，对比停摆前后 governor/max_freq/cpu_boost/cpuidle/IO/sched/cpuset/在线核数。
- telemetry gpu_busy 每秒读的采集成本 A/B（GPU busy 开关对照，最便宜的「功耗为何变高」验证项）。**【2026-09-28 已评估】**：采集开销的写盘主体是 devimp（20–28 kB/s，aff 帧占 88%），logd 侧 0.13 kB/s，`gpu_busy` 属 logd 每秒读、量级远小于 devimp；**A/B 已作废**，改为长时观察（同一版本内看该列读取与 `batt_power_w` 的相关性）。已知的反例警示：GPU **快照**（读 Adreno 节点）实测 playback 1.76→2.59 W（+47%，已默认关），即"读节点唤醒设备"类探针能到 W 级，故不能凭"读一次很便宜"推断所有探针都便宜。
- V4 媒体场景 GPU 频率上限：唯一能碰 GPU 功耗大头的杠杆（云音乐 gpu_busy 33.9% / 8745 74.1%、与功耗 r=0.851），掉帧一票否决，须真机先测。
- fps_monitor 挂载失败活跃期 500ms warn 节流（建议同 PID 只告警一次、PID 变化或 FAS 重激活才重试）；fps_probe 100ms poll 事件化（eventfd/自管道 + mio，`set(false)` 已 notify）。
- devimp 真机验证：main/aff 拆分后 @A/@S 实录 + 失败注入 e{errno} + 归档双文件。
- 「先读回 scaling_max_freq 再决定写」未实施（需真机确认内核同值写是否真触发重新锁频）；FastLock/PowerBase 5s 盲写改读校验、`cpu_freq_snapshot` 复用句柄（前置：cpufreq 节点支持重复读——FastReader 依赖 seek(0)，不支持 llseek 的节点读失败）。
- clamp_change 内直接加 `smax ≤ cap 档位` 校验未做（现用 governor 侧 `clamp_apply` 跃迁事件作行为级证据替代）。
- **帕累托前沿（CLG-PF）真机验证（2026-09-28 落地，仅 8550）**：① 确认接管日志出现 `clg-pf-enabled`（指纹 `92bcedf210e6aed1`、桶数 46），稳态期打 `clg-pf-tick-log`；② 若出现 `clg-pf-fallback`（缺容量 / 桶越界 / 该簇无目标）说明表与真机口径有漂移，先查 capacity 与桶边界再谈落点；③ 落点是「只下调」，观察**同场景稳态频率是否下移而掉帧不增**（R3 口径：只做长时观察，不做开关对照）；④ 触摸窗口内 PF 已与 decay 同口径关闭（`pf_target = None`）——若在触摸/滑动场景看到落点被压回低频桶，先确认 `touch_active` 状态而不是怀疑表数据。
  - **①②已结案（2026-10-01，`devimpbin/1001-190148` A08-11/8550 两批次）**：两批次接管时均打
    `[CLG-PF] 帕累托前沿查表已启用 | 指纹=92bcedf210e6aed1 桶数=46`（与 soc.yaml 逐字一致）；
    **全程无 `clg-pf-fallback`**（`capacity-missing` / `bucket-out-of-range` / `cluster-target-missing` /
    `unknown-cluster` 均未出现）→ 0928 的「头号待办 ②：capacity-missing 全程回退」在本包**已消失、PF 全程生效**。
    历史（0928）：启用后数分钟即 `[CLG-PF] 前沿表不可用（capacity-missing），回退比例路径（只此一次）`，两段均复现
    → CLG-PF 全程未生效（落点 = 旧比例路径）；诊断已增强为带核编号（`capacity-missing(cpu=N)`）。已排除
    「soc.yaml capacity 段缺失」（FDP 用同一个 `soc_capacity_for_group` 且无 `nodata` Skip）。
    **③④ 现可观察**（PF 已真正生效）：落点只下调、同场景稳态频率是否下移而掉帧不增（R3 口径：只做长时观察）。
- **`modes` / `anticipate`（三模式取点与预判交叠带）已生成但运行期未消费**：当前只落地了「需求桶 → 三簇目标频点」。要在 CLG 里消费三模式差异与交叠带，需先把「流畅度下界（`reserve_pct`）」与「预判基准提前量（`lead_frac`）」从占位值（reduce 0 / default 0.10 / boost 0.25；`lead_frac` 0.30）定稿——重跑脚本即可覆盖（`--reserve` / `--lead-frac`），不需要改代码。
- **FDP（前沿主导的放置内核）已落地（2026-09-28），待真机验证**：`src/chiri/energy_cost.rs` 用**双边净收益**给后台/批处理 promote 选目的簇——「等算力下把这份算力放到边际 mW/Mdmips 更低的地方」；数据不手抄，直接读 `soc_frontier_policy().per_cluster.<簇>.{freq_khz, power_w}`（由 `scripts/frontier_policy.py` 从文档 §3 生成）。缺省 `fdp_enabled: false`（8550 已开 true）；`bg_promote_exclude_prime` 仍把关 prime 资格（静态表不含漏电/idle-exit，且判不出 P-b 的 >2600 Mdmips 门槛）。**降级口径（2026-09-28 用户定）**：`fdp_enabled` 为真但静态数据缺失（无 `capacity` / 无 `per_cluster` 功耗表）时，经 `energy_cost::fdp_available()` 判定后**回退原 A3 放置路径**（不把后台 promote 整段停掉）；FDP 自身的配额/净收益/选核否决仍不回退。**待验证**：① 日志 `action=fdp` 的 `net` 与真机 `batt_power_w` 走向是否同号（R3 口径：只做长时观察，不做开关对照）；② 自算例（little u=0.5 / big u=0.3 / prime u=0，d=168）在真机上是否落在同一档（模型 vs 实测 `scaling_cur_freq`）；③ 若出现「该升不升」（后台批处理变慢）先把 `fdp_cost_hysteresis_w` 调到 0.01。
  - **真机实证（A08-04/8550 两段会话，2026-09-28）**：`action=fdp` 帧 91 条**全部 Skip**（`hysteresis` 90 + `dstfreq` 1），**零 Move 帧、`pin bg_busy` 零出现**——该场景下 FDP 未批准任何迁移（这些时刻旧 A3 路径会 promote，属可观测的行为变化）。保守来源 = 成本包络口径（见下条「不同底」）与 `fdp_cost_hysteresis_w` 阈值；**阈值先不动**，按 ③ 收集「后台批处理是否变慢」的证据再定。另：`fdp_ready` 判据已与准入同源（防「数据缺失时 FDP 与 big 饱和保护同失」，2026-09-28 修）。
  - **真机基线（2026-10-01，`devimpbin/1001-190148` A08-11/8550 两批次）**：`action=fdp` 共 **270 帧**，
    Skip **268**（`hysteresis` 157 + `dstfreq` 111）、Move **2**（`little->big f_src=1785`，`move_group fdp ok`）
    → **动作率基线 2/270 = 0.74%**（相对 0928 的 0/91 已有首个 Move）。**口径注意**：本包统计是**旧脚本产物**
    （`aff.txt`），`dstfreq` 111 里含「同一候选被多重拦截只报一个」的老口径高估（见 T1a 修，`energy_cost.rs`
    出参已改为 blocked 位组合 `quota+dstfreq` 等，下包起可分离）；`net` 与 `batt_power_w` 同号性仍待 ①。
- **F3 / F4 待真机**：F3 = fps 探针 attach 累计失败数（已补打点 `fps-monitor-attach-stats`）需真机看真实失败率；F4 = FAS 延迟退出时长（`deactivate_delay_secs`）**本轮刻意未改数值**——缩短会提高「短暂离开（弹窗/画中画/后台配音）被误判为真退出」的概率并引发 activate/deactivate 抖动，应先测「离开→回来」时长分布。
- **T11 三模式取点待真机复核（8550）**：`reduce.steady_decay_target_ratio = 0.80` 是刻意低于 default 的 0.90（天花板拆除后 reduce 与 default 其余参数已很接近，靠取点保住省电档差异）；`boost` 的 `steady_decay_enabled: false` 是「对功耗宽松档不主动下探」的选择。判据 = 省电档滑动手感 + 同场景稳态频率；省电档过钝就把该行删掉回到缺省 0.90。**8475/8998/根兜底的 boost 档同批也已显式 `steady_decay_enabled: false`**（同判据：`up_threshold` 0.55~0.65 时 T 落进常态负载区、削掉响应优势），这三份的 reduce/default 仍走缺省开启，待真机复核。
- **FDP 源簇让出收益「不同底」（2026-09-28 审查发现，已在代码留 TODO）**：`src/chiri/energy_cost.rs` 的源簇一对值 = 现状取**实测**、之后取**模型**，而目的簇侧已用 `f_dst_now`/`f_dst_after` 的 min/max 包络把两侧锚到同一底；源簇侧缺模型现状值 → 当 little 实测高于模型（QoS / 热 / 别的线程抬频）时 `min` 仍会**放大让出收益**，可能推动不该有的迁移。修法 = 对称取 `f_src_now = min(实测, 模型)`；动手前先用真机 `action=fdp` 日志的 `net` 与 `batt_power_w` 走向对照（R3 口径：只做长时观察）。

### WebUI / 工具

- 视觉走查人工项（本机无 agent-browser/无头浏览器）：0926 screen_off_value / screen_prop 交互、analyze/ 图表页。
- 导出轮询改阻塞 exec 未做（需真机验证长命令回调）；mock-shell readMany 整体 failed 的已知缺口保持。
- state.svelte.ts 零单测；export 轮询 4 分钟不可取消。
- 归档新格式真机验证：logd/ 产出 `.tar.lz4`（至今仅合成夹具）；WebUI 导出 `.tar.gz` 由内置 `chiri gzip` 产出（退出码 0）。
- aff Busy 释放修复待验：需一份凉机、放电、非 boost、压力有升有落的包——压力解除后 `pin=1/home=-1` 计数应回落到零。

### 配置 / 文档 / 杂项

- updateInformation/changelog.md 空文件，是否填 Alpha06-12→14 汇总待确认（HotUpdateInfo.md 亦空）。
- 完整安装会清掉 down.chr / rhine.chr（用户停摆/实验室状态丢失，不在「保留配置」范围）；实验室还原不原子（meta 写失败半还原可重试）；实验室与 sync_meta_snapshot 都写 meta.yaml 理论交错未加锁。
- （2026-09-17 提出，**先核实**）down.rs 非 UTF-8 读失败当缺失 → 模板覆盖会冲掉用户手改；BOM(U+FEFF) 两侧 trim 行为不一致；monitor `dynamic_enabled=false` 直返 global_mode 绕过 fas/akmode 门控；看门狗释放后 vector/特调无周期重建。
- panic 收尾缺 governor/gpu/lab 清理（靠重建幂等收敛，P2 可接受）；kgsl-3d0 与 devfreq 扫描可能收双节点（重复写无害）。
- meta 两个写者共用固定 tmp 名（撕裂需两线程同时写）；rhine on_startup→DirWatcher 毫秒级窗口写入丢失；meta 被改非法后锁定期不自动 reassert（等下次改文件/重启）。
- OPlus 电池：2S/2P 判定缺一次性标准节点读值（已加 telemetry-raw-snapshot 兜底）；bcc_parms 下标 12/13 疑似重复电流字段待核对；2S 机型自检：`dumpsys battery` voltage 3841mV 级=单节域（平均对）、7680mV 级=2S 直串（改回求和）。
- 文档修订（用户定暂缓，顺手勿改）：README FAS 措辞与断链（README:28 链 `updateWithoutRestart.md` 不存在；`updateWith.md` 已移 `.archive/mdocs/`，恢复链接或删除）；mdocs/socList.md 滞后（只列 8550，实际 8550/8475/8998 三份）；`src/chiri/mod.rs:184`「多实例」注释过时（实际单实例）；affinity.rs 头注释 0.2 vs `PINNED_WEIGHT` 0.4；soc.yaml capacity 注释「厂商板级 DTS 覆盖」建议改弱为「来源待证」。（原列的「feature.yaml『整簇断电』注释 vs 实际只下线 prime」已随 2026-09-28 的离核策略改动一并消解——注释与行为现在都是「下线 little」。）
