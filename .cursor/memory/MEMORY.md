# ChiRi Cursor 环境记忆

> 按环境分文件夹写入：Cursor 的产出只写 `.cursor/`。历史长期事实（工具链口径、守护进程契约）
> 在 `.codebuddy/memory/MEMORY.md`，只读，开工必读。

## 协作约定

- 一切 AI 产出文件（计划、评估、报告等）只写项目文件夹内：本环境文档放 `.cursor/docs/`、记忆放 `.cursor/memory/`；
  工具自动写到项目外的副本（如 Cursor plans 目录）复制回项目后删除原件。
- **【铁律】任何文件不得落在项目文件夹之外**（2026-09-26 用户严令，违者零容忍）：
  CreatePlan 会把计划写到 `C:\Users\Aibeto Zhu\.cursor\plans\`——**每次创建计划后必须立即**：
  ① 原文复制到 `.cursor/docs/plans/<日期>-<名称>.plan.md`；② 删除 plans 目录原件；③ 确认项目外无残留。
  本日已违例一次（两份计划滞留项目外被用户抓到），此流程刻死，不依赖回忆。
- 架构约定、契约数字、命令口径变更同步 `agentsdocs/` 与本文件；具体改动经过写 `.cursor/memory/YYYY-MM-DD.md`。
- CI 跳过口径：`build.yml` 的 `check-skip` 闸——提交信息含 `skip ci`（不分大小写、无需方括号）即跳过该次构建；
  `[skip ci]` 等方括号标记另走 GitHub 原生整 run 跳过；`workflow_dispatch` 手动触发恒运行；提交信息仅提及该字样也会命中。
- WebUI type-check 口径（2026-09-24）：`npm run type-check` = `svelte-check --tsgo`；TypeScript 7 经 npm 别名
  `@typescript/native` 接入，根 typescript 保持 6 系列（svelte-check 对等依赖只认 5/6 系列）；别名与 `--tsgo`
  成对，缺任一 svelte-check 直接报错退出。

## 契约要点

- FAS 所有权/叠加实现（2026-10-06）：前台检测与游戏 owner 分离；帧事件采集时携带 PID/generation，摘挂 drain 与 CLOCK_MONOTONIC 截止界标隔离旧 ring/队列。失焦期间暂停 FPS 反馈与目标推断，按真实 policy CPU 集合负载控制，不冻结旧高频；恢复清状态并预热 12 个有效新帧，无帧 500ms 回退 load-only，热保护仍由负载驱动。窗口核验单 worker、3s 最小间隔、800ms 超时、512KiB cap、5s 证据 TTL；Unknown 不无限续期，VisibleUnfocused 需 mObscured=false。详见 agentsdocs/03-chiri.md；本机 Cargo 目标验证遇到 registry/rustup 权限与缓存阻塞，portable 测试不能代替 Android/eBPF 通过证据。
- devimp 统计（2026-10-06）：文件/批次/root + --since 归档戳选择；样本等效 Wh 与有效短区间积分分开，跨 type/charge/mode/package/screen 边界不外推，长缺口不归因为 suspend；总口与动态模型扣同子集各自基线，负残差保留，模型不是物理下界。FPS 全空不证明未 FAS；版本按解析身份去重，缺失占位不冲突。命令正文与 agentsdocs/05-maint.md 已同步。
- daemon 自耗专项：唯一有效计划为 `.cursor/plans/2026-10-07-daemon-selfcost-implementation-plan.md`（合并版）。不调大 aff_secs、不主动关闭诊断作对照、不降采样、不摘 K3；30~40ms 是进程 CPU 配对估计，不是墙钟阻塞，日志覆盖不证明开关状态，所有测量调用入账、50ms 只限告警。2026-10-09 `diag_build_cost.rs` 已落：`selfcost-build/1 parent=build` 四类子块（get_thread_tids/tid_stat 含解析/snapshot_procs/sched_affinity）+ residual，失败也 count；CPU/墙钟各用有效分母，缺时钟/负残差/溢出不假零，父子绝不可相加。residual 含组装/渲染及未归子块 observer，端点读钟含部分自身成本；60s/关闭尾窗复用 selfcost，会话独占，窗口 reset 保留 errors、session reset 全清。worker 默认关闭；独立审查完成，无剩余阻断发现，编译/运行验证仍受阻，真机收益待验证。
- 本轮验证限制（主线回传）：Android cargo check 已尝试，Compiling chiri 后因 build.rs eBPF 的 -Z 仅 nightly 允许、当前 stable 失败，无 Checking chiri；纯模块 rustc --test 因项目内 TMPDIR 只读失败，测试未执行。两真实模块已由主线加入 scripts/tests/rust_regression.rs，尚未重跑；ReadLints totalFiles=0 不构成有效验证。mod.rs 单测 aff_cost_record 缺第 7 参已补 None 并经同一独立审查员复核有效；独立审查完成，无剩余阻断发现，静态审查不等于测试通过。
- 亲和重试缓存（2026-10-09，代码已落、验证待定）：normal_busy/normal_press 真实 EINVAL 按 starttime + 目标 CPU 列表退避 4/8/16/32s 封顶，成功/目标/身份变化清，释放/reload 失效；仅 restricted cpu.uclamp.max 真实 ENOENT 负缓存 600s，目标值与 max 恢复不破缓存，background 60s 守卫不变、到期可绕过同值 guard。mod.rs 两处真实 config_dirty/ConfigReload 入口 invalidate_retry_caches，不是 2s apply 清缓存。

- selfcost 一致性契约：配置指纹仅排序 map，保留列表优先顺序；配置、指纹及代际在配置写锁内发布，采样参数与身份在对应读锁内冻结，build 前释放。worker 故障关闭发送入口后排空至通道断开，失败提交等待 join 后回退，不再超时放行新差分帧；永久 IO 阻塞仍会导致等待无界，worker 保持默认关闭。修订未编译测试，详见 agentsdocs/02-convention.md。

- **SoC 硬件基线 soc.yaml（2026-09-26）**：`module/config/{soc}/soc.yaml`（topology/capacity/freq_khz/idle/dpc），编译期嵌入，sysfs 兜底与校准基准、非调优输入；`chiri_core_ranges`/capacity/freq 兜底已接，硬编码 match 保留为最后兜底。口径与坑见 agentsdocs/02-convention.md 配置嵌入条；审计结论（其余 rs 常数不外挂）见 `.cursor/docs/2026-09-26-soc-hardcode-audit.md`。
- logd/ 归档预算清理（2026-09-24 起）：`enforce_logd_limit` 按**归档批次原子删**——
  `<ts>.tar` 与 `devimp_<ts>.tar` 同进退、最新批次永不删，最新批次 ≥200MB 时收到 256MB 即停。
  背景：超大 devimp 归档（114MB）曾把同批 daemon.log/status.csv 归档挤掉（logd_0924-173105 包）。
  目录预算同日扩容：`LOGD_MAX_BYTES` / `DEVIMP_DIR_MAX_BYTES` 128→256MB、TARGET 96→200MB。
  详见 agentsdocs/02-convention.md 归档条与「被挤掉」历史坑。
- **归档格式（2026-09-25，契约变更；同日改内置库）**：启动归档两目录产物均为 **`logd/<ts>.tar.lz4`** 与
  **`logd/devimp_<ts>.tar.lz4`**——pack.sh archive 只打无压缩 tar，**Rust 内置 `lz4_flex` 流式压缩**
  （不依赖设备 lz4 二进制；仅 lz4 落盘 I/O 失败才回落 `.tar`）。导出包 `logd_*.tar.gz` 的 gzip 主选走
  **模块二进制工具模式 `core/bin/chiri gzip <file>`**（main.rs [toolbox] 分发，flate2 纯 Rust；系统
  gzip/busybox 兜底）。**分析侧禁止按扩展名判断内层形态**——一律嗅探 magic。实测 logd_0925-045336
  内层仍为纯 `.tar`（旧 lz4-二进制回落分支的产物），**内置库分支产出的真机 `.tar.lz4` 待下次归档验证**。
  解压用 `scripts/devimp/dvlz4.py`。

- **devimp 分析走预置脚本（2026-09-25）**：一律用 `python scripts/devimp/dvrun.py <logd_*.tar.gz>`
  （或 `scripts\devimp-run.cmd`）一条命令跑完 extract/analyze/main/aff/status，产出落 `devimpbin/<MMDD-HHMMSS>/`
  的 `inventory.txt` / `analyze.txt` / `main.txt` / `aff.txt` / `status.txt` / `report.md`。
  **禁止再在 `devimpbin/` 现场写一次性 probe py**——该目录分析完即清，工作全丢（2026-09-25 前的实际损失）。
  单个探针可 `--only <阶段>` 或直接跑 `dvmain.py`/`dvaff.py`/`dvstatus.py` 复用。

- 日志打包门限（128MB，**未变**）的看门狗判定（2026-09-24 重做）：pidfile 铁证
  （`logs/watchdog.pid` == getppid()）∪ 脱管 shell（comm 属 shell 家族且祖字段 ==1）；
  达门限被抑制时打点 `logger-log-restart-suppressed` warn，不再静默清零（旧 comm∈{sh,mksh}
  白名单会把调试直跑误判成有监督、把 busybox/ash 父或 /proc 读失败误判成无监督）。

- FAS 防篡改强制重写（2026-09-24 改）：`freq_force_reapply_interval` 由「帧」改「**秒**」（默认 30、
  `normalize()` 钳 ≥1，旧实现 0 会 `% 0` panic）；`force_reapply()` 去掉两次无用的 `re_unmount()`
  （只 `umount2` 不重开 fd、对写入零贡献；`verify_prev_freq` 那次保留）。收益：120fps 下
  强制重写路径由 24 写 + 24 `umount2`/s 降到 0.2 写/s、0 `umount2`，并与刷新率解耦。
  `mdocs/TechAnalyze.md`、`README.en.md` 的单位说明已同步为秒。

- aff 减量（2026-09-24，四文件）：① `@S` 线程快照改**差分帧**——前台线程与被管条目
  （`pinned`）每帧全量，长尾只在 `u/core/home/pin` 变化时落行、每 30 帧全量刷新一次
  （**缺失行 = 与上帧相同**，长尾 `u` 为 ≤30s 均值）；每帧 t 行 767→约 374、stat 读 −52%、
  aff\_ 121MB/42min → ~62MB（128MB 门限重启周期 42min → ~1.2h）。② 清理类动作汇总为
  `<场景>_bulk`（`value`=条数），逐条 `bind_release` 只剩真有内核动作的条目。③ `t` 行 pid 由
  `/proc/<tid>/status` Tgid 补全（旧包 54% `pid=0`）。④ `threads` 表新增 `is_fg`，四处
  「`pid>0` 当前台哨兵」的判据改看它（等价重构，调度决策未变）。⑤ bg uclamp 值守卫：
  同值不写、60s 强制再断言（~1 次/s → 1 次/60s），`uclamp` 帧只在真写时产生。
  判读口径见 `.cursor/commands/devimp-log-analysis.md` 已知坑 13/14。

- 亲和组绑定释放必须覆盖「压力窗口外」（2026-09-25）：前台线程的 `group_bind`（Key/Busy 共用枚举）
  释放分支条件 = `group_bind != GroupBind::None` + `!key_pressure` 守卫（原 `== GroupBind::Key`
  只兜 Key 绑定）。原因：Busy 绑定的空闲回落只写在 `promote_busy_foreground` 内，而该函数在
  `key_pressure` 解除后整段不再被调用 → 压力一落，Busy 线程就带着 big∪prime 收窄掩码滞留
  （logd*0925 实测 95min 会话末帧仍有 1115 条 `pin=1/home=-1` 未释放，占 aff* 116MB/128MB）。
  **结论：新增组绑定触发条件时，必须同步检查释放侧是否有窗口外兜底。**

- devimp 目录懒创建与零写入（2026-09-24）：`devimp/` **唯一创建者 = `main_open`/`aff_open`**（两者都在
  `diag_active()` 门控内）；`diag_prepare` 目录不存在即早退、启动归档不预建、短会话清空后连空目录
  `remove_dir`——**dev_record 关闭时不产生任何数据（含空目录）**。`set_diag_package` 加 `diag_active()`
  门控（关时零锁零分配；重开后下一秒仍按包名切 main\_ 文件）。`current_mode.chr` 5s 自愈改 `ModeFile`
  （记账 + 磁盘内容双重比对，跳过时 5 syscall → 1 read；`remove()` 清记账防文件空窗）。
- 前台 PID 广播改推送（2026-09-24）：`monitor/mod.rs` 的 `pid_watcher`（500ms 轮询原子量）已删，
  改在 `app_detect` 的 `set_current_package` 生效点就地 `pid_tx.send`（发送条件与原线程逐位一致：
  pid 变化且 >0）；cpu_monitor / fps_monitor 消费同一 watch 通道，稳态唤醒 2 次/s → 0。同批监控侧另有屏幕节点清单缓存 **10 分钟 TTL**（2026-09-24 已落，`monitor/screen_detect.rs` `SCREEN_NODES_TTL=600s`）：命中零分配零 syscall，TTL 到期或全不可读即重枚举，投票/每轮新鲜读口径不变。

- FAS 激活信号（2026-09-24）：`monitor::FasSignal`（`Mutex<()>` 判谓词 + `Condvar`，
  **`set` 的 store 与 `notify_all` 同锁**防丢唤醒）取代裸 `Arc<AtomicBool>`；生产方
  `fas_manager` 三处（activate/续期/`deactivate_active`），消费方 `fps_probe` 待机与
  `mod.rs` 首激活门控——稳态唤醒 2 次/s → 0。**勿退回裸原子量轮询**：FAS 会在前台 PID
  未变时重新激活（息屏释放、冷却结束），只等 PID 会漏唤醒。

- **播放态帧信号（2026-09-27 契约，P2-3）**：`FasSignal` 增第二个谓词 `playback`，生产方 =
  `TunedGovernor`（`init_policies` 接管 `playback` 置位、`release()` 清零，只碰这一位），消费方 =
  fps 探针挂载谓词 `frame_source_active()` = **FAS 激活 或（playback + `diag_active()`）**；探针在
  FAS 未激活时**不投喂控制路径**，只按秒落离线直方图 `event` 行（`decision=playback_fps`、
  `reason=n=…;b<档>=…`，档宽 4ms、无帧不落行、进出旁路重起窗口）。`wait_until_active` 已删——
  统一走 `wait_until_frame_source(timeout)`，只有「playback 在接管但诊断关着」用 1s 有界等待兜底
  （诊断开关无唤醒通道）。口径正文见 `agentsdocs/03-chiri.md` 的 FAS 帧源门控条。

- ChiRi 是设备上唯一的 userspace sysfs 写频调度程序（口径，2026-09-26 依内核源码分析修正）：设备上**不存在常态竞争的厂商守护进程/第三方调度模块**；频率实际决策链 = waltgov + vendor hook（OMRG/frame_boost）+ FREQ_QOS 聚合，详见 `.cursor/docs/kernel-analysis/06-vendor-inventory.md`，ChiRi 写 scaling_max/min 是 clamp 不是频率决策，内核 thermal QoS 钳制是合法态不算篡改；防篡改/周期重写（fast/power_base 5s、FAS 30s 强制重写、CLG 1s、bg uclamp 60s 再断言等）一律是**异常兜底**——防的是残留旧模块、手动调试、内核异常态下的异常改写，文档/汇报勿再写成厂商对抗。

- **热压制恒钳开关（2026-09-26 契约，配置级）**：`Thermal.clamp_heavy: bool`（serde 默认 `true`），`Config::load` 同步到 CLG，启动与热重载均生效。`true` = cap 窗口内所有簇**恒钳**写频目标、`free_above` 豁免档被忽略；`false` = 精确回退旧 `free_above` 豁免行为。只改钳制判据，仍**只钳写频、不回写 `current_perf`**（状态卡死根因不存在）。8550 feature.yaml 已置 `true`；行为级证据 = governor 侧 `clamp_apply` 跃迁事件（bind/unbind 跃迁时、`diag_active()` 门控内）。口径正文见 `agentsdocs/03-chiri.md` Thermal 段。

- **热阶梯四级 + 逐级回滞（2026-09-27 契约，配置级）**：`ThermalGuardConfig` 由三级扩为**四级** `1.0 → soft_perf_cap → mid_perf_cap → hard_perf_cap`，新增 `batt_mid_temp_c`（默认 43）、`cpu_mid_temp_c`（默认 92）、`mid_perf_cap`（默认 0.60，8550 0.60）、`hysteresis_hard_c`（硬档专用回滞，默认与 `hysteresis_c` 同值 3，8550 取 2）；`normalize` 强制 `soft < mid < hard`、`hard_cap <= mid_cap <= soft_cap`，越界**钳到最近合法值 + warn**（`thermal-config-clamped`，不 panic）。硬限兜底判据是 `hard < soft + 1.0`（**不是** `hard <= soft`）：mid 的钳制区间是 `(soft+0.5, hard-0.5)`，差在 (0,1) 的配置（如 batt 44.9/45.0）在旧判据下不兜底，`f32::clamp` 因 `min > max` **panic** → `Config::load` 崩溃 → watchdog 重启循环（2026-09-27 修，`thermal_clamp_warn` 另加 `hi < lo` 时夹成 `hi = lo` 的兜底）。`eval_thermal_cap` 每一级的上/下两条边都带回滞（改前硬档→软档这条边无回滞，batt 44.9°C 即从 0.40 跳回 0.85）。语义：8550 的 41/43/45°C ↔ 0.85/0.60/0.40。
- **热态 tuned 响应（2026-09-27 契约，配置级）**：`Thermal.tuned_resp_enabled`（默认 true）+ `tuned_thermal_floor`（默认 0.55，normalize 保证 >= `hard_perf_cap`）；cap 经 `tuned::set_thermal_cap()` 与 CLG 同点下发，tuned 侧有效上限 = `min(档位天花板, max(cap, floor))`（无压制时零变化；软限 0.85 对播放态天花板天然 no-op，中档/硬档才咬得住）。改前 tuned 对温度零响应。
- **CLG 方向稳定（2026-09-27 修，P1-2）**：`on_load_update` 上边补齐与下边同源的死区 gate（no-op up 记 hold、不清 `down_wait`），下边 `up_wait = 0` 移到死区判定之后；无新增参数，保守化旋钮 = `write_deadzone`（0.03）。**up-hold 期间 `down_wait` 有意不清零**（`cpu_load_governor.rs` `[dir_stabilize_tick]`）——代价是「`down_wait` = 上限-1 → up-hold → 真 down」可比连续 N tick 提前 1 tick 降频（N=`down_rate_limit_ticks`，生产取 1..10）；换来的是噪声 tick 不抹掉降频确认进度，故保留（有意设计，非缺陷）。

## 进行中

- daemon-perf-opt 五阶段性能优化计划：`.cursor/docs/daemon-perf-opt-plan.md`
  （Phase 3 前置 `.trae/specs/harden-scenemode-and-screen-detect` 的 hold 三分支）。
