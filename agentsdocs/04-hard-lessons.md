## [hard] 硬性约束

- 不修改 CI 构建流程（`.github/workflows/build.yml`），除非明确要求。

- eBPF 程序目标为 `bpfel-unknown-none`，改动需确保 CI 环境交叉编译通过。

- 保持 KernelSU/Magisk 模块规范兼容（`module/` 目录结构、`service.sh` 启动流程）。

- Yumi 调度兜底已删除（2026-09-22，用户指令）：`src/scheduler/` 只剩 FAS 引擎与 policy 探测工具（`config.rs` / `cpu_load_governor.rs` / `scheduler.rs` / mod.rs 的 thread/cfgwatch/ipc 接线均已移除），非 ChiRi SoC 不接管 CPU、只跑监控/WebUI/日志。共享层（`src/monitor/`、`main.rs`）的 `is_chiri_soc()` 分支保留作「仅监控模式」（非 ChiRi 采样 200ms 口径不变）。`yumi-ebpf`、`YUMI_SKIP_EBPF`、设备形态 `'yumi'` 是刻意保留名，勿当残留清理。

- 不允许在未经许可的情况下主动创建git提交

- 所有任务除例外外都需要调用 /token-efficient-coding 这个SKILL，如果找不到这个SKILL则终止任务并提示

## [lessons] 经验教训

- 含 `std::ops::Range<usize>` 字段的结构体别 `derive(Copy)`：CI（`-Z build-std` + nightly）编译 `common::CoreGroupRanges` 时报 E0204（字段不实现 Copy），即便标准库中 `Range<usize>` 实现了 Copy。按值 move 或显式 `clone()` 即可，用 `#[derive(Debug, Clone)]` 够了，不要加 Copy。

- 日志文件被删不能崩：`src/logger.rs` 用自实现 `SelfHealingAppender`（替换 log4rs `RollingFileAppender`），每次写入按路径 `create+append` 重新打开，`daemon.log` 被外部删除会自动重建；循环轮转与锁上锁全程 `Result`/剥除 poison，绝不 `unwrap`。别改回 log4rs 滚动追加器。

- FAS 帧投喂必须全量：fps_monitor 的 `latest_frametime()` 单条投喂会把 100ms 窗口内 6 帧（60fps）丢 5 帧，所有「帧数」常数（confirm/cooldown/decay 阈值、freq_hold_frames）的时间尺度被拉长 ~6 倍，PID/防抖/jank 全面钝化（平均帧↓、1%Low 崩塌）；且无新帧时同一条陈旧 delta 被重复投喂，静态 UI 上一条 heavy 帧 ~1s 即可刷出假 loading（perf 钳 0.60~0.70，纯 UI 卡顿）。现按 ProbeState.pending 队列逐帧 try_send 全量投喂，通道满时丢弃（EMA 可容忍，绝不阻塞 fps_probe）。

- FAS governor 快照/恢复：load_policies 把每 policy 的 scaling_governor 改写为 performance 配合 min=max 锁频，PolicyController 必须在 new 时快照接管前 governor、reset()（deactivate 路径）时恢复——否则 performance 泄漏给后续 CLG/akmode/系统调频，所有模式功耗拉满、低负载降频失效（用户实测「FAS 用一次之后功耗不降」的根因）。

- FasManager 复用路径不显式 reset_runtime（其为 pub(super) 引擎内部方法）：依赖引擎 app_switch_gap 语义——去激活时已 clear_game，复活后首个真实帧的帧间隔跨越失前台时长、必然超过 app_switch_gap_ms，引擎 handle_early_exit 内部自动 reset_runtime 并落 app_switch_resume_perf；手动补 reset 反而破坏该语义。

- 看门狗崩溃自愈 + 进程模型：`service.sh`（开机）/`action.sh`（手动启动）用 `setsid sh -c ... &`（setsid 不可用时回退 `nohup sh -c ... &`）拉起统一定义的看门狗循环，启动时把自己 PID 写入 `logs/watchdog.pid`。**两个分支都必须 `&` 后台化**——setsid 前台执行会阻塞拉起脚本直到 daemon 退出（看门狗 while 循环存活期永不返回，action.sh 卡在 stopped 之后、"daemon restarted." 打不出来的根因），且脚本退出时向作业补 `disown` 防 SIGHUP。循环内 `"$DAEMON"` 在前台运行，进程以任何方式结束（正常退出 / panic 展开退出 / 被信号杀）都会返回并重启；仅当存在卸载标记 `$MODDIR/.uninstalling`（`uninstall.sh` 开头 touch）或主进程二进制被删时才 `break` 退出。**崩溃退避（2026-09-15）**：每轮用 `date +%s` 量 daemon 存活时长，退出用时 <60s 判为异常短命（启动即崩），sleep 按 3→10→30→60s 递增封顶，活过 60s 或 `date` 不可用时回到 3s——此前固定 `sleep 3` 在启动即崩时会形成重启风暴（日志/IO 放大）。**daemon 侧哪些失败能触发看门狗重拉**（2026-09-15 核查）：main 线程 panic、monitor_core panic（main 末尾 `join().unwrap()` 把 panic 带进 main）、chiri 调度线程连续 panic >`SCHEDULER_IPC_RESTART_MAX`(5) 次后的 `exit(1)`、日志门限 `exit(0)` 都终止进程；**监控子线程**（screen/config/pid/fps/cpu/telemetry，经 `monitor/mod.rs::spawn_guarded` 派生）panic 后落盘并 `exit(1)`——此前它们是游离线程，panic 只死线程、进程照活，看门狗永远感知不到（fps 死后 FAS 无帧、CLG 超时退回原生调频）；正常返回（通道关闭等）不受 spawn_guarded 影响。仍未覆盖的是「daemon 硬挂（死锁/死循环）」：看门狗只监控进程存活，无存活超时探测。旧写法 `"$1" || exit 0` 在 yumi 崩溃（返回非 0）时会直接让看门狗退出、无法自愈，禁止复用。`service.sh` 开头的清理必须先按 `logs/watchdog.pid` kill 旧看门狗再 `killall chiri`（脚本被重新执行——模块热更新/管理器重载——时只 killall 会留下旧看门狗，3s 后与新看门狗各拉起一个 daemon，形成双实例并行写两份 devimp/status 日志）；`action.sh` 已有同口径清理。

- **热更新必须先杀看门狗再复制二进制，且复制结果必须显式校验**：`customize.sh` 热更新只 killall chiri 不杀看门狗时，看门狗的 3s 周期复活可落在 killall 与 `cp` 之间的窗口——`cp` 覆盖**运行中的可执行文件**会 `ETXTBSY` 失败，且 `cp -r ... 2>/dev/null` 把它静默吞掉：模块目录残留旧版本，热更新后手动重启调度（action.sh 拉的是 live 目录的二进制）仍跑旧版，直到重启设备被 KSU 的 `modules_update` 覆盖才固化。修复四件套：① 复制前按 `logs/watchdog.pid` 杀旧看门狗；② killall 统一 `-9` 并轮询确认 daemon 退出（pidof，≤5s，D 状态进程对 SIGKILL 也要等 IO 返回）；③ `core/bin/chiri` 单独 `cp` 并校验退出码，失败恢复配置备份、重启旧版服务保持调度连续并明确告知；④ **`MODDIR` 赋值后必须 `export`**——安装器环境可能已把 MODDIR 导出为 staging（modules_update），仅局部赋值子进程看不到，service.sh 的 `[ -z "$MODDIR" ]` 会继承错位路径（看门狗从 staging 拉起 daemon、日志/pidfile/锁写入 staging，KSU 清理后调度静默死亡）。顺带修正 `chmod 755 "$MODDIR/yumi"` 的错误路径（二进制在 `core/bin/chiri`）。**热更新路径（成功与失败）都必须以 `exit 1` 结束**——exit 0 会让 KSU 把模块归为「待更新」，重启前 Action/WebUI 全部禁用（脚本 MSG_HOT_UPDATE_HINT 注明的预期行为）。

- WebUI「关闭调度」而非重启：`contract/daemon.ts::stopScheduler` 先按 `logs/watchdog.pid` kill 掉看门狗（防止其把主进程再拉起），再 `killall -9 yumi`（缺失时回退 pkill），实现彻底停止调度；恢复需点击模块 Action（`action.sh` 手动启动）或重启设备。不要在 ksu.exec 里用 `nohup ... &` 后台拉起——`ksu.exec` 返回会清理执行 shell 的进程组，直接拉起会被一并杀掉；要启动调度一律调用模块自身的 service.sh/action.sh（内含 `nohup`，且 disable_boost 幂等）。

- 模块热更新必须 `exit 1` 收尾：customize.sh 的热更新分支把新文件直接 cp 进已安装模块目录 `/data/adb/modules/chiri` 并重启服务后，必须轮询确认 daemon（`pidof`/`pgrep yumi`，最多 \~6s）存活，然后无条件 `exit 1` 按报错退出。若 `return 0` 让安装器继续"完整安装"，会再覆盖一遍模块目录并写 update 标记，管理器随即识别为"模块更新"提示重启、隐藏 WebUI/Action；`exit 1` 使安装器按失败中止（清理暂存目录、不碰已热替换的目录），管理器不感知更新。管理器显示"安装失败"是预期行为，脚本内已双语提示；服务未启动时同样 exit 1 并提示重启设备走完整安装。

- 没有要求或引用的情况下默认视所有log和csv分析文件都是过时的、错误的、具有误导性的，不能参考或用于分析。但是如果引用或指出了需要参考文件夹则需要查看里面的所有文件，无论你认为有没有必要。

- **暂停功能用 `// [PAUSED]` 注释入口/触发点而非删除**：需求方明确「功能以后大概率还要加回来」时适用（实例：息屏节电 doze + scenemode，2026-09 暂停后同日恢复，恢复 = grep "PAUSED" 解开标记块）。变量声明随触发点一并注释、调用点实参保留恒值以防扩散改动；若需求变为「完全移除」（如 FAS 息屏省电），则删除代码并同步清理 i18n key 与文档，不留注释残骸。

- **shell 命令读文件前必须守卫**：`wc -c < 缺失文件`（或任何 `< file`）的输入重定向错误由 shell 直接打到 stderr——命令上的 `2>/dev/null` 覆盖不到，真机 mksh 会把整条命令判为失败（实例：导出进度轮询把「产物尚未生成」误报成「查询导出进度失败」）。统一写法：`[ -f path ] && …` 守卫再读，变量给初值 0；WebUI 侧对「非零退出但输出可用」宽容处理。

- **写 sysfs 但不属于任何 governor 的调用面必须自己接停摆语义（2026-09-20）**：DOWN 的释放清单只能回收「被某个对象持有快照」的接管（CLG/特调/FAS/fast_lock/affinity/core_ctl/scenemode），而 `CpuScheduler::apply_system_tweaks` 这类「一次性下发后就没人管」的系统调整（cpuidle governor、IO 调度器、`cpu_boost` 输入升频、内核 `Sched` 参数）不在任何快照链里 —— 曾被 `config_watcher` 在停摆期反复重放、启动时也先于 DOWN 判定下发，表现就是「开了 DOWN，调度仍在被改」。判定方式：新增任何 sysfs 写入前先问「它被谁的 release 收拾？」——答不上来就自己接 `crate::down::is_down()` 门控 + 快照/还原。另：跨线程的下发/还原要共用一把互斥闸并在闸内**复查**标志，只在外面判一次会留下竞争窗口。

- **整文件覆盖（Write）前必须确认目标不是已跟踪文件**：本轮为嵌 WebUI 资产直接整文件重写了已有 `build.rs`，丢掉 eBPF 构建与 `assert_required_configs` 断言——cargo check 静默通过（ebpf 占位产物已存在、断言消失不会报错），靠 diff 审查才抓回。写整文件前先 `git status` 该路径 / 读原文件；向既有文件加功能一律局部 Edit。cargo/类型检查通过 ≠ 构建脚本语义完整。

- **「field is never read」当 bug 查，别当冗余直接删（2026-09-22）**：`fast.rs` 的 `LockedPolicy.target` 报 never read，真相是 `tick()` 防篡改重写误用了 `hw_max`——frozen（锁最低频）每 5 秒被拉回最高频，功能等于没有。之前的审查两轮都只看到字段存在就放行了。字段「写了没人读」和「读的地方读错了对象」在警告里长得一样：先找这个字段的赋值语义本该被谁消费、那个消费方实际读的是不是另一个字段，再决定是删字段还是修消费方。

- **口径类改动必须代码表达式与文档双向核对（2026-09-18）**：功耗采样门槛 `sample_ok` 的表达式与文档/记忆口径写反并存续 17 轮——记忆把错误表达式当口径记录。沉淀口径时同一句话拿代码算一遍；发现矛盾以代码为准并同步修文档。「存在 ≠ 有效」这类语义也勿过度解读（用户纠正过）。

- **normalize 会静默抬参数**：headroom clamp 下限曾是 1.0，任何 <1 的收紧值（如 0.95）被 normalize 静默抬回 1.0，省电方向参数等于失效且无告警（2026-09-20 放宽到 (0.5, 2.0)）；同理热压制豁免档 `free_above` 被 normalize 抬到与 soft_perf_cap 相等 → 压制带为空、cap=85 时段零压制（2026-09-23 修复：normalize 对「过低或相等」统一抬到 soft_perf_cap+0.10）。新增 normalize 约束时先想清楚「合法的省电方向值」会不会被它吃掉。

- **is_fast_lock 六入口收敛（2026-09-22）**：`is_fast_lock(mode)` 必须覆盖 vector|frozen 全集，六个接管入口（启动/亮屏/ModeChange/ConfigReload/DOWN 退出/panic 自愈）漏一个就静默失效——曾漏 ConfigReload 分支（`== "vector"` 没算 frozen），配置热重载即 fast_lock.release()。核对手段：`git grep '== "vector"' src` 应为空。

- **锁频写序**：升频先写 max 后写 min、降频先写 min 后写 max——反序出现 min>max 被内核拒绝、写失败后状态静默失效（frozen、PowerBase 接管首拍都踩过：接管时立刻写 min=max=最高频且 perf 同步 1.0，否则首 tick 走升频写序时 max<min 被拒 → perf 不前移 → 永久静默失效且零报错）。

- **写频滞回三断链（2026-09-24，全部 cargo check 通过仍错）**：① 死区吞写 × hold 用精确比较 → 假 down 循环（hold 改用死区比较，`deadzone_band()` 是唯一换算点，对称假 up 同修）；② CLG up 判定加 1e-6 epsilon（α≤0.5 慢升时 1 ulp 舍入停滞 → 假 up 无限累加）；③ last_failed 悬挂：write_freq 值去重早退路径要补清失败标记。cargo/类型检查通过 ≠ 行为等价，浮点比较与早退路径要单独过一遍。

- **组绑定释放臂必须覆盖压力窗口外（2026-09-24/25）**：新增任何组绑定触发条件时，同步检查「触发条件消失后」的释放侧兜底——Busy 绑定回落曾被 key_pressure 门控在 promote 路径内，压力一落线程带收窄掩码滞留 95 分钟（详见 03 亲和小节）。

- **有副作用的读不能挂高频采样（2026-09-22）**：每秒读 Adreno gpuclk/devfreq cur_freq 会唤醒低功耗态 GPU，playback 实测 +47% 功耗（1.76→2.59W），CPU cpufreq 读取则廉价。定位手法：调参没变而功耗涨 → 涨的只能是新加的东西（控制变量）。

- **Rust 块注释里写 sysfs 通配路径会吃掉注释配对（2026-09-26）**：路径中的 `/*` `*/` 与块注释定界符冲突（screen_detect.rs 实例），注释里的通配路径要改写成不含 `/*` 的形式。

- **meta 新字段漏同步 = 配置死循环（2026-09-22）**：`MetaYamlFile`（deny_unknown_fields）缺新字段时，WebUI 写入该键 → 整文件判非法 → sync_meta_snapshot 内嵌默认整体覆盖 → 用户 oplus/双电芯设置被抹、循环往复。同步点清单见 02「新增字段要同步」，漏一处即此后果。

- **devimp main_ 的 batt_i 列疑似整数化（值域仅 0-3，2026-09-24）**：「batt_i==0 严格排除」会误删过半样本、P_avg 系统性高估（aweme 3.35 vs 真值 2.14W）——**功耗一律以 status.csv 的 charge+batt_power_w 为准**。

- **PowerShell 管道陷阱（2026-09-17）**：svelte-check 等命令输出经 `2>&1` 变对象数组，直接管道 Select-String 会漏——先存变量再筛（`$o = & node ... 2>&1; $o | Select-String ...`）。

- **js-yaml 5.x 无 default 导出**（只有命名导出 `load`），且须卸载 `@types/js-yaml`（v4 的 `export =` 会掩盖 default 导入类型错误）。

- **CSS 覆盖三坑**：别靠「写在后面」覆盖前文规则（同权重后者胜，要用后代选择器提权重）；类选择器自带 `display` 时会压过 UA 的 `[hidden]` → 显式写 `.xxx[hidden]{display:none}`；原生 `<select>` 自绘弹层不吃继承色，`option` 也要显式背景+文字色（深色页白底白字）。

- **ak-ui 深坑（2026-09-18/19 实测）**：`--ak-progress-signal/value` 默认值声明在 `.ak-progress` 根元素上，裸用 track/fill 取不到 signal 变量 → background invalid → 进度条透明（两次「修宽度」无效的真根因）；独立页面（analyze/）应 import tokens.css 自建样式，别拼组件类。走查用 agent-browser 时 `wait --load networkidle` 因 Vite HMR 长连接永不完成——固定等待 + eval 读 DOM。

- **git 尾注与历史改写（2026-09-26 实录）**：commit message 尾注 `Co-authored-by:` 会被 GitHub 解析计入 Contributors（本地 shortlog/log --author 查不到）；`git filter-branch --msg-filter` 改写后**全部 commit hash 作废**（tree/parent/作者/日期不变），其他克隆需重 clone 或 reset --hard，dependabot 旧链等其自行 rebase。

- **A/B 纪律（2026-09-22/24）**：采集态自身开销不可忽略（logd 待机占 launcher 场景 ~18%、视频 ~14%）——任何功耗 A/B 必须同 dev_record 状态；devimp 无 charge 列只能 `batt_i<0` 滤充电，不滤 39W 快充直接进均值；跨批次（不同日/固件/机型）数据不可比。

- **文档里的数字必须从常量定义核对**，不能凭印象写（CLG_STALE_MAX 实为 5s，初稿误写 30s；TUNED_COOLDOWN=300s 已核）。

- **未提交的工作区改动（尤其注释掉的代码）可能是用户重构中间态（2026-09-16）**：报错指向用户正在编辑的文件时只报告、不动手；顺手修之前区分「我改的/工作区已有的」，评估 ≠ 批准开工。
