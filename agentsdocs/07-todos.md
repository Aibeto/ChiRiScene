## [todos] 未竟事项台账（2026-09-26 汇总）

> 历史计划/日志归档后的**唯一 TODO 权威**。开工前先查本文件；完成一项删一项。2026-09-17 审查提出的旧项标注「先核实」——动手前先确认现行代码是否已修。
>
> **口径变更（2026-09-28）**：R3「**永不再新增 A/B 测试**」生效（详见 `04-hard-lessons.md` 的 `[hard]` 首条）。下列以 A/B 为手段的条目**不再按 A/B 执行**，改为「离线重放 + 同版本观察」：`:9` [C3] 构建剖面 A/B、`:12` 真机 A/B 整体未做、`:18` T7 uclamp.max=85 A/B 与 T8 clamp_heavy 恒钳 A/B、`:39` touch_boost_tiers A/B 与 clamp_heavy A/B、`:44` telemetry gpu_busy 采集成本 A/B——逐条待裁决去留，其中涉及"采集开关对照"的直接作废。

### 性能与探针（perf-report backlog）

- [K3] 摘除纯遥测探针需 meta 开关：wakeups/migrations 被 WebUI 消费（status.csv 18/19 列 + devimp snap + telemetry-summary），属数据面变更——需显式开关 + 关时写 `-` + WebUI null 展示同步。
- [C1] fps_monitor / cpu_monitor 两 tokio runtime 改 current_thread（**必须 enable_time**；Cargo.toml features 收窄 `rt-multi-thread`→`rt`）；收益 ≈ 省 14 空闲线程，CPU 收益≈0。已定稿未实施。
- [A3] devimp join 缓冲复用；[A7] i18n t() 预格式化缓存（硬约束：load_language 切语言必须整体重建）；[C3] 构建剖面 A/B（opt-level z/s/3 + thin LTO，比体积与 8550 实测功耗）；[C5] regex 跨热重载缓存（每次热重载全量重编译）。
- [E1] app_detect cgroup tasks 内容比对短路（定级「最推荐先做」、行为等价、稳态每轮 N 次 /proc 读降为 1 次）；[E2] uevent 扩展（cpu hotplug / power_supply，前置=真机验证子系统覆盖）；[E3] logdr events buffer（liblog 协议 150-250 行 + SELinux 可达性验证）；[E4] pid_watcher 并入 app_detect（时延 500ms→1.5s 取舍待拍板）。
- 自测量基线未做：daemon utime/stime 1s 采样进 status.csv 末列（新列一律追加末尾）+ cargo bench 三纯函数。
- 真机 A/B 整体未做（perf 16 项优化 + K1/K2 eBPF 同场景 status.csv 逐核 util 对比）；K1/K2 CI 构建确认（本机 stub 编译盲区，「CI 通过才算落地」）。
- U1 真机验证：`rm daemon.log` / `rm -rf logs/` 后 ≤8KB 写入窗口内自愈重建；写满 50MB 轮转后新文件续写、备份链完整。
- **已定案不做（勿重评）**：U3 cpu_monitor 侧、A5b、panic=abort、换 hasher/parking_lot、热路径值缓存、事件化否决四项（cgroup.events poll / PSI / thermal uevent / timerfd 全量合并）、换语言与双进程拆分。

### 内核取证（详见 06-kernel.md）

- **T1** DT 反编译（capacity 出处 / OPP 电压 / trip / cooling-map / idle 厂商改动全实证）——最大单点缺口；**T2** thermal zone 全清单 + cpu_temp 两路定案；**T3** cpuidle state 全量抓取；**T4** 真热压时段 dcvsh/lmh 采样（常规负载已排除）；**T5** msm_performance / Horae 活跃性（部分定案：msmp 不可读、horae=1）；**T6** ftrace（trace_sched_task_util / trace_dcvsh_freq）；**T7** uclamp.max=85 A/B → P1-5 tg 层钳制立项与否；**T8** clamp_heavy 恒钳 A/B（cap85 2.78 vs 3.18W 基线）；**T9** input boost 实际挂载点；**T10** 热压场景 FAS verify 行为。
- 树外 oplus_cpu 专项（frame_boost / eas_opt / OMRG / cpufreq_health 的 uclamp/亲和写入路径）。
- 次级未确认：OMRG/kalama 启用与否、msm_lmh_dcvs/lmh.c 是否 probe、厂商 defconfig cdev 清单、`sched_lpm_disallowed_time` 定义文件、WALT busy-hold 对应 waltgov 行号、cpuset 收紧被 WALT 恢复宽掩码的实测。

### 调度 / 机制验证

- 亮屏 gap①：电源键亮屏无 touch 事件 → 20% 稳态 3 tick 降到地板；候选 = 亮屏复用 `on_touch()` 做一次性 400ms ceiling 窗口（待用户批准）。
- 亮屏 gap②：亮屏恢复「任一权威节点报亮即恢复」不对称判定（0926 换属性轮询后需在新机制下重评；息屏保留多数票语义）。
- FAS 无帧降级（退出 FAS 交回 CLG 或固定档保底）——会改调度行为，先定取向。
- FAS 白名单真机验证：speedmobile / lolm / 国服 sgame 首刷看 devimp fps 是否贴 target 档（玩家开 90 帧则 target_fps 调 [60,90,120]）。
- playback 参数复核（tuned*profiles.yaml 内 TODO）：hysteresis 0.06 / down_hold 150、`steady_decay*\*` 实际效果（headroom 0.85 与三簇 per_cluster 天花板已于 2026-09-28 按天花板全拆删除）。
- **播放态掉帧验证（P0-3 收尾）**：动态帧信号已落地（2026-09-27：`event` 行 `decision=playback_fps` 直方图），
  拿到真机包后按「占优簇反推基线 + 超基线长尾档」判读——中档 0.60 / 硬档窗口与播放态重叠时若基线没动，
  即可把 `tuned_thermal_floor` 从 0.55 下调试探；若出现长尾档即证明 0.55 的下限是必要的（并考虑在线判据）。
- **播放态「边充边放」温度曲线复核（P0-3 收尾，2026-09-27 追加）**：`tuned.rs:54` `profile_ceil.min(cap.max(floor))`
  在硬档（cap 0.40）下由 `tuned_thermal_floor`（0.55，定义于 `src/chiri/config.rs::d_tuned_thermal_floor` +
  各 `module/config/<device>/feature.yaml`）把大核抬到 0.55，即 tuned 接管的播放态在 ≥45°C 时**可比 CLG 跑得更热**
  （播放态不走 CLG，CLG 此时已压到 0.40）——这是为「视频解码需要稳定档位」有意保留的，见 `tuned.rs [thermal_ceil]`
  与 `agentsdocs/03-chiri.md` 的「热态 tuned 响应」条。待验证：**边充边放场景**下看整机/壳温曲线是否仍持续爬升；
  若爬升不止，`0.55` 这个地板即首要嫌疑（次选：`tuned_resp_enabled` 是否需要在充电态额外降 cap）。
  判据同上行——`tuned` tick 行 `cur_max` + status.csv `batt_power_w`（按 package 均值）+ 电池温度列。
- `touch_boost_tiers: 3→2` 真机 A/B（8550，滑动/打字流畅度不行回 3 档）；`clamp_heavy` 恒钳 A/B（功耗 + 性能，与 T8 同源）。
- `background_uclamp_max_pct` 现值 **25**（8550；2026-09-28 由 35 下调，A1 降权）待真机复核后台同步 / 消息推送时延；回退 = 改回 35。
- PowerBase 触摸突破恒 false（CLG 的 AtomicTouchState 私有，需另备共享触摸标志）；8998 已移出 CHIRI_SOC_HINTS（恢复支持需补 [powerbase] 显式段与 hints 片段，现兜底 3.0W）。
- 线程写观测：sched_setaffinity / cpuset / uclamp 写失败无观测（event 行 kind=thread_write 未实施；相关 i18n 键已删、要用需从 git 历史找回）。
- **DOWN 实测从未做过**：写 down.chr → down-boot-halted + 60s 心跳（注意实际周期 300s）+ devimp mode=down + current_mode.chr 停在 down，对比停摆前后 governor/max_freq/cpu_boost/cpuidle/IO/sched/cpuset/在线核数。
- telemetry gpu_busy 每秒读的采集成本 A/B（GPU busy 开关对照，最便宜的「功耗为何变高」验证项）。**【2026-09-28 已评估】**：采集开销的写盘主体是 devimp（20–28 kB/s，aff 帧占 88%），logd 侧 0.13 kB/s，`gpu_busy` 属 logd 每秒读、量级远小于 devimp；**A/B 已作废**，改为长时观察（同一版本内看该列读取与 `batt_power_w` 的相关性）。已知的反例警示：GPU **快照**（读 Adreno 节点）实测 playback 1.76→2.59 W（+47%，已默认关），即"读节点唤醒设备"类探针能到 W 级，故不能凭"读一次很便宜"推断所有探针都便宜。
- V4 媒体场景 GPU 频率上限：唯一能碰 GPU 功耗大头的杠杆（云音乐 gpu_busy 33.9% / 8745 74.1%、与功耗 r=0.851），掉帧一票否决，须真机先测。
- fps_monitor 挂载失败活跃期 500ms warn 节流（建议同 PID 只告警一次、PID 变化或 FAS 重激活才重试）；fps_probe 100ms poll 事件化（eventfd/自管道 + mio，`set(false)` 已 notify）。
- devimp 真机验证：main/aff 拆分后 @A/@S 实录 + 失败注入 e{errno} + 归档双文件。
- 「先读回 scaling_max_freq 再决定写」未实施（需真机确认内核同值写是否真触发重新锁频）；FastLock/PowerBase 5s 盲写改读校验、`cpu_freq_snapshot` 复用句柄（前置：cpufreq 节点支持重复读——FastReader 依赖 seek(0)，不支持 llseek 的节点读失败）。
- clamp_change 内直接加 `smax ≤ cap 档位` 校验未做（现用 governor 侧 `clamp_apply` 跃迁事件作行为级证据替代）。
- **帕累托前沿（CLG-PF）真机验证（2026-09-28 落地，仅 8550）**：① 确认接管日志出现 `clg-pf-enabled`（指纹 `92bcedf210e6aed1`、桶数 46），稳态期打 `clg-pf-tick-log`；② 若出现 `clg-pf-fallback`（缺容量 / 桶越界 / 该簇无目标）说明表与真机口径有漂移，先查 capacity 与桶边界再谈落点；③ 落点是「只下调」，观察**同场景稳态频率是否下移而掉帧不增**（R3 口径：只做长时观察，不做开关对照）；④ 触摸窗口内 PF 已与 decay 同口径关闭（`pf_target = None`）——若在触摸/滑动场景看到落点被压回低频桶，先确认 `touch_active` 状态而不是怀疑表数据。
- **`modes` / `anticipate`（三模式取点与预判交叠带）已生成但运行期未消费**：当前只落地了「需求桶 → 三簇目标频点」。要在 CLG 里消费三模式差异与交叠带，需先把「流畅度下界（`reserve_pct`）」与「预判基准提前量（`lead_frac`）」从占位值（reduce 0 / default 0.10 / boost 0.25；`lead_frac` 0.30）定稿——重跑脚本即可覆盖（`--reserve` / `--lead-frac`），不需要改代码。
- **FDP（前沿主导的放置内核）已落地（2026-09-28），待真机验证**：`src/chiri/energy_cost.rs` 用**双边净收益**给后台/批处理 promote 选目的簇——「等算力下把这份算力放到边际 mW/Mdmips 更低的地方」；数据不手抄，直接读 `soc_frontier_policy().per_cluster.<簇>.{freq_khz, power_w}`（由 `scripts/frontier_policy.py` 从文档 §3 生成）。缺省 `fdp_enabled: false`（8550 已开 true）；`bg_promote_exclude_prime` 仍把关 prime 资格（静态表不含漏电/idle-exit，且判不出 P-b 的 >2600 Mdmips 门槛）。**降级口径（2026-09-28 用户定）**：`fdp_enabled` 为真但静态数据缺失（无 `capacity` / 无 `per_cluster` 功耗表）时，经 `energy_cost::fdp_available()` 判定后**回退原 A3 放置路径**（不把后台 promote 整段停掉）；FDP 自身的配额/净收益/选核否决仍不回退。**待验证**：① 日志 `action=fdp` 的 `net` 与真机 `batt_power_w` 走向是否同号（R3 口径：只做长时观察，不做开关对照）；② 自算例（little u=0.5 / big u=0.3 / prime u=0，d=168）在真机上是否落在同一档（模型 vs 实测 `scaling_cur_freq`）；③ 若出现「该升不升」（后台批处理变慢）先把 `fdp_cost_hysteresis_w` 调到 0.01。
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

- **8745 配置疑案**：`module/config/8745/` 因 CHIRI_SOC_HINTS（现 ["8550","8475"]）不含 8745 **永不生效**，且与 8475 的 feature.yaml 内容不同（8745 缺 affinity/corectl/thermal 段）——合并方向待拍板（拍错毁调优）。
- 版本号两套命名（module.prop Alpha*-Canary* vs Cargo.toml 2.0.2）是否统一留用户。
- updateInformation/changelog.md 空文件，是否填 Alpha06-12→14 汇总待确认（HotUpdateInfo.md 亦空）。
- 完整安装会清掉 down.chr / rhine.chr（用户停摆/实验室状态丢失，不在「保留配置」范围）；实验室还原不原子（meta 写失败半还原可重试）；实验室与 sync_meta_snapshot 都写 meta.yaml 理论交错未加锁。
- （2026-09-17 提出，**先核实**）down.rs 非 UTF-8 读失败当缺失 → 模板覆盖会冲掉用户手改；BOM(U+FEFF) 两侧 trim 行为不一致；monitor `dynamic_enabled=false` 直返 global_mode 绕过 fas/akmode 门控；看门狗释放后 vector/特调无周期重建。
- panic 收尾缺 governor/gpu/lab 清理（靠重建幂等收敛，P2 可接受）；kgsl-3d0 与 devfreq 扫描可能收双节点（重复写无害）。
- meta 两个写者共用固定 tmp 名（撕裂需两线程同时写）；rhine on_startup→DirWatcher 毫秒级窗口写入丢失；meta 被改非法后锁定期不自动 reassert（等下次改文件/重启）。
- OPlus 电池：2S/2P 判定缺一次性标准节点读值（已加 telemetry-raw-snapshot 兜底）；bcc_parms 下标 12/13 疑似重复电流字段待核对；2S 机型自检：`dumpsys battery` voltage 3841mV 级=单节域（平均对）、7680mV 级=2S 直串（改回求和）。
- 文档修订（用户定暂缓，顺手勿改）：README FAS 措辞与断链（README:28 链 `updateWithoutRestart.md` 不存在；`updateWith.md` 已移 `.archive/mdocs/`，恢复链接或删除）；mdocs/socList.md 滞后（只列 8550，实际 8550/8475/8998 三份）；`src/chiri/mod.rs:184`「多实例」注释过时（实际单实例）；affinity.rs 头注释 0.2 vs `PINNED_WEIGHT` 0.4；feature.yaml「整簇断电」注释 vs 实际只下线 prime；soc.yaml capacity 注释「厂商板级 DTS 覆盖」建议改弱为「来源待证」。
