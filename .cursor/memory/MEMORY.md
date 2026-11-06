# ChiRi Cursor 环境记忆

> 按环境分文件夹写入：Cursor 的产出只写 `.cursor/`。历史长期事实（工具链口径、守护进程契约）
> 在 `.codebuddy/memory/MEMORY.md`，只读，开工必读。

## 协作约定

- 一切 AI 产出文件（计划、评估、报告等）只写项目文件夹内：本环境文档放 `.cursor/docs/`、记忆放 `.cursor/memory/`；
  工具自动写到项目外的副本（如 Cursor plans 目录）复制回项目后删除原件。
- **【铁律】任何文件不得落在项目文件夹之外**（2026-09-26 用户严令，违者零容忍）：
  CreatePlan 会把计划写到 `C:\Users\Aibeto Zhu\.cursor\plans\`——**每次创建计划后必须立即**：
  ① 原文复制到 **`.cursor/plans/<日期>-<名称>.plan.md`**（**2026-10-11 更正：原写 `.cursor/docs/plans/` 是 2026-09-26 归档前的旧路径，该目录已随归档删除**；计划目录现行口径见 `agentsdocs/07-todos.md:37`）；② 删除 plans 目录原件；③ 确认项目外无残留。
  本日已违例一次（两份计划滞留项目外被用户抓到），此流程刻死，不依赖回忆。
- 架构约定、契约数字、命令口径变更同步 `agentsdocs/` 与本文件；具体改动经过写 `.cursor/memory/YYYY-MM-DD.md`。
- CI 跳过口径：`build.yml` 的 `check-skip` 闸——提交信息含 `skip ci`（不分大小写、无需方括号）即跳过该次构建；
  `[skip ci]` 等方括号标记另走 GitHub 原生整 run 跳过；`workflow_dispatch` 手动触发恒运行；提交信息仅提及该字样也会命中。
- WebUI type-check 口径（2026-09-24）：`npm run type-check` = `svelte-check --tsgo`；TypeScript 7 经 npm 别名
  `@typescript/native` 接入，根 typescript 保持 6 系列（svelte-check 对等依赖只认 5/6 系列）；别名与 `--tsgo`
  成对，缺任一 svelte-check 直接报错退出。

## 契约要点

- **LTO 下发口径（2026-11-07）**：`cargo xtask build --lto full|thin|off|auto` 或 `CHIRI_LTO`；本地（`scripts/build-arch/build-arch.sh` 先询问一次）为固定档，全部模块共用、失败不回退；CI 用 `CHIRI_LTO=auto`，每个模块先 full、失败按模块回退 thin→off。模块 = 独立 cargo 构建单元：`chiri` 主程序 + `build.rs` 单独构建的 `yumi-ebpf`（WebUI 非 Rust 不参与）。用户口径 full 在 Cargo/rustc 侧必须写 `fat`（传 `full` 报 `incorrect value`）。实测同一提交：chiri LTO=off 7,117,024 B vs full 6,059,000 B；eBPF off 3824 B vs full 3504 B；本机热缓存 off 7m19s / full 5m53s。详见 `agentsdocs/02-convention.md` 的 LTO 契约条。
- **息屏静默档（2026-10-31）**：`scheduler_ipc` 周期任务按屏幕分两档——亮屏 telemetry 1s / mode file 5s / 兜底 `EVENT_POLL_MS=1s`；息屏 `TELEMETRY_LOG_INTERVAL_SCREEN_OFF=5s` / `MODE_FILE_REWRITE_INTERVAL_SCREEN_OFF=30s` / `EVENT_POLL_MS_SCREEN_OFF=2s`（thermal 2s 不动）。目的是减少息屏期唤醒与落盘（每次唤醒可能 `pm_wakeup_pending()` 打断 freeze、重跑 `dpm_suspend_start`，抬高挂起路径耗时撞 vWdog ≈6.18 s 实际窗口）；推送事件即时打断，亮屏响应不变；息屏兜底 2s < `CLG_STALE_MAX=5s` ⇒ 断投发现时间不变。详见 agentsdocs/03-chiri.md「屏幕状态判定与释放兜底」末条。
- FAS 所有权/叠加实现（2026-10-06）：前台检测与游戏 owner 分离；帧事件采集时携带 PID/generation，摘挂 drain 与 CLOCK_MONOTONIC 截止界标隔离旧 ring/队列。失焦期间暂停 FPS 反馈与目标推断，按真实 policy CPU 集合负载控制，不冻结旧高频；恢复清状态并预热 12 个有效新帧，无帧 500ms 回退 load-only，热保护仍由负载驱动。窗口核验单 worker、3s 最小间隔、800ms 超时、512KiB cap、5s 证据 TTL；Unknown 不无限续期，VisibleUnfocused 需 mObscured=false。详见 agentsdocs/03-chiri.md；本机 Cargo 目标验证遇到 registry/rustup 权限与缓存阻塞，portable 测试不能代替 Android/eBPF 通过证据。
- devimp 统计（2026-10-06）：文件/批次/root + --since 归档戳选择；样本等效 Wh 与有效短区间积分分开，跨 type/charge/mode/package/screen 边界不外推，长缺口不归因为 suspend；总口与动态模型扣同子集各自基线，负残差保留，模型不是物理下界。FPS 全空不证明未 FAS；版本按解析身份去重，缺失占位不冲突。命令正文与 agentsdocs/05-maint.md 已同步。
- daemon 自耗专项：唯一有效计划为 `.cursor/plans/2026-10-07-daemon-selfcost-implementation-plan.md`（合并版）。不调大 aff_secs、不主动关闭诊断作对照、不降采样、不摘 K3；30~40ms 是进程 CPU 配对估计，不是墙钟阻塞，日志覆盖不证明开关状态，所有测量调用入账、50ms 只限告警。2026-10-09 `diag_build_cost.rs` 已落：`selfcost-build/1 parent=build` 四类子块（get_thread_tids/tid_stat 含解析/snapshot_procs/sched_affinity）+ residual，失败也 count；CPU/墙钟各用有效分母，缺时钟/负残差/溢出不假零，父子绝不可相加。residual 含组装/渲染及未归子块 observer，端点读钟含部分自身成本；60s/关闭尾窗复用 selfcost，会话独占，窗口 reset 保留 errors、session reset 全清。worker 默认关闭；独立审查完成，无剩余阻断发现，编译/运行验证仍受阻，真机收益待验证。
- 本轮验证限制（主线回传）：Android cargo check 已尝试，Compiling chiri 后因 build.rs eBPF 的 -Z 仅 nightly 允许、当前 stable 失败，无 Checking chiri；纯模块 rustc --test 因项目内 TMPDIR 只读失败，测试未执行。两真实模块已由主线加入 scripts/tests/rust_regression.rs，尚未重跑；ReadLints totalFiles=0 不构成有效验证。mod.rs 单测 aff_cost_record 缺第 7 参已补 None 并经同一独立审查员复核有效；独立审查完成，无剩余阻断发现，静态审查不等于测试通过。
- 亲和重试缓存（2026-10-09，代码已落、验证待定）：normal_busy/normal_press 真实 EINVAL 按 starttime + 目标 CPU 列表退避 4/8/16/32s 封顶，成功/目标/身份变化清，释放/reload 失效；仅 restricted cpu.uclamp.max 真实 ENOENT 负缓存 600s，目标值与 max 恢复不破缓存，background 60s 守卫不变、到期可绕过同值 guard。mod.rs 两处真实 config_dirty/ConfigReload 入口 invalidate_retry_caches，不是 2s apply 清缓存。

- selfcost 一致性契约：配置指纹仅排序 map，保留列表优先顺序；配置、指纹及代际在配置写锁内发布，采样参数与身份在对应读锁内冻结，build 前释放。worker 故障关闭发送入口后排空至通道断开，失败提交等待 join 后回退，不再超时放行新差分帧；永久 IO 阻塞仍会导致等待无界，worker 保持默认关闭。修订未编译测试，详见 agentsdocs/02-convention.md。

- **SoC 硬件基线 soc.yaml（2026-09-26）**：`module/config/{soc}/soc.yaml`（topology/capacity/freq_khz/idle/dpc），编译期嵌入，sysfs 兜底与校准基准、非调优输入；`chiri_core_ranges`/capacity/freq 兜底已接，硬编码 match 保留为最后兜底。口径与坑见 agentsdocs/02-convention.md 配置嵌入条；审计结论（其余 rs 常数不外挂）见 `.archive/.cursor/docs/2026-09-26-soc-hardcode-audit.md`（该文件上轮已归档）。
- logd/ 归档预算清理（2026-09-24 起）：`enforce_logd_limit` 按**归档批次原子删**——
  `<ts>.tar` 与 `devimp_<ts>.tar` 同进退、最新批次永不删，最新批次 ≥200MB 时收到 256MB 即停。
  背景：超大 devimp 归档（114MB）曾把同批 daemon.log/status.csv 归档挤掉（logd_0924-173105 包）。
  目录预算同日扩容：`LOGD_MAX_BYTES` / `DEVIMP_DIR_MAX_BYTES` 128→256MB、TARGET 96→200MB。
  详见 agentsdocs/02-convention.md 归档条与「被挤掉」历史坑。
- **归档格式（2026-10-12 起为 zstd；本条已按当前代码校正，旧 lz4 口径作废）**：启动归档两目录产物均为
  **`logd/<ts>.tar.zst`** 与 **`logd/devimp_<ts>.tar.zst`**——`pack.sh archive` 只打**无压缩 tar**
  （tar 二进制优先 `core/bin/tar`，回退 `/system/bin/tar`/toybox/busybox），再由 **Rust 内置 `zstd` crate
  流式压成 `.tar.zst` 并删中间 tar**（vendored C，`zstdmt` 开 multithread，级别 2、6 线程；不依赖设备
  任何压缩二进制）；zstd 落盘 I/O 失败才回落保留 `.tar`。**导出（WebUI `pack.sh export`）自 2026-10-13
  起只打纯 `.tar`、不压缩**——设备上 zstd -19 分钟级太慢，改由用户侧解压。`core/bin/chiri zstd <file>`
  这个 [toolbox] 分发仍存在（级别 19），但导出路径已不再调用它（旧 `chiri gzip`/flate2 口径作废）。
  **分析侧禁止按扩展名判断内层形态**——一律嗅探 magic：`scripts/devimp/dvextract.py` 按内容路由，
  zstd 走三级兜底（stdlib `compression.zstd` → `zstandard` 库 → 系统 `zstd` CLI）、LZ4 帧走
  `dvlz4.py` 纯 py 解码器、其余按 raw tar。一条命令入口是 `dvrun.py`。

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

- ChiRi 是设备上唯一的 userspace sysfs 写频调度程序（口径，2026-09-26 依内核源码分析修正）：设备上**不存在常态竞争的厂商守护进程/第三方调度模块**；频率实际决策链 = waltgov + vendor hook（OMRG/frame_boost）+ FREQ_QOS 聚合，详见 **`.archive/.cursor/docs/kernel-analysis/06-vendor-inventory.md`**（2026-10-11 更正：`kernel-analysis/` 目录早已归档，`.cursor/docs/` 下的同路径已不存在），ChiRi 写 scaling_max/min 是 clamp 不是频率决策，内核 thermal QoS 钳制是合法态不算篡改；防篡改/周期重写（fast/power_base 5s、FAS 30s 强制重写、CLG 1s、bg uclamp 60s 再断言等）一律是**异常兜底**——防的是残留旧模块、手动调试、内核异常态下的异常改写，文档/汇报勿再写成厂商对抗。

- **息屏省电现状：两段（doze 0.30 立即 → scenemode 0.12 延迟）；2026-10-11 评估后决定不拆**：
  ① **doze 段**：屏幕一灭就生效（`src/chiri/mod.rs:3723-3732`，另在 `:3321-3329` 与 `:4222-4233` 重复同一构造），用 `get_clg_cfg(&cfg, "reduce")` 起底后覆盖 5 个字段 —— `enabled=true` / `perf_floor 0.0` / **`perf_ceil = .min(0.30)`** / `smoothing_up 0.10` / `touch_boost=false`；
  ② **scenemode 段**：`feature.yaml:20 scene_mode_delay_secs`（8550 YAML 现值 300 s；代码缺省 3000，被 YAML 覆盖）到期后进入（`mod.rs:4272-4285`），改用 `module/config/normal/scenemode.yaml` 的 `perf_ceil 0.12`。
  **两段配置同源**（都是 `scenemode.yaml` 那份，doze 是 reduce 起底覆盖得来）⇒ DiD（0.12 相对 0.30）量的是**段间边际**（≈0，甚至略正 +0.0461 W）。
  **来历**：doze 段是上游 yumi 的设计（`7e73d60` 2026-03-10，i18n `Enabling Extreme Doze mode`），ChiRi `a64366c` 继承、`540af54` 压到 0.30；scenemode 段是 ChiRi `1b9957b`（2026-08-30）自己加的。**全仓真天花板只有这两个**。

- **【方案 P1，本轮要做】取消二段式息屏（2026-10-11 用户目标 + 已定做法）**：**「息屏只有一个档」是用户明确目标**（原话「我的目的就是取消二段式息屏」）。做法（用户 2026-10-11 定）：**doze 与 CLG 本就是同一套算法、doze 只是换参数 ⇒ 取消二段式 = 息屏后的 CLG 保持与 `reduce` 完全相同参数**，删掉那 4 行息屏专用覆盖（`perf_ceil .min(0.30)` / `perf_floor 0.0` / `smoothing_up 0.10` / `touch_boost=false`，3 组共 12 行），**不换基座、不改走 default**（那会连带改 7 个字段并引入 little 0.95）。**硬约束：任何模式下都必须保留「随时可到硬件最高频」的能力**。计数 **`scene_mode_delay_secs = 3000 s`**（五机型 YAML 300→3000）。饱和退出**保持退回 reduce 不变**（窗口内同为 reduce 参数，天然一致）。方案 `.cursor/plans/2026-10-11-调度改进方案.md` **P1**。
  **代价 0.09 ~ 1.17 Wh（中枢 0.59）**：作用面 = 每次息屏的 `[0, min(段长, 3000))`，共 185 次息屏、其中 **119 次段长 <3000 s 永不进 scenemode**；实际窗口 **13469 s / 2.6791 Wh / avgW 0.716 / >1W 率 17.7%**。上界按 t2 匹配 ΔW 0.314 W → 1.17 Wh；中枢按窗口内「有 0.30 上限(13193 s, 0.713 W) vs 无上限(276 s, 0.871 W)」直接对比 Δ=0.158 W → 0.59 Wh；下界按 DiD 比值 0.147 折算 → 0.09 Wh。
  **关键参照**：窗口内「天花板咬住」组 avgW 0.702 与「未咬住」组 0.729 **几乎相同**，且咬住秒里 cpu_dyn 只占整机 22.6%（cpu_dyn avgW 0.157 W）⇒ 这条改动的代价主要不在「放掉天花板」，而在「把压制推迟到 3000 s」。
  **净收益（P1 代价 + P2 + P4）保守 +0.36 / 中枢 +1.59 / 乐观 +3.08 Wh**（P2 下界修正为 1.38 后，最保守情形**不再转负**）。
  **改动口径**：`mod.rs:3716-3733` 删那 4 行覆盖（`:3725/:3728/:3729/:3730`，基座 reduce 不动）；`:3321-3329`（FAS 失效）与 `:4222-4233`（scenemode 被关）同口径删；**饱和退出 `:4359-4381` 保持退回 reduce 不变**；五机型 `feature.yaml` 的 delay 300→3000；i18n 只改 `scheduler-doze-enable/restore` 文案（双语），special-keep 保留。
  **回退** = 恢复 doze 段构造 + 五机型 YAML 改回 300。

- **热压制恒钳开关（2026-09-26 契约，配置级）**：`Thermal.clamp_heavy: bool`（serde 默认 `true`），`Config::load` 同步到 CLG，启动与热重载均生效。`true` = cap 窗口内所有簇**恒钳**写频目标、`free_above` 豁免档被忽略；`false` = 精确回退旧 `free_above` 豁免行为。只改钳制判据，仍**只钳写频、不回写 `current_perf`**（状态卡死根因不存在）。8550 feature.yaml 已置 `true`；行为级证据 = governor 侧 `clamp_apply` 跃迁事件（bind/unbind 跃迁时、`diag_active()` 门控内）。口径正文见 `agentsdocs/03-chiri.md` Thermal 段。

- **热阶梯四级 + 逐级回滞（2026-09-27 契约，配置级）**：`ThermalGuardConfig` 由三级扩为**四级** `1.0 → soft_perf_cap → mid_perf_cap → hard_perf_cap`，新增 `batt_mid_temp_c`（默认 43）、`cpu_mid_temp_c`（默认 92）、`mid_perf_cap`（默认 0.60，8550 0.60）、`hysteresis_hard_c`（硬档专用回滞，默认与 `hysteresis_c` 同值 3，8550 取 2）；`normalize` 强制 `soft < mid < hard`、`hard_cap <= mid_cap <= soft_cap`，越界**钳到最近合法值 + warn**（`thermal-config-clamped`，不 panic）。硬限兜底判据是 `hard < soft + 1.0`（**不是** `hard <= soft`）：mid 的钳制区间是 `(soft+0.5, hard-0.5)`，差在 (0,1) 的配置（如 batt 44.9/45.0）在旧判据下不兜底，`f32::clamp` 因 `min > max` **panic** → `Config::load` 崩溃 → watchdog 重启循环（2026-09-27 修，`thermal_clamp_warn` 另加 `hi < lo` 时夹成 `hi = lo` 的兜底）。`eval_thermal_cap` 每一级的上/下两条边都带回滞（改前硬档→软档这条边无回滞，batt 44.9°C 即从 0.40 跳回 0.85）。语义：8550 的 41/43/45°C ↔ 0.85/0.60/0.40。
- **热态 tuned 响应（2026-09-27 契约，配置级）**：`Thermal.tuned_resp_enabled`（默认 true）+ `tuned_thermal_floor`（默认 0.55，normalize 保证 >= `hard_perf_cap`）；cap 经 `tuned::set_thermal_cap()` 与 CLG 同点下发，tuned 侧有效上限 = `min(档位天花板, max(cap, floor))`（无压制时零变化；软限 0.85 对播放态天花板天然 no-op，中档/硬档才咬得住）。改前 tuned 对温度零响应。
- **CLG 方向稳定（2026-09-27 修，P1-2）**：`on_load_update` 上边补齐与下边同源的死区 gate（no-op up 记 hold、不清 `down_wait`），下边 `up_wait = 0` 移到死区判定之后；无新增参数，保守化旋钮 = `write_deadzone`（0.03）。**up-hold 期间 `down_wait` 有意不清零**（`cpu_load_governor.rs` `[dir_stabilize_tick]`）——代价是「`down_wait` = 上限-1 → up-hold → 真 down」可比连续 N tick 提前 1 tick 降频（N=`down_rate_limit_ticks`，生产取 1..10）；换来的是噪声 tick 不抹掉降频确认进度，故保留（有意设计，非缺陷）。

- **通知组件三处调用 + 投递下沉脚本（2026-10-17 契约、2026-10-18 扩充，用户指定）**：`notify::post_notification(title, tag, body) -> bool` 现三处调用——① FAS 开环翻转沿（`src/chiri/fas_manager.rs`，tag `ChiRi-FAS`，走拦截器：前 120 条静默 + 1h 同文去重）；② **启动完成通知**（`src/main.rs` 启动尾部，`monitor_thread` spawn 成功后、`join()` 前，tag `ChiRi-Status`，标题=**字面量 `ChiRi已启动`**，正文按启动时是否 DOWN 分叉，见下方「启动通知正文按 DOWN 分叉」条；用户明确指定**中英同值且不走 i18n**）；③ **日志超限通知**（`src/logger.rs::note_write`，tag **`ChiRi-Logs`**——**刻意不与启动通知的 `ChiRi-Status` 共 tag**，见下方「日志超限」条）。位置理由：调度线程与监控线程都起来才算「启动完成」，放更早会让「起一半」与「起完」在通知栏同形（正是静默失效专项要消灭的形态）。`ChiRi-Status` **无拦截器**（`FAS_TAG` 常量只认 `ChiRi-FAS`；该常量原名 `FAS_CHANNEL`，已随语义校正一并改名，`post_notification` 的形参也从 `channel` 改为 `tag`）——每次启动都该有一条，去重反而错。与该 tag 同名的旧常驻通知子系统（小写 `chiri-status`、`-c` 渠道 + `-o` tag）已于 db299f6 整体删除，**不是同一样东西**。日志留痕 `notify-startup-posted`（zh/en 均补，357:357）、成功留痕 `notify-post-ok`、脚本缺失 `notify-script-missing`。

  **真机故障与根因（2026-10-17，用户报告「日志显示推送了，通知栏没有」）**：投递已从「Rust 直接 spawn `/system/bin/cmd`」改为「三参交 `module/scripts/notify.sh`」（与 `scripts/pack.sh` 同级对外稳定接口，构建流程不得修改；`customize.sh` 补 `chmod 755`，`service.sh` 本就 `chmod -R 755 scripts/`）。**注意 2026-10-17 第三轮后 daemon 实际执行的是 `include_str!` 内嵌正文**（`scripts::run` 释放到临时目录再跑），磁盘那份只给 WebUI 用并由 daemon 监听还原——详见下方「脚本防篡改」条。三条 AOSP 事实（已核 `NotificationShellCmd` 源码）：① 用法 `post [flags] <tag> <text>`，**第三位置参数是 tag 而非渠道**（同 tag 互相覆盖）；`-c` 是 `--content-intent` **不是渠道**，渠道 id 由 AOSP 硬编码为 `shell_cmd`/`Shell command`，**调用方无法指定渠道**——本仓库「渠道」叫法实际进的是 tag 位。② **根因（机制已核 AOSP 源码）**：`NotificationShellCmd.onCommand` 在 `callingUid == ROOT_UID` 时把 `callingPackage` 设为 `NotificationManagerService.ROOT_PKG`（字符串 `"root"`，**不是真实安装的包**）→ `enqueueNotificationInternal` → `fixNotification` 的 `getApplicationInfoAsUser("root", …)` 抛 `NameNotFoundException` → 记 `Cannot fix notification` → **`return false`，通知被丢弃**。换 uid 2000（`com.android.shell`，真实包）才落地。root 其实被 `checkShellCommandPermission` **明确放行**且持有 POST_NOTIFICATIONS（AMS `canAccessUnexportedComponents`）——「root 被禁」说法错、结论对。**退出码 0 毫无意义**：`onCommand` 丢掉 `doNotify` 返回值并 `return 0`，只有权限门返回 255（`-o|--ongoing` 不存在；`-u|--user` 13~16-release 也没有，传了打 usage **仍退出 0**）。**换身份也可能静默失败**：MagiskSU optstring `+:himpds:VvZ:Mt:g:G:` **没有 `l`**，`su -lp 2000 -c …` 在 Magisk 上**仍是 root**（toybox/KernelSU 支持 `-l`）⇒ **`su 2000 /system/bin/cmd …`（argv 直传）必须排第一**（真机实测通过：不经内层 shell，免一层引号转义又绕开各家 `su -c` 语义差异），`su 2000 -c` 次之。**真机实证（2026-10-17 一加 SM8550/ColorOS）**：`su 0 cmd notification post -v …` 打印 `warning: couldn't find notification after enqueueing` 且无 NotificationRecord；`su 2000 cmd notification post -v …` 打印 `posted:` + `NotificationRecord(… pkg=com.android.shell uid=2000 key=0|com.android.shell|2020|TAG|2000)`。那条 `fixNotification` 推导已由「源码推导」升级为**真机证实**。③ 故**一律以输出判定**：脚本用 AOSP 自带 `-v`（enqueue 后回查，打 `posted:` 或 `warning: couldn't find notification after enqueueing`）分出 `posted`/`notfound`/`rejected`/`unknown`，**只确认落地才收下**，`notfound` 整档跳过换下一 su 形式；回传 `S<n>=…` / `S<n>:ok|dropped|fail:rc|rejected` / `OK S<n>` / `VERIFY posted|unverified|dropped` / `LIST ok|miss|unavailable`，Rust 只按脚本退出码记账并留 `{ $strategy }`+`{ $verify }`、**绝不自行拼 `cmd` 命令行**。`main.rs` 改为**成功才打** `notify-startup-posted`（原无条件打，把失败报成成功——即使按失败也留痕）。

- **脚本防篡改（`src/scripts.rs`，2026-10-17 第三轮）**：两个 `.sh` 有**两份身份**，口径不能混——
① **daemon 走二进制内嵌正文**（`include_str!` + `Script::run`）：每次调用前释放到私有临时目录（`$TMPDIR`→`/data/local/tmp`→`/dev/shm`，文件名带 pid）、`chmod 0700`、跑完即删；**每次必写不做「存在即跳过」**（否则抢先落地伪造同名文件即可长期劫持），故磁盘副本改了不生效。② **WebUI 走磁盘副本**（WebView 取不到内嵌内容）：可信度由 daemon 维持——`main.rs` 启动**无条件覆盖**一次（置 0755，WebUI 要 `sh` 它）+ 常驻 `scripts-watch` 线程 inotify 监听还原（独立线程、不 join、不 panic）。**监听必须挂目录**：原子替换后文件级 watch 会永久失效（同 `app_detect::watch_config_file` 教训）；已实测原地写/原子 mv/删除/无关文件四种形态判定正确。**仓库里不能删这两个 `.sh`**（`include_str!` 编译期读取，删了构建失败；与 `BIN_ONLY` 那批运行时无读取方的配置不同）。**WebUI**：新增 `contract/notify.ts` + `AdvancedView` 底部「通知测试」面板（tag 固定 `ChiRi-Test`，标题/正文预填可改，**原样展示脚本输出**；新增 `components/TextField.svelte`、`REL.notifySh`）。副产物：`shQuote` 外的脚本内 `quote()` 已单测验证免注入（`'`→`'\''` 循环实现）。

- **远程提醒（`src/remote_push.rs`，2026-10-18 用户口径）**：后台线程 `remote_push`（独立、不 join、失败不 panic）。**每天本地 08:00~24:00 每 2 小时**拉一次，槽位 `[8,10,12,14,16,18,20,22]`（24 点即次日零点，不在偶数槽位）；线程 60s 醒一次比对小时，同槽位只拉一次（key=年/月/日/时），槽位中途启动会补拉一次。主源 GitHub raw → 失败改 gitee raw，**失败不重试**、各留一条 **info**（`remote-push-primary-failed`/`remote-push-fallback-failed`）。JSON 为 `{versionCode,title,content}` 数组，`versionCode` 为**数字**（用户 2026-10-18 修正）；与 versionCode **恒等比较、不去前导零**（归一会让 `"030001"` 命中 `30001`，用户明确排除）。**当前 versionCode 只认编译期 `include_str!` 内嵌的 `module.prop`，绝不读磁盘**（磁盘可被篡改，改它就能命中任意远程条目、收到伪造提醒）；内嵌值缺失→空串→`find_entry` 直接判不命中，绝不当通配符。**未命中静默**（debug 级）。命中用 JSON 的 title/content 发通知，tag **`ChiRi-Remote`**（无拦截器）。依赖 `ureq`(rustls)+`serde_json`：rustls 走 ring（C/asm，与 zstd-sys 同需 cc，CI lint 已注入 NDK CC_/AR_/CFLAGS_），根证书 `webpki-roots` 内嵌；超时 10s、响应上限 64KB。

- **日志超限：FAS 门控（10 分钟冷却）+ ChiRi-Status 通知（2026-10-18 用户口径）**：`logger::note_write` 达 `LOG_RESTART_THRESHOLD_BYTES`(128MB) 后先做**冷却判定**（`LOG_RESTART_COOLDOWN_UNTIL_MS`，相对 `PROC_START` 的毫秒；冷却中直接 return，**必须排在 `watchdog_pid()` 之前**，否则冷却 10 分钟里每次写日志都在读 pidfile），再读 `current_mode.chr` 经纯谓词 `log_overrun_allowed`——**只有明确的非 FAS 模式才放行**。`fas` 或读不到模式（`None`）→ 记一条 `logger-log-restart-fas-deferred` warn + 置 10 分钟冷却（`LOG_RESTART_FAS_COOLDOWN`=600s），到点**重新判定**（不是等再攒满 128MB）；**计数不清零**。每个冷却周期一条 warn（≤6 条/小时）。`None` 也拦下是刻意的：无法确认模式时宁可暂缓归档，也不要在 FAS 对局中退出调度。**FAS 模式下通知一并不发**（门控在 `post_notification` 之前 return）。放行后**先发通知、再判看门狗**——`notify::post_notification("ChiRi日志事件", "ChiRi-Logs", "日志大小即将达到阈值，即将开始压缩，调度将短暂退出")`，全字面量不走 i18n。**tag 必须与启动通知的 `ChiRi-Status` 分开**：AOSP 同 tag 是覆盖关系，而本通知后进程立刻 `exit(0)`、看门狗 3s 后拉起的新进程会再发一条 `ChiRi-Status` 启动通知，共用 tag 会让日志事件几秒内被顶掉——表现为日志里 `verify=posted` 成功、用户却看不到（2026-10-18 真机实证）。**看门狗在发通知前先判一次**：有 → 只发超限通知；**没有 → 除超限通知外再补一条人工接管告警**（标题 `ChiRi daemon`、tag **`ChiRi-Watchdog`**、正文 `Watchdog 代理启动失败，调度无法自动重启，请求人工接管`，字面量、独立 tag 互不覆盖），成功留痕 `logger-log-watchdog-alert-posted`。相关日志带 `notify={ $notify }` 与 `alert={ $alert }`。用 `LOG_OVER_NOTIFYING` 串行化投递。**读不到 `current_mode.chr` 按非 FAS 放行**（`logger-log-restart-mode-unknown` 留痕）：原实现把「无法确认」当「确认 FAS」永久推迟，文件一读不到通知就再也发不出。通知必须在 `exit(0)` 之前。`note_write` 的调用点均在 appender 锁释放后，故内嵌 `post_notification`（会走 `log::info!` 重入）不会死锁，`LOG_RESTARTING` 另防重入。

- **启动通知正文按 DOWN 分叉（2026-10-18 用户口径）**：`src/main.rs` 启动尾部 `post_notification("ChiRi已启动", "ChiRi-Status", body)`，`body` = 以 DOWN 模式启动时 `ChiRi调度已启动，当前以DOWN模式运行，需要其他调度接管`，否则 `ChiRi已启动`（标题不变）。DOWN 判定必须是 `down::is_down() || down::read_down(&root)`：全局 `DOWN_ACTIVE` 由**调度线程**里的 `down::on_startup` 置位，而通知在监控线程 spawn 后立刻执行——**存在竞态**，只信它会漏判。`down::read_down` 已由 `fn` 放宽为 `pub(crate) fn`（它读不到时会补建 down.chr 模板，属既有行为，模板只有注释故判 false，不会误判 DOWN）。

- **devimp main_ 45 列 + `activity` 列（2026-10-18 契约；同日修正采样设计）**：main_ CSV 44→45 列，末尾追加 `activity`（既有列索引不变、旧脚本按前 44 列仍可读）。值 = 前台 Activity 查表键短形 `pkg/.MainActivity`（`activity_key` 归一：`.MainActivity` 与全限定同包名收同一键、`$` 内部类不剥；外部包 component 归一后保留全串但本列按当前前台包前缀校验，非当前包写 `-`）。**修正（2026-10-18）：采样全局化，不再只限 FAS**——原实现只在 `reconcile_fas_focus` 慢路径查窗口，`fast_same` 快速路径直接 return ⇒ 正常对局一次也不采样，activity 恒 `-`；而游戏通常只有一个 Activity，真正需要 Activity 级规则的是微信/B 站等多页面应用。现新增 `ActivitySampler`（`src/monitor/window_visibility.rs`）：屏幕亮且有有效前台时请求，同包 3s 节流、换包 1s 内立即，只跑 `dumpsys window` 取 `mCurrentFocus`；节流基准是「上次查询的包」（失败也算，避免退化 1s 重试）；触发条件 = 配置了 Activity 级规则（`dynamic_enabled` && `activity_aware` && `app_modes` 含 `pkg/.Activity` 键）或开启 `meta.dev_record`，两者都关时零 dumpsys 开销。`app_detect` 只接受属于当前前台包的 activity 键（换包旧值/系统叠加层丢弃），**同包内 activity 变化会重算模式**（activity 不参与 identity/generation，不触发 FAS 重接管），Activity 级规则因此对非 FAS 应用生效；`-` = 从未采到或焦点属其他包，屏幕灭/采样失败/同包空焦点保留最后采样值（可能陈旧，落行前仍有前缀复核）；DOWN 停摆期间采样继续；非 ChiRi 的 diag_writer 路径不采样恒 `-`。`aff_` 文件头自 2026-10-18 起只写标题 + 4 行可解析 schema（@A/@S/p/t）+ 元信息，差分/刷新帧/NaN 说明不再写入（语义在 agentsdocs/02 与 `dvaff.py`）。验证：host `cargo test --bin chiri` 233 passed / 3 failed（既有 fas_manager 失败）；Android target 因本机无 NDK 未编译（既有环境限制）；真机待验。

## 进行中

- **CLG 高位升频门槛（2026-10-13 落地，未提交）**：`CpuLoadGovernorConfig` 新增 `high_zone_gate`(0.85)/`high_zone_count`(0=关)/`high_zone_util`(0.90)/`high_zone_hold_ticks`(20) 四字段；`on_load_update` 在 perf ≥ gate 后升频需「≥count 核 util ≥ util」确认，未满足冻结 tick（decision=`hz_hold`，早退不动 up/down_wait），满 hold_ticks 强制放行一次。**只压决策意图，不封顶不回写 current_perf**（R2/R7 合规）；8550 三段（reduce/default/boost）全启用 count=2。replay 模板/生成器/契约测试已同步（clg_tuning 5/5 含 `high_zone_gate_knobs`；selfcheck 含「单核阻塞 + hold 兜底 2000 tick 到 hw_max」断言）。WebUI status 快照表温度列已删（数据解析契约不动）。回退 = 删 8550 三段三行。详见 `.archive/.cursor/memory/2026-10-13.md`。

- daemon-perf-opt 五阶段性能优化计划：**已归档** `.archive/.cursor/docs/daemon-perf-opt-plan.md`（原路径 `.cursor/docs/` 下的副本已不存在）
  （Phase 3 前置 `.trae/specs/harden-scenemode-and-screen-detect` 的 hold 三分支）。

- **devimpbin/ 口径（2026-10-11 用户纠正）**：`devimpbin/` 是**本项目工作区内的日志包目录**（`.gitignore:41` 忽略，15G，历次解包），**不是要删除的残留** —— 本轮对话早前多次把「项目外 `~/devimpbin`」当成待清理残留，是混淆了两处路径；`devimpbin/_tmp/` 是一次性探针的合法落盘处。
- **一次性分析脚本落盘位置（2026-10-11 新增）**：确需手写一次性探针时一律写 `devimpbin/_tmp/`
  （`devimpbin/` 已被 `.gitignore:41` 忽略），**不得落在项目根目录** —— 项目根无 ignore 覆盖，
  会在 `git status` 里堆出上百个 `??` 脏项。优先复用 `scripts/devimp/` 常驻脚本（含 `dvselfcost.py`，
  `feas`/`diag`/`pair` 三子命令，需 `--manifest`；注意 `dvrun.py` 的 `STAGES` 未包含它）。

- **收口/汇报前必做智能体生命周期核查（2026-10-11 新增）**：`list_agents` + `job_list` + `ps -eo pid,etimes,args`
  三重核实，确认无活跃子代理、无未完成后台任务后再宣布收口。教训：曾出现汇报结束时子代理仍在跑、
  3 个 sleep 轮询 job 未收的情况。派子代理前必须在 prompt 写明终端调用上限 / scratch 单文件路径 / 收尾清理要求。

- **汇报结构最低要求（2026-10-11 新增）**：调度/功耗类结论必须给出「问题 → 原因（含源码路径）→ 后果 →
  影响（量化）→ 快速判定方法（用哪个 dv*.py 看哪一段）+ 改哪一行」，缺一不可。仅给结论数字视为不合格。

- **logd_1011-000909 调度功耗定稿口径（2026-10-11）**：整机放电 66.5 Wh（status.csv `charge=discharging` ∧ `0<batt_power_w<=20`，100344 样本）；
  调度侧**净收益视 P1 代价档位**：保守 **+0.36 Wh（+0.5%）** / 中枢 **+1.59 Wh（+2.4%）** / 乐观 **+3.08 Wh（+4.6%）** = P1 代价（−1.17 / −0.59 / −0.09）+ P2 + P4。P2 对外区间 **1.38 ~ 2.77 Wh**（10%~20% 两档、**均已含热态 4% 折扣**；旧版 0.93~2.88 一端打折一端没打，已修正）；P3 与 P2 共享窗口不另加。
  硬地板必须先扣：屏幕本体 6.35 Wh + 息屏地板外围 4.38~8.14 Wh = 10.73~14.49 Wh（16%~22%）。
  **CPU 动态模型只占整机 31.3%（20.79 Wh）**，残差 68.7% 是 GPU/外围/模型误差 —— 任何 CPU 调频方案的收益上限受此约束。
  定稿文档：**`.cursor/plans/2026-10-11-调度改进方案.md`**（**2026-10-11 由 `.cursor/docs/` 迁入计划目录并精简为 326 行**；结构 = §0 结论 / §1 能量盘子 / §2 改动清单（P1/P2/P4/P5/P11/P12）+ 已删除 8 条备查 / §3 约束与口径（含 doze＝scenemode、DiD 裁决、tick 四口径、硬口径 R1–R7）/ §4 验证 / §5 不做与证据不足 / §6 六路索引）。**完整版 639 行备份在 `devimpbin/_tmp/_plan_full_backup.md`**（gitignore 覆盖）。
  两条裁决须记住：① scenemode 相对 doze 的因果边际 DiD=+0.0461 W（方向与省电相反）⇒ **息屏类收益只剩 P1 的 0.02~0.07 Wh**，t2 原上界 0.49 Wh 作废；
  ② t6（playback/fas/akmode）0.50~1.50 Wh 与 tuned headroom **同一杠杆**（后者的 P9/P10 已删，不再是重叠问题）。
  热档不要当省电手段：放电期非 100 档仅 4.03%，cap=60 段均 5.33 W **高于** cap=100 段 2.32 W。
  **2026-10-11 两批删除共 8 条**（方案内记作「第一/第二轮」）：①「删掉不建议做的和几乎没收益的」→ P8（doze 再压 0.20）、P9/P10（tuned headroom）、P6（scenemode 解耦/阈值）、P7（delay 300→120）、CR.1（CLG 重建，≤0.02 Wh）；②**「不调 GPU，另外 doze 就是 scenemode」**→ **P3（GPU 协同抑制）、P13（削三特调模式）**。**保留清单 = P1 / P2 / P4 / P5 / P11 / P12**。删除理由/勿重评/重开条件集中在方案 **§2.2**；对账与借鉴底账在 **§2.1、§3.4**。
  决策占比有**至少四个不可互换口径**（tick 行 `screen_on` 恒为 `-`，**无法按屏筛选**）：全 mode 合并 tick（n=1899928）up 40.8 / down 9.0 / down_wait 9.2；default 单独 tick（n=1329613）up 51.6 / 7.6 / 3.6；全 mode 按秒（n=1147979）48.4 / 10.4 / 10.0；default 按秒（n=714358）66.9 / 8.0 / 2.2。**「亮屏」二字不得挂在 tick 数字上**；`fas` 无 tick 行（走 governor=performance）。脚本 `devimpbin/_tmp/tmp_chk_speed6.py`（**从仓库根执行**）。

- **计划/报告分目录口径（2026-10-11 定，用户指出）**：**计划类 → `.cursor/plans/`**（唯一权威 = `agentsdocs/07-todos.md:37`；`.cursor/docs/` 下只留报告类）。2026-10-11 已把 `2026-10-06-fas-ownership-plan.md`、`2026-10-09-energy-three-items-plan.md`（`git mv`）、`2026-10-11-调度改进方案.md` 迁入；`.cursor/docs/plans/` 与 `.cursor/docs/kernel-analysis/` 两个空目录（内容早已在 `.archive/`）已删。
  **其他环境目录 `.codebuddy/` `.trae/` `.workbuddy/` 与 `agentsdocs/` 只读不写**（`AGENTS.md` 首条）；跨环境借鉴只做对账，改动一律落 `.cursor/`。

- **其他环境的硬口径（读，勿重复评估）**：`.codebuddy/plans/d924b09f…/plan.md`（v14）的 **R1**（只有 SceneMode 允许不到最高频）、**R2**（天花板全拆；`perf_ceil<1`/`headroom<1` 都算，全仓现只剩 `module/config/normal/scenemode.yaml:15 perf_ceil: 0.12`）、**R3**（**永不再新增 A/B 对照**，允许同版本同场景观察）、**R5**（判据 = 帕累托前沿，46 点下凸；先 big 低频 499200→614400、prime 只在末端）、**R6**（放置类动作三道时间闸 `MIN_MIGRATE_INTERVAL` 4s/`REPIN_DEBOUNCE` 8s/`RETURN_COOLDOWN` 16s 不得绕过，新门槛须成对滞回）、**R7**（模式差异只能靠取点/到达速度/降权表达，不得用封顶；交叠带 default×1、reduce×0.5、boost×2）；回退三项（触发条件/字段级可逆/留档位置）**缺一不排期**。`agentsdocs/07-todos.md:53` 明写棘轮**不要往调阈值方向改**；`ANALYSIS-1009.md:297-301` ③④同样限定了 FAS 退出期不得提前降 perf。

- **并行写者互斥范围（2026-10-11 教训）**：派子代理写文档时必须给出互斥写范围，否则会覆盖主线程版本。
  实例：子代理 `2d93d9f6-56d3-40dd-a701-955ca80ea414` 覆盖了当时的 `.cursor/plans/2026-10-11-调度改进方案.md`（保留原位；321 行 → 445 行），
  丢失速判表与合规自查；同名 `tmp_an200~203.py` 亦被覆盖（脚本丢失、结论尚存）。规则见 `.cursor/rules/parallel-agent-file-ownership.mdc`。

- **MemSearch（@zilliz/memsearch-dsh）索引故障与修复口径（2026-10-11 新增）**：这是 **MemSearch 记忆配置**，不是宿主 agent 自带的 memory/config 系统。
  根因：全局 `~/.memsearch/config.toml` 不存在 ⇒ 嵌入 provider 走内置默认 **openai**（`config.py:58`），而本机**无 OPENAI_API_KEY** ⇒
  `.index-state.json` 长期 `status: error`（`last_error = OpenAIError: Missing credentials`）。默认 milvus uri `~/.memsearch/milvus.db` 也从未创建。
  **memsearch 只认 `$HOME/.memsearch/config.toml`（`config.py:26` expanduser，无 env 可重定向）**；项目级 `.memsearch.toml` 被白名单过滤
  （`config.py:38-46`：只允许 milvus.collection / embedding.batch_size / chunking.* / indexing.* / watch.debounce_ms）；**插件从不传 `--provider`/`--model`**
  （`@zilliz/memsearch-dsh/index.js:382-402`）。
  选定修复：`[embedding] provider="onnx" model="gpahal/bge-m3-onnx-int8"`（纯 CPU、无密钥、可离线；索引 21 chunks ≈ 20~25 s，单次 search ≈ 10~12 s，
  插件超时 15000 ms）；**huggingface.co 本机不可达，须 `HF_ENDPOINT=https://hf-mirror.com`**；模型本地副本在 `devimpbin/_hf-cache/`（560MB）；
  一键脚本 `devimpbin/fix-memsearch-onnx.sh`，**必须在真实终端跑**（DSH 工具沙箱 /home 为 ro，`mkdir ~/.memsearch` 报只读文件系统），跑完重启 DSH。
  已排除：ollama（无二进制）、local（需 torch）、hapi-cn（无 embedding 模型 / 404）、MiniLM 系（512 截断与位置嵌入不匹配，onnxruntime broadcast 报错）。

## 亲和写失败契约（2026-10-26 定，改任何 pin/place 写路径都必须守住）

- **失败是一等状态，不是重试信号**：掩码写被内核拒（EINVAL）后必须①记账、②指数退避、③回滚引擎状态。
  三处实现：`pin_fail_wait()`（`REPIN_DEBOUNCE`=8s × 2^(n-1)，封顶 `PIN_FAIL_BACKOFF_MAX`=60s）、
  进程级 `narrow_pid` 备忘（同 pid 同簇 15s，只对 EINVAL 记账，前台/boost 变化清表）、
  `placement::Engine::abort()`（退回 `applied` 锚点，失败退避 4s × 2^(n-1) 封顶 30s）。
- **记账取回读掩码，不取写入意图**（`Engine::note_applied`）：cgroup cpuset 与不可用核会让内核静默摘核，
  「写返回 0」不等于全生效。`@A act=place` 的 `value` 已追加 `eff=<lo>-<hi>` 供对账。
- **取证基线**：sm8550/PHB110 14 批 A02-14 —— `place` 写失败 **80%**、ok 里 54% 又被裁剪 → 真正生效 ≈9.5%；
  `pin fg_pin` 失败 **90.5%**；失败呈爆发（单秒 431 条，全为同 pid 的 417 个线程钉同一颗核）；
  sm8750 同一份代码几乎零失败。完整报告 `.cursor/docs/2026-10-26-亲和写失败取证与窄域修复.md`。
- **本机验证口径更正**：`YUMI_SKIP_EBPF=1 cargo check -p chiri` **可用**（≈36s，exit 0），
  比 `agentsdocs/01-overview.md` 里「本机只能做 fmt / sh -n / rustc --test 三件套」更宽 —— 涉及 Rust 语义的改动应先跑它。
- **复核结果（2026-10-30，29 批）**：fg_pin e22 63,856 → **60**；place 62,531 → **6,353**；合计 **19.6×**。
  但 place ok 的 34,937 行里 **81% settled=false / 64% clipped=true** ⇒ 一部分 EINVAL 转成了**静默裁剪**，
  故新增 `note_stripped`（写 ok 但回读整簇被摘 → 同样记 15s 窄域备忘，只记整簇）。被摘核 94% 在核 0-2，
  但小核仍承担 11-12% 驻留且 93 个 pid 拿到过 ⇒ 是「间歇可用」而非 cpuset 收窄。

## EEM 调频/放置契约（2026-10-30 定，改 `energy_model`/`cpu_load_governor`/`placement` 必守）

- **`k` 是核当量近似，不是实测忙核数**：代码里 `k = ceil(Σ clamp(util,0,1))`。它只保证总量够，
  不保证每颗核够 ⇒ `eem_target_freq` 必须再过一道 `demand_with_core_floor`（`max(Σ, busiest×k)`）。
  纯函数、均匀载荷逐位不变，回归 `core_floor_is_noop_for_even_load` / `core_floor_lifts_gear_for_uneven_load`。
- **封顶的释放是滞回，不是节拍**：超 `EEM_CAP_RELEASE_TICKS`(10 tick = **1.6s**，不是 10s) 即挂起
  （`eem_cap_suspended`），只有甜点档供给重新够用才解挂；「清 0 后下一 tick 重封」的占空比是 91%，等于长期欠供。
- **「别处更便宜」必须同口径且供得上**：`best_placement(demand, online, only, hysteresis, cores)`
  现在按调用方给的**核数**比价（单线程传 1，簇级传 k），并要求候选 `supply ≥ demand`、在线核数 ≥ cores。
  原先恒 `cores:1`，k≥2 时本簇成本被放大 2–4 倍，会封甜点又迁不过去（big 两核各 0.7 实测缺供 29%）。
- **换前台即清记忆**：EMA/steady/up_wait/high_zone_wait/fast_down/eem_* 是「上一个 App 的负载记忆」，
  包名变化即重置（`[fg_reset]`），但**不动 current_perf/current_freq**。空包名（解锁瞬间）不算切换。
- **模型不能当整机预测器**：低估 1.0–1.8 W（8750 段反而高估 1.34×），方向都不稳定，只可用于相对排序；
  8750/8650 的簇级瓦数是单核线性外推，顶档不可达（求和 35.31 W vs 实测整机 7.03 W）。
- **日志口径**：main tick 行的 `cur_freq_khz` 是**决策值/上限**，不是实测频率（实测在 snap 的 `cpu_cur_khz`）；
  行数 ≠ 时间（tick 行按 (policy,cluster) 去重 + 2s 心跳），做墙钟统计必须按相邻 Δts 加权。


## 负载源与降级契约（2026-10-31 定，改 `cpu_monitor`/`monitor`/`fas_manager` 必守）

- **eBPF 主探针不是所有机型都可用**：`BPF_MAP_TYPE_RINGBUF` 需内核 ≥5.8。加载失败时**必须**切
  `start_proc_stat_fallback`（逐核 /proc/stat 差分，节拍与 eBPF 路径一致），否则 daemon 活着但永不发
  SystemLoadUpdate，CLG/tuned 看门狗会反复 release（实测 sm8250/4.19：68 分钟 release 26 次）。
- **降级路径两条硬约束**：① `total_delta == 0` 沿用上一拍（不能抖成 0）；② `/proc/stat` 连续失败
  >50 拍（≈8s）返回 Err 交回看门狗 —— 宁可不接管，也不发一串全 0 把频率压到底。不伪造 BpfStats。
- **用户通知要按「持续时长」而不是「翻转沿」**：FAS 开环 500ms 判据本身正确，但实测 417 次开环里
  中位 1s、p90 7s，逐沿通知 = 207 条骚扰。`OPEN_LOOP_NOTIFY_AFTER=10s`，进入只记 INFO。
  换会话（PID 替换）与去激活都必须清 `open_loop_since/notified`，否则新会话首条通知被吞。
- **`sched_migration_cost_ns`/`sched_nr_migrate` 在零售 GKI 不存在**（CONFIG_SCHED_DEBUG=n，
  只挂 debugfs）⇒ FAS 的 `migration_cost_ns` 与 Sched 段的迁移微调**在零售机上是空转**，
  不要再把「没生效」当异常，也不要每 boot 重试刷 WARN。
- **`activate()` 是设备依赖路径**：先查编译期白名单，再 `load_policies` 读真机 sysfs。
  写/改它的单测不能假设宿主机能通过（`fas_manager` 现有 3 条:588/:683/:694 即因此恒红）。

## 内核取证口径（minidump，2026-11-01 三次取证后定）

- **容器形态**：近三份 `SYSTEM_LAST_KMSG@….dat.gz` 解出来都是 **tar（PAX）**，成员
  `olc_get_log.txt` / `device.info` / `minidump.bin`(256 MiB) / `prop.txt`，**不是裸 ELF**
  ⇒ `ELF at 0x2db8 / shoff=0x40` 的段表解析无效，用全文件 grep + 结构体签名定位。解包目录 ~257 MB，
  中间 tar 用完即删。
- **printk ring 自毁（关键）**：s2idle 路径上的 `suspend ns / suspend cycles / resume cycles` 调试打印
  每轮 ~129 B、约 15 万轮/s ≈ **19 MB/s**，2 MB ring **每 ~106 ms 冲刷一次** ⇒ minidump 里只能看到
  崩溃前最后 **~100–140 ms** 的 s2idle 记录；"首条打印时间戳 − last pet = 6.2 s"是 **ring 深度产物，
  不是挂起路径耗时**（2026-10-31 报告曾据此误判，已订正）。
- **撞线判据**：只看 `bark Now − last pet at` 与 `vWdog-time since last pet` 的 tick 数——
  三次实测 Δ 6.348–6.390 s / tick 均 **206** ⇒ 30.82–30.90 ms/tick（32.45 Hz = 32.768 kHz/1024）
  ⇒ 客侧 `bark 20000 ms` 不生效，实际窗口 **≈200 tick ≈6.16–6.18 s**。栈指纹 `qcom_wdt_bark_handler`
  ← GIC ← `cpuidle_enter_s2idle+0x98/0xb0` ← `do_idle`（三份逐帧相同、跨两个内核构建）。
- **suspend cycles 时基不可靠**：不同 dump 内 ns/cycle 比值不同（1.9697 vs 0.16415 即 6.09 GHz）
  ⇒ 只用 sched_clock 差 + vWdog tick 反推，不折算成时间。
- **`prop.txt`/`device.info` 是重启后快照**：只能用于计数（`persist.sys.oplus.total_abnormalreboot_count
  = total_N_dump_N_pmic_N`），**不能**判崩溃瞬间亮/息屏或电量。minidump 只留少量用户态串，
  不能据某个 daemon 日志串缺失就断言该构建没装机。


## 息屏保活锁（wake_guard）契约（2026-11-01 落地）

- **是什么**：息屏期间向 `/sys/power/wake_lock` 写一把 `chiri_screen_off`，让 PowerManager 的挂起尝试
  因 `pm_wakeup_pending()` 吃 -EBUSY ⇒ 不进 s2idle ⇒ 绕开 vWdog ≈6.18 s 撞线窗口（sm8750 三份 minidump
  同型崩溃的唯一应用侧"挡"法；静默档/观测/降噪都只降概率）。代价：息屏不进 s2idle。
- **代码**：`src/chiri/wake_guard.rs`（`acquire/release/sync`，无需超时，启动盲释放清遗留）；
  调用点只三处——screen 事件、DOWN 停摆切换、`config_dirty` 热重载，统一表达式
  `enabled && !halted && !is_screen_on`。
- **开关**：`meta.yaml wake_guard_enabled`；**缺键按机型**（`common::default_wake_guard_enabled()`，
  仅 `matched_soc_config_name() == Some("sm8750")` 为 true）。只改模板不够：`sync_meta_snapshot`
  对合法 meta.yaml 原样不动，老安装不会补写新键。WebUI 开关在 SceneMode 下方，缺键回退
  `app.configRel.startsWith('sm8750/')`。
- **新增 meta 字段仍是"五处同步"**：`MetaYamlFile` / `ExternalMetaOverrides` / `chiri::config::Meta`(+apply) /
  meta.yaml 模板 / WebUI `contract/meta.ts`（META_FIELDS + WRITABLE_FIELDS + 布尔清单 + validateFieldValue）。

## 调度修复契约（2026-11-07，A02-16 之后）

- **EEM 成本口径 `k` = 忙核数**（`cpu_load_governor.rs` `[eem_k_busy]`）：`k = count(util > EEM_BUSY_UTIL 0.02).clamp(1, 本簇核数)`。
  **不要再用 `ceil(Σ clamp(util))`**：均匀轻载（6 核各 0.5）会被当成 3 核重载永久封顶，
  而 `supply(cap×k)` 同比例放大 ⇒ 判据永远欠供。取忙核数后判据收敛为 `cap(f) ≥ busiest`。
- **EEM 规划池 = 可行域交集**（`affinity.rs` `eem_allowed_cpus`）：每轮读 `GROUP_TOP_APP`/`GROUP_FOREGROUND`
  的 `cpuset.cpus` 取交集、再与在线位图求交，簇内无允许核即整簇剔除（变化打 INFO `affinity-eem-allowed`）。
  根因：`normal_fg_exclude_little` 把前台组收窄到 big∪prime，规划池却看全部在线簇 ⇒ 每轮发窗口每轮被摘。
  读 cpuset 带 **`EEM_ALLOWED_TTL=1s` 缓存**（写路径 160ms 一拍）；**写前还要与可行域求交**（`dst_mask = a.mask ∩ allowed`），
  只剔整簇挡不住「簇内部分核被排除」；空集回滚、不发 syscall，留 `act=place result=skip reason=dst_empty` 的 @A 帧。
  `clipped` 以**实际写下的掩码**为基准（被提前剔掉的核不算裁剪）。
- **升频速率闸判据前置**（`[up_fast]`）：`if !is_significant_jump && up_wait < up_rate_limit_ticks` 才拦；
  显著跳变直接 `up_wait=0; current_perf=target_perf`（CLG 写上限，抬上限几乎不耗能）。稳态无跳变逐位不变。
- **`activity-first` 是特调优先级的唯一例外**（`special_tuned.yaml` 第四段标志位）：
  `special_yields_to_activity(flag, dynamic_enabled, has_rule) = flag && dynamic_enabled && has_rule`
  （**三条件缺一不可**：activity 规则本身只在 `config.dynamic_enabled` 时生效，漏了它会在关闭动态模式时
  「先让位、再落到 global_mode」，白名单包丢特调）。命中显式规则时该 activity 走 app_modes 普通模式（带触摸升频），
  其余仍走包名回退；未声明标志位的条目行为不变。
- **全序** = FAS 白名单 > 特调(activity-first 让位例外) > activity 级 app_modes > 包名级 app_modes > global_mode。
- **1107 基线（用于下轮对比）**：up_wait 30.6% / up_wait→up 中位 478ms / place clipped 38.3% /
  被摘核 86.1% 在核 0-2 / 小核被摘率 79-94% vs 大核 2-14%。daemon 自耗 playback 档 p50 7.9% 单核。
- **`narrow_pid` 按 (pid, 簇) 分键**（2026-11-07 自审）：`HashMap<(i32, CoreGroup), Instant>`，`remember_narrow`
  超 512 条按到期清理；`note_narrow`（仅 EINVAL）与 `note_stripped`（回读缺簇）共用。单槽会被异簇覆盖。
- **`status.csv` 28 列**：末尾 `daemon_rss_kb`（`logger::self_rss_kb()` 读 `/proc/self/status` 的 VmRSS，失败 0）。
  **只作跨会话趋势**，单点不可判；WebUI `STATUS_COLUMNS` 已 28、解析兼容 27/28（旧归档 `daemonRssKb=null`）。
- **`best_placement` 两簇都供不上时取供给更大者**（`(false,false)` 分支，并列留先到者）：
  此前按成本取最便宜与文档不符；`cheaper_elsewhere` 有 `supply >= demand` 闸，生产行为不变。
- **自审轮验证基线**：单测 30 passed；离线 energy_model 40 / affinity_cpuset 19 / app_mode_activity 10 /
  clg_tuning 5 / diag_slice 6；WebUI vitest 107 passed（8 文件）。

## FAS 性能优先档 `[perf_lock]` 契约（2026-11-07 落地，首个使用者 = 终末地）

- **落点唯一**：`src/scheduler/fas/policy_mgmt.rs::apply_freqs`，`if locked { self.perf_index = ceiling; effective_perf = ceiling; }`，
  位置必须在 util 软封顶块**之后**、`thermal_cap` 压顶**之前**。FAS 全部降频路径（PID / steady decay / util 软封顶 / 开环 load_only /
  loading / post_loading / app_switch / 齿轮机 min(0.90) / floor-rescue / post-jank / damped cap）只改 `perf_index`，写频必经此处 ⇒ 一处收口即可全旁路。
- **`load_only = !locked && self.uses_load_control()`**：开环按 util 下探与锁频互斥，不能同时成立。
- **辅助闸**：`controller.rs::adjust_target_for_util` 与 `frame_pipeline.rs::apply_steady_decay`（`pub(super)`）在锁频档开头 return；
  这两处不改也不会掉频（最终仍被 apply_freqs 夹回），改的目的是不让内部状态漂移误导判读（如 target 60→57）。
- **缺省必须 false**：`fas_types.rs` 的 `#[serde(default)] pub perf_lock: bool`，只给掉帧敏感应用逐应用显式开；禁止为了让某档更稳而改缺省值。
- **单档配套**：`fps_gears: [60.0]` ⇒ `next_gear/prev_gear` 恒 None ⇒ 齿轮机升/降两段整段不执行（这是掐「降档加速压到 0.90」与「原生档翻面」的唯一手段）。
- **热限频仍生效**：`core_temp_threshold > 0` 时 `thermal_cap` 写在锁频之后，内核 thermal 也独立生效（`qos_clamped` 只告警不重写）。
- **热重载**：`reload_rules` 同步 `perf_lock`，由关到开才补一条 `fas-perf-lock` 日志（i18n `zh.ftl`/`en.ftl` 键位对称）。
- **验证基线**：`cargo test -p chiri --bin chiri -- perf_lock` 4 passed；离线 `scripts/tests/fas_perf_tier_regression.rs` 4 passed；
  全量 `278 passed / 3 failed / 1 ignored`（3 failed 为 `chiri::fas_manager::tests::` 既有环境依赖）。
- **未改**：`controller.rs::effective_perf_floor()` 的 `.min(0.45)` 硬顶仍在（`perf_lock` 绕过它；做「半锁」时才需改成可配字段并同步 `normalize()`）。
