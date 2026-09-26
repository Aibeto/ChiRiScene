## [todos] 未竟事项台账（2026-09-26 汇总）

> 历史计划/日志归档后的**唯一 TODO 权威**。开工前先查本文件；完成一项删一项。2026-09-17 审查提出的旧项标注「先核实」——动手前先确认现行代码是否已修。

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
- playback 参数复核（tuned_profiles.yaml 内 TODO）：headroom 0.85、hysteresis 0.06 / down_hold 150、big ceil 0.75。
- `touch_boost_tiers: 3→2` 真机 A/B（8550，滑动/打字流畅度不行回 3 档）；`clamp_heavy` 恒钳 A/B（功耗 + 性能，与 T8 同源）。
- `background_uclamp_max_pct` 50→35 待复核。
- PowerBase 触摸突破恒 false（CLG 的 AtomicTouchState 私有，需另备共享触摸标志）；8998 已移出 CHIRI_SOC_HINTS（恢复支持需补 [powerbase] 显式段与 hints 片段，现兜底 3.0W）。
- 线程写观测：sched_setaffinity / cpuset / uclamp 写失败无观测（event 行 kind=thread_write 未实施；相关 i18n 键已删、要用需从 git 历史找回）。
- **DOWN 实测从未做过**：写 down.chr → down-boot-halted + 60s 心跳（注意实际周期 300s）+ devimp mode=down + current_mode.chr 停在 down，对比停摆前后 governor/max_freq/cpu_boost/cpuidle/IO/sched/cpuset/在线核数。
- telemetry gpu_busy 每秒读的采集成本 A/B（GPU busy 开关对照，最便宜的「功耗为何变高」验证项）。
- V4 媒体场景 GPU 频率上限：唯一能碰 GPU 功耗大头的杠杆（云音乐 gpu_busy 33.9% / 8745 74.1%、与功耗 r=0.851），掉帧一票否决，须真机先测。
- fps_monitor 挂载失败活跃期 500ms warn 节流（建议同 PID 只告警一次、PID 变化或 FAS 重激活才重试）；fps_probe 100ms poll 事件化（eventfd/自管道 + mio，`set(false)` 已 notify）。
- devimp 真机验证：main/aff 拆分后 @A/@S 实录 + 失败注入 e{errno} + 归档双文件。
- 「先读回 scaling_max_freq 再决定写」未实施（需真机确认内核同值写是否真触发重新锁频）；FastLock/PowerBase 5s 盲写改读校验、`cpu_freq_snapshot` 复用句柄（前置：cpufreq 节点支持重复读——FastReader 依赖 seek(0)，不支持 llseek 的节点读失败）。
- clamp_change 内直接加 `smax ≤ cap 档位` 校验未做（现用 governor 侧 `clamp_apply` 跃迁事件作行为级证据替代）。

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
