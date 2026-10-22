## [chiri] ChiRi 调度子系统

以下子系统仅在命中 `CHIRI_SOC_HINTS` 的 SoC 上生效（未命中不接管 CPU，原 Yumi 兜底已删）。全局统一默认调速器（`{soc}/soc.yaml` 的 `[cpu_governors]` 按从左到右优先级与真机 `scaling_available_governors` 求交选定，无该段或全不可用 → `schedutil`）：CLG 与 akmode 均把内核调速器写为该值（各自 `init_policies` 时写 governor、release 时恢复快照）。选型在 schedule 启动时一次算定（`common.rs [governor]`，日志 key `governor-selected`），特调 / FastLock / 残留清理共用同一结果；启动期离线（之后才上线的簇）与停摆期被跳过的 policy，在写入点按同一口径惰性补算。

### 特调（akmode）

白名单数据：

- 白名单数据在独立文件 `src/chiri/special_tuned.yaml`，经 `include_str!` 编译进二进制（common.rs 的 `parse_special_tuned` 解析、OnceLock 缓存），用户/WebUI 不可修改。

- 格式：每行 `匹配器:模式列表(逗号分隔):优先回退模式`。匹配器支持精确包名与 `re:` 前缀正则（忽略大小写用 `(?i)`，匹配器内不能含 ':'）；`special_tuned_entry(pkg)` 先精确匹配（文件顺序）、未命中再按正则条目匹配。每项含可用模式列表 `modes` 与优先回退模式 `fallback`（用户未显式配置则采用 `fallback`）。

- 当前条目：明日方舟国服 `com.hypergryph.arknights`、日服 `com.YoStarJP.Arknights`、正则兜底 `re:(?i)arknights`（覆盖台服/Mod 变体），模式均为 `akmode`（fallback 同）；**playback 组**（视频播放，2026-09-17 8550 日志分析新增）：`tv.danmaku.bili` + `re:(?i)(bilibili|danmaku\.bili)`，模式 `playback`。**daily 组已于 2026-09-17 晚上机实测后移除**（桌面/聊天/工具属高频短交互，特调收益低、侵入高——「不是所有程序都需要特调」；移除条目留档在 special_tuned.yaml 注释里，特调只保留长稳态场景）。新增特调模式 = 本表加条目 + `normal/tuned_profiles.yaml` 的 `tuned_profiles` 段加同名参数组，无需改 .rs。

- main.rs 启动时把精确条目导出到运行时文件 `special_tuned.yaml`（每行 `包名:模式列表(逗号分隔):优先回退模式`，正则条目无法按包名精确查找故不导出）并 info 打点。导出仅 `is_chiri_soc()` 下发生，非 ChiRi 机型不生成该文件。

生效范围与门控：

- 特调体系仅限 ChiRi：`determine_mode` 先判 `is_chiri_soc()`，非 ChiRi SoC 上特调映射一律回退全局模式。

- 只在 chiri 的 `Config` 挂载独立特调字段 `akmode`（缺省段：游戏特调兼未注册模式的回退）与 `tuned_profiles: HashMap<模式名, SpecialTunedConfig>`（**特调参数组**，2026-09-17 加：`normal/tuned_profiles.yaml` 的 `tuned_profiles:` 段，`get_tuned_profile(mode)` 按模式名取、缺省回退 `akmode` 段，**全部调用点唯一的取参入口**——不要再加第二个取参方法）；不要注册进 `get_mode`（只认 CLG 常规模式）。

- 白名单应用始终进特调：前台命中 `special_tuned_entry()` 就返回特调模式，不管 app_modes/global_mode 配了什么（`determine_mode` 开头直接判定）。rules.yaml 里给该应用配的普通模式只作为特调起始档（scheduler 侧 `get_ak_initial_tier` 识别）。

- 非白名单应用的模式优先级仍为用户 `app_modes` > `global_mode`；后端门控：非白名单包名映射到特调模式时 warn 并回退 `global_mode`（`app_detect.rs` 的 `determine_mode`）。

调度行为：

- 特调是完全独立调度：`src/chiri/tuned.rs` 的 `TunedGovernor`（akmode / playback / daily 共用）与 CLG 完全解耦。前台为白名单应用时由 `mod.rs` 的 scheduler_ipc 先 `cpu_governor.release()` 再 `ak_governor.init_policies()` 接管，退出前台反向释放。**全部特调模式共用这一套连续控制**（每 40ms 按组内最大占用 × headroom 直接算上限，升频立即执行），差异只在参数组；**`boost_affinity`**（参数字段，默认 true）= 是否走 boost 类亲和（cpuset 收窄 big+prime + core_ctl 保大核常在线）——游戏保响应 true，省电型特调（playback/daily）必须 false：保大核常在线与「贴负载降频」相反（空转漏电），且会把前台重线程钉到大核组被低上限压住。`is_boost_mode` 命中的特调再经 `tuned_boost_affinity()` 二次过滤。**`util_smoothing`**（参数字段，默认 1.0=不平滑）= 决策负载 EMA 系数：抖动负载下「升频立即执行」会被瞬时尖峰反复推高上限（降频又被 hold 拖住），上限均值反而高于带平滑的 CLG（2026-09-17 日志回放证实），现值 playback 0.25、akmode 0.85（2026-09-22 扫参定值，以 tuned_profiles.yaml 为准），游戏保持 1.0（响应优先）；daily 组已整体移除。**降频计时重置语义（同日修复）**：down_target 只在目标**明显回升**（> down_target + hyst）时重置计时，下探/微调只更新目标——原「档位不等即重置」在抖动下让上限卡高位降不下来（回放中平滑 util 都降不动），连续控制退化成「只升不降」。

- 特调模式下息屏保持 akmode 接管（akmode 已统一 schedutil，息屏随负载自然降频省电）；非特调走 CLG doze → 超时 scenemode（见「息屏省电与屏幕状态」——2026-09 曾短暂暂停后**已恢复**，屏幕状态正常驱动息屏/亮屏切换；旧的「[已暂停] 屏幕状态不驱动调度」描述已删除）。

- **模式家族（2026-09-17 重申）**：CLG = reduce/default/boost（兜底档；2026-09-16 由 powersave/balance/performance/fast 改名，id 与 feature.yaml 段名/rules global_mode/current_mode.chr 同名，显示名走 i18n `mode.*`）；**stardust = scenemode（独立息屏轴，不产生模式值；2026-09-18 起在 UI 家族体系注册 `mode.family.stardust`——不管实际运行中看不看得到都占位，不并入 CLG）**；**down（停摆布尔）是独立家族 `mode.family.down`（kind `'down'`），不属于 stardust**；**rhine = vector/contingency/babel/frozen（仅实验室**，rhine.chr 经 global_mode 覆盖驱动）。CLG 档位语义：reduce 升频门槛最高、上限压低（2026-09-26 8550 现值 up_threshold 0.90、perf_ceil 0.60）；default 为默认兼参考基线（up_threshold 0.88、perf_ceil 1.00，0923 起不封顶）；boost 升频更激进（up_threshold 0.55）。**档位参数随调参迭代，真值只认各 feature.yaml，文档不维护第二份数字**。档位由 rules.yaml 生效模式决定（明日方舟 app_modes > global_mode）；特调期间固定应用；所有档位都能使用硬件最高档位。

- 档位差异仅在升降频策略参数和防抖等待（wait_ms，每档可不同）。核心组区间随命中 SoC 变化，统一在 `common::chiri_core_ranges()`（8550 little 0-2 / big 3-6 / prime 7；8475 0-3/4-6/7；8998 0-3/4-7 无 prime），akmode 与 CLG 触摸升频共用。每组独立 up_core_count/up_util_percent/down_core_count/down_util_percent：核心数为组内绝对个数，yaml 写整数，0 = 组内任一核心命中即触发，写大值如 64 = 关闭该方向判定；占用率写整数百分比，加载时转 0..1。

- 动态限频（schedutil + 负载驱动升降 max）：`TunedGovernor` 激活时写 schedutil、min 压到硬件最低、max 设为硬件最高。`on_load_update`（特调 40ms tick）用当前档位策略参数按核心组判定升降，升频优先：升频 = 任一组内达到 up_core_count 个核心 util > up_util_percent；降频 = 任一组内达到 down_core_count 个核心 util < down_util_percent（达到 = 组内核心数 >= core_count，即配置值就是绝对个数）。统计口径：util 恰为 0.0 的核心（离线与整窗空闲的在线核均为 0.0、不可区分）不计入升频 over、但计入降频 under——空闲即低负载，避免挂机/息屏时永不降频。升频前检查实际频率（scaling_cur_freq）是否已达当前设定的 max（schedutil 余量），达到才在频率表中升一档；降频直接把 max 降为当前实际频率对应档位（`read_cur_freq` 后 `partition_point` 找 <= 实际频率的最高档，实际不可读回退降一档，绝不高于当前 max），max 上下限均为硬件上下限。升降频带 wait_ms 防抖（升降后 `after_change_duration_ms` 内减半）。CLG 与 akmode 同构：min 压硬件最低、只调 max。

- 特调参数独立成文件（仅嵌入 normal/，非处理器绑定）：`module/config/normal/tuned_profiles.yaml` 定义缺省段 `akmode` + 按模式名分派的 `tuned_profiles` 段，经 `common::embedded_tuned_profiles_str()` 编译进二进制，不放在默认 `feature.yaml` 里。原处理器目录 `{soc}/akmode.yaml` 绑定已在 4cb4d97 重构中移除（磁盘已无该文件，勿再加回）。嵌入内容解析失败时 `set_special_tuned_available(false)` → 特调不可用、白名单应用回退 CLG（不再保留旧值用默认参数接管 CPU）。

- 特调接管失败冷却：`TunedGovernor::init_policies(mode, cfg)` 返回 `bool`（无可用 cluster 即 false）。scheduler_ipc 在三个特调接管入口（亮屏恢复/ModeChange/ConfigReload）检查返回值，失败时置 `tuned_cooldown_until = now + 300s` 并 warn 打点 `scheduler-tuned-cooldown`，5 分钟内该特调模式不再触发、改由 CLG 接管（冷却中特调模式名保持，息屏 doze/亮屏恢复均走 CLG 分支），冷却结束后经 ConfigReload 或下次 ModeChange 自然恢复重试。

WebUI 侧：

- 设备形态两态：`contract/sources.ts::deviceKind()` 依据 active_config 是否指向处理器子目录判定 `chiri` / `unknown`（非 ChiRi 与读不到一律 unknown，界面显示「无法判定」）。状态落在 `src/state.svelte.ts`。

- 特调/FAS 在 WebUI 为只读标注：`data/whitelists.ts` 解析白名单，仅 `chiri` 机型显示「特调 / FAS」标签（其余不显示）；不提供专属特调模式选项、不做特调清理/重扫修复。应用性能模式已禁止在 WebUI 指定（rules.yaml 只读），应用列表仅展示特调/FAS/现存 app_modes 标签，提供「重新扫描」按钮（扫描中禁用防并发）。

- 当前模式读取与自愈：`contract/sources.ts::readCurrentModeRaw()` 读 `current_mode.chr`（内容为模式名原样字节、无换行无空格）；守护进程启动时写一次、模式切换时写、并常态每 5 秒**无条件**重写一次（两套 scheduler_ipc 各一份实现），因此**不能用 mtime 判断模式变化**；守护进程停止后文件是陈旧值，界面须结合存活探测呈现。文件为空/缺失时界面显示「未知 · 尚未产生模式记录」，与「模式名不在已注册档位内」区分开。

### FAS 帧感知调度（单实例 + 延迟进出，ChiRi 专属）

- **当前所有权与叠加契约**：游戏 owner 与检测前台分离。`FrameUpdate` 携带采集时的 PID/generation，旧队列不在出队时重标。反馈暂停/恢复、新实例或进程替换建立新帧会话；探针摘挂时 drain 并用同源 `CLOCK_MONOTONIC` 截止界标过滤迟到事件。帧反馈暂停期间仍是唯一算力 writer，按各 policy 实际 CPU 集合的负载调频，不把 overlay 前台线程 util 或低 FPS 解释成游戏算力不足。恢复清空反馈状态并经过 12 个有效新帧预热；无帧超过 500ms 回退 load-only。温度刷新同时由负载驱动，热安全上限优先于地板/hold。以下历史段中的「频率停在最后状态」不再适用于延迟退出期。
- **混合前台焦点算法（2026-10-16 起，用户定名）**：`src/chiri/fas_focus.rs` 的 `Focused`/`LoadOnly`/`Grace` 三态决策。用户可见行为等价于「这个前台现在该不该拿帧驱动」，`mCurrentFocus` 只是输入之一而非判据本身。**帧率优先规则**：窗口证据判为 `LoadOnly`（即 `VisibleUnfocused`）时，若帧源仍在正常供帧（`FasManager::frame_feedback_active()` = `!uses_load_control()` = 帧反馈使能 + 已过预热 + 最近一帧未超 `FRAME_FEEDBACK_STALE` 500ms），维持帧驱动、不降级为仅调频。动机：游戏上盖帧率悬浮窗/输入法时游戏窗口不再 `Focused`，但帧数据仍可取，降 `LoadOnly` 会关掉帧驱动、被用户感知为「FAS 异常退出」。**边界**：本规则只作用于 `LoadOnly`；`Hidden`/`Unknown` 仍走 `Grace`，「有叠加画面时退出 FAS 是预期行为」语义不变。**两处实现硬约束**（写错即规则静默失效）：① 判据必须在「暂停反馈」**之前**读取，否则 `set_frame_feedback_enabled(false)` 使 `on_frame` 早退、`last_frame_at` 不再更新，判据恒 false 且不自愈；② 帧源仍供帧时不得抢先暂停（暂停会 bump generation 使本次窗口观测失配 → `evidence=None` → `Grace`，即「帧率可用却退出」）。判据不用 `frame_source().frame_feedback` 单字段：load-only 会话该字段也为 true，会把「正在仅调频」误判为「能取到帧率」。
- **窗口证据**：cgroup 是低成本候选，不是可见性证明。可疑前台变化按需启用单后台 **两条** `dumpsys` 查询并合并：`-t 1 window`（提供 `mCurrentFocus`/`mFocusedApp`）+ `-t 1 window windows`（提供 `mObscured`/`mViewVisibility`/`mHasSurface`）。两条**互补**，只用后者会让焦点判定恒不可达，只用前者会缺遮挡字段而连 `VisibleUnfocused`/`Hidden` 都判不出。段间以 `== CHIRI-DUMPSYS-SEGMENT ==` 分隔，按段取字段。正常稳定前台不扫描。查询最小间隔 3s；**每段各自** 800ms 超时（不共享预算，否则第一段耗时会挤掉第二段）；单段输出上限 512KiB；错误退避 5s、Unknown/不支持退避 15s，证据从查询开始时计龄且仅 5s 有效。请求绑定 owner PID、generation 和 raw 前台，revision 防止旧/乱序结果回填。可见且 `mObscured=false` 的失焦窗口保持 owner/load-only；隐藏、格式缺失、失败或过期仅使用有界退出宽限，不无限续期。返回确认在模式去重与 deadline 到期前处理，离开请求幂等且 pending 目标随最新前台更新。分屏、小窗、PiP 依靠相同窗口证据，不凭包名分类。**ROM 字段支持已由 2026-10-16 真机验证**（8550/PHB110，ColorOS），详见下条；**真实遮挡仍需更多设备验证**。
- **解析兼容（2026-10-16 真机实测补充）**：`window_visibility::parse_windows` 必须容忍 ROM 差异，三条已实测入测：① `window` 段的 `Window #` 有两种格式 —— `Window #N: WindowStateAnimator{…}`（无组件名）与 `Window #N Window{token u0 pkg/Act}`，各 26 条并存；② `mSession=Session{…}` 是**行内**出现的（`mDisplayId=0 taskId=851 mSession=… mClient=…`），不能按行首匹配；③ AID 形式 `u0aNNNNN` 与同块 `mOwnerUid=NNNNN` **必须归一到同一 uid**（AOSP 文本形式里 appId 已是最终值，`android_uid` 不得再加 10000，否则两者恒不相等 → 所有窗口落 `unknown_owned` → 恒 `Unknown`）。**教训**：合成 fixture 的字段布局会掩盖真实 ROM 形态，两处旧 bug 都因 fixture 恰好写成行首/恰好没触发偏差而长期逃过测试；新增解析逻辑一律用**真机原文**做夹具。

- 白名单与模式判定：`module/config/normal/fas.yaml` 精确包名→配置名；`determine_mode` 优先级 = FAS 白名单 > 特调 > app_modes > global_mode；rules.yaml 中 "fas" 映射视为非法（`app-detect-fas-rejected` / `app-detect-fas-global-rejected` 告警回退）；`fas_available()` = 白名单非空且至少一个应用配置解析成功（fps_monitor/FAS_FG_UTIL 联动门控见「架构与事件流」）。

- **单实例 + 延迟进出（2026-09-17 重构，原多实例已废弃）**：`src/chiri/fas_manager.rs` 的 `FasManager` 同一时刻至多一个白名单应用被接管（`instance: Option<FasInstance>`）。白名单前台 → `activate`（FasController.load_policies 快照频率 + **GovernorGuard 把各 policy 的 `scaling_governor` 切 performance**，原值快照待恢复）。**governor 的两个写者 2026-09-28 统一走 `utils::write_sysfs`**（写前 `enable_perm`、**不**在收尾 chmod 0444、失败读回校验即算成功）：原引擎侧用 `try_write_file`，写完把节点留在 0444，随后治理层 `GovernorGuard` 的裸 `fs::write` 必然 `EACCES`——全部已收 daemon.log 里它的成功行（`governor-switched`）**0 次**，FAS 期的 performance 实际只由引擎写入、guard 从未拿到快照（退出时也不做恢复）。**凡 sysfs 结点写入一律走 `write_sysfs`**，普通配置文件才用 `write_to_file`/`try_write_file`；失去前台不立即退出 → `request_delayed_exit` 进入 **延迟期**（`FasRulesConfig.deactivate_delay_secs`，**缺省 2026-10-16 由 15s 改 3s**（用户定：非白名单前台接管时 mode 残留 fas 污染归因，3s 足以覆盖白名单应用间快速切换），normalize 夹 1..=600，`mdocs/fas-example.yaml` 可配）：mode 保持 fas、FAS 仍持有接管（频率停在最后状态、governor=performance），期间切回白名单应用由 activate 无缝续期；到期由 1s 遥测块的 `tick()` 完成退出（reset_all_freqs + clear_game + governor 按快照恢复），并按延迟期记住的目标模式（`pending_mode_after_fas`，fas→X 的 ModeChange 被延迟时记录）重新接管——contingency/babel 走 apply 的 lab 分支，其余走 `apply_mode_takeover`。ModeChange 的 fas→非fas 分支因此**不再立即切换**（拦截在 mode_clone 更新之前，避免「文件写 default、FAS 还持着频率」的中间态）。C5 收尾 deactivate_all（DOWN/panic/进程收尾/fas_enabled=false 热重载）立即全退。事件路由：FrameUpdate/SystemLoadUpdate/温度只喂活跃实例。

- **governor 残留 performance 与 reset() 的快照陷阱（2026-10-20 修）**：`src/scheduler/fas/policy_controller.rs` 的 `reset()` 原按 `orig_governor` 快照无条件写回，而该快照若取自 GovernorGuard 已切 `performance` 之后，写回即把 performance 持久化——表现是新一轮启动仍报「检测到残留 performance 调速器」。现改为：快照值 `trim() == "performance"` 时改走 `common::cpu_governor_for_policy(policy_id)` 回落到该 policy 的默认选型，并打 i18n key `fas-governor-reset-performance`（`module/config/i18n/{zh,en}.ftl:269`，中英对称，args = `pid` / `governor`）。判据 = 后续 `daemon.log` 不再出现残留告警。**根因边界**：这是「快照必须早于切档」这一时序契约的兜底，不是性能问题——FAS 期 performance 本就由引擎写入（见上条写者统一），该修复只防泄漏持久化。

- 同模式热切换：新增 `DaemonEvent::PackageSwitch { package_name, pid }`（app_detect 主循环在模式不变且 ChiRi 且前台包变化时发射；非 ChiRi 机型不发射）。fas→fas 切换序列 = deactivate_active → activate → fas_affinity_hook；另有 1s 遥测兜底（事件丢失/启动即 fas 自愈：非活跃且冷却外且白名单命中 → 三 governor release 后 activate；启动残留 fas 模式且前台非白名单且三 governor 均不活跃 → CLG default 自愈）。兜底块仅亮屏时执行——息屏时 FAS 必须保持释放，否则会把息屏前旧包名拉回 FAS、绕过 CLG doze。

- FAS 模式下 CLG/特调/vector 全部暂停（三 governor release 后接管）；激活失败（load_policies 后无可用 policy）→ 300s 冷却（`FAS_COOLDOWN`，镜像 TUNED_COOLDOWN）+ CLG default 回退；进入 fas 的回退分支也先释放三 governor（可能从特调/极速切入）。

- **前台 util 口径（2026-09-23 修正）**：TGID 主路径 `compute_tgid_util` 以 adj = raw + pending 差分算 util（与线程级降级路径 `compute_thread_level_util` 同口径，基线存 adj）；旧写法「raw 差分 + 当前 pending」把上一轮 pending(t0) 重复计入（恒等式 consumed(t)=raw(t)+pending(t)），前台 util 系统性高估、FAS 输入失真，勿改回。

- 移除的调度（FAS 活跃期间豁免）：ChiRi 热保护仅当 `mode=="fas" && fas_mgr.is_active()` 时跳过（fas 模式但实例未活跃——息屏已释放/冷却/初始化失败——照常生效，否则 CLG doze 期间失去热保护；FAS 活跃时温度由 FasManager 每 3s 独立读传感器喂引擎内部限温，endfield 配置 core_temp_threshold=0 即关闭；1s 遥测温度读数保留）；触摸升频不参与（fas 非 boost，`is_boost_mode` 不含 fas）；scenemode 进入判定按 `!fas_mgr.is_active()` 门控（FAS 活跃即不进入——息屏保持接管后 fas 模式天然屏蔽 scenemode，FAS 失效后正常进入）；config_dirty/ConfigReload 的 CLG 分支对 fas 模式守卫（FAS 配置编译期嵌入静态）。

- **2026-10-05 FAS 日志判读**：① FAS 与 CLG 热阶梯脱节——FAS 只有电池 45℃ 单点护栏，CLG 的 41/43/45℃ 三级 cap 完全不作用；lolm 的 FAS 段有 53.6% 时间落在中档温区，却只有 4.6% 触发 45℃ 护栏。② FAS 段 snap 的 `cur/max=100%` 是 `min=max` 双写的构造结果，**不是锁 hw_max**；当时实频仅约 hw_max 的 55%（little 1498 / big 1930 / prime 1979 MHz）。③ 当时 15s 延迟退出期内 mode 仍写 fas，微信 / launcher / TIM 因而被记成 fas 并吃 FAS 频率（8.1W / 7.5W / 6.4W）；该延迟缺省已于 2026-10-16 缩到 3s，15s 是历史日志判读值。

- 息屏释放已完全移除（2026-09，原 C4 doze 更早已废弃删除）：原 `ScreenStateChange(false)` 在非特调分支统一 `ak/vector release` + （fas 活跃时）`fas_mgr.deactivate_active()` → 走 CLG doze → scenemode 全局接管。现 FAS 与屏幕状态完全解耦：**FAS 激活/去激活仅由前台包名驱动**，息屏保持接管（fas 活跃时跳过 doze）、boost 布局全时段生效、1s 兜底与 FrameUpdate 喂帧无 is_screen_on 门控、ModeChange 息屏 fas 特殊分支删除。仅 FAS 失效（初始化冷却/异常）后息屏才走 doze → scenemode：亮屏恢复分支的 fas arm 对失效场景激活 CLG default（唯一恢复路径），1s 兜底在冷却结束后重新激活 FAS（activate 复用保留实例 + apply_freqs，activate 前 release 全部 governor，且若 scenemode 激活先恢复全部在线核）。勿改回「息屏 enter_doze 写低频锁、亮屏 exit_doze 恢复」：锁屏前台无帧事件时 apply_freqs 恢复路径不触发（且被 freq_hold_frames 挡 2 拍），全簇锁死最低频表现为亮屏 0.2fps。

- 线程亲和预留接口：`fas_affinity_hook(affinity_mgr, corectl_mgr, active, fg_pid)` 在 chiri/mod.rs，FAS 激活/去激活时调用；当前职责 = FAS 激活期放开 top-app uclamp.max 为 100（快照还原；boost 进入按机型写入的 85 会钳制 EAS 对重线程的 capacity 视图、抑制 prime 放置，与 FAS 让 prime 承载负载相悖——8475 实测 prime 空转 45%；FAS 激活期 min=max 锁频绕过 schedutil，85 对调频无效仅剩放置负效应；去激活时若 boost 已退出则跳过还原，由 restore 链归位避免把 boost 值泄漏到 normal；8998 内核 4.4 走兜底探测永久跳过），后续 FAS 线程钉核扩展也在此实现，无需再改事件循环接线。**同一「放开 100」机制已与特调（akmode）共用**：方法改名 `set_boost_uclamp_override`，由 `apply_affinity_and_corectl` 按模式同步 uclamp 归属（特调 → `Some(true)` 本函数负责、fas → `None` 让出给本 hook、其余 boost → `Some(false)` 保证不残留 100）；8550 实测 arknights 特调下 big 饱和 47.6% 而 prime 仅 37%，正是 85 抑制 prime 放置的同一现象。

- FAS 白名单与配置（2026-09-20 起，终态 2026-09-22）：`module/config/normal/fas.yaml` 共 5 条目、4 份配置——`com.hypergryph.endfield: endfield`、`com.levelinfinite.sgameGlobal: sgame`、`com.tencent.tmgp.sgame: sgame`（**王者荣耀国服**：国服玩家实际玩 tmgp.sgame，此前白名单只有国际版 → 国服一直 default 裸跑 4.65W；sgame.yaml 已补 per_app_profiles）、`com.tencent.tmgp.speedmobile: speedmobile`、`com.tencent.lolm: lolm`（后两款同日新增，此前 default 裸跑 prime 顶格 0.61-0.67 util、迁移 1.2-2.1 万/s、4.13/3.09W；speedmobile/lolm.yaml 抄 sgame 模板）。sgame.yaml：fps_gears [30,60,120]、target_fps [60,120]、perf_init 0.50、core_temp_threshold 45（电池温度护栏）、migration_cost_ns 400000；perf_ceil 不覆盖保持 1.0，遵守「不限制最高性能释放」硬性约束。**扩容与参数均无真机 fps 验证**（2026-09-22 起未补；同版本观察口径下按 `mdocs/socList.md` 的「配置就绪」对待）。新增 FAS 游戏 = fas.yaml 加一行 + 新建 normal/fas/<配置名>.yaml，无需改 .rs。

- **FAS 快升慢降（2026-10-16 契约，`fas_types.rs` + `gear_state.rs`）**：升档确认 `d_up_confirm` 60→**45 帧**（升要快）；升档后进入 `DOWNGRADE_MIN_DWELL_FRAMES=120`（≈2s 墙钟，过 `scale_frames`）的**降档驻留期**——`downgrade_dwell_frames > 0` 时普通降档确认流被抑制（`downgrade_confirm_frames` 清零、`frame_pipeline` 逐帧递减）。**兜底不受驻留限制**：原生档识别与极端卡顿路径（`detect_native_gear`/`is_extreme`）在驻留期仍可直接降档，驻留只挡「慢降」常规确认流与 boost 抢救（`else if self.downgrade_dwell_frames > 0` 分支在 `recent30` 取消分支之后、boost 抢救分支之前；同时 `cancel_boost()` + `downgrade_confirm_frames = 0`）——不得把驻留检查放到这两条兜底之前。依据：logd_1016-165413 实测 185 次档位切换、15:13 附近 30↔45 秒级连打（confirm=10 帧快车道下仍抖）；包2 同场景待复核。
- **FAS 升档瞬间落点（2026-10-05）**：`MIN_UPGRADE_PERF_CEIL = 0.62`，替换 `do_gear_switch` 中 `min_upgrade_perf` 的 clamp 上限；120fps 档的 `new_fps/144` 会撞上限，因此 0.62 就是升档瞬间落点。降档侧 `max_downgrade_perf` 未改；`pid_jank` 的大帧紧急跳频仍直接抬到 0.70。
- **FAS 档位状态机判据（2026-09-28 修，`scheduler/fas/gear_state.rs`）**：升/降判据必须**同源同尺度**——升档 = `recent30 >= next - 10 && avg_fps >= tfps*0.9`，降档触发 = `avg_fps < tfps - 10`，**降档取消阈值同为 `tfps - 10`**；原取 `tfps - 5` 会造出 `[tfps-10, tfps-5)` 这个「既允许降档、又满足升档」的重叠区，落进去必然来回切档（2026-09-28 实测 45 s 内 4 轮 120↔60，`降档加速` 记录的瞬时均值全卡在 109.7~110.0）。注意 `recent30` = 最近 30 **帧**均值（120fps 下仅 0.25 s）、`avg_fps` = 120 帧窗口均值——窗口按帧不按秒，`WINDOW_SIZE=120` 与各 confirm 计数都是帧预算（`scale_frames(x, tfps) = x × tfps/60`，`upgrade_cooldown` 也必须过它，否则冷却墙钟时长随档位漂移）。`consecutive_downgrade_count` 的指数退避（`cooldown = 90 × 2^n`，n ≤ 4）**只在降档路径维护、升档不清零**——升档清零会让 backoff 恒为 `1<<1`、防抖失效，宽容度由 `update_stability_forgiveness()` 在「稳定跑满目标 900 帧」后逐档回收（app 切换经 `reset_runtime()` 归零），另有**时间衰减**：距上次降档超过 `DOWNGRADE_BACKOFF_RESET`(60s) 即视为新一轮降档、计数归 1——否则一次抖动期把计数推到 `min(4)` 后，16 倍冷却（≈24s 不许升档）会长期咬住后续孤立降档。原生档位识别 `detect_native_gear`（`|avg − g| < 8` 且 stddev < 10%）必须能命中「游戏原生 60fps」：原实现把它关在 `is_extreme`（`avg < tfps*0.40`）里，与自身要求交集为空（tfps=120 时需 `avg < 48` 且 `avg ∈ (52,60)`）、g=60 分支恒不可达，现提到降档分支顶部、与极端卡顿判定并列。

- **FAS 帧源与校验修复（2026-09-25/26 契约）**：① 帧源——`libgui.so` 的 `Surface::queueBuffer` mangled 名随 Android 版本变化，硬编码签名解析失败即 uprobe 挂不上 → FAS 档位永不调整且每 500ms 重试、每秒 2 条 warn（一个会话数千行）。现 `scan_queue_buffer_symbols()` 读 ELF64 dynsym 按 `_ZN7android7Surface11queueBufferE` 前缀取全部变体（候选顺序：短签名→长签名→扫描变体），attach 失败按符号类 60s/其余 1/2/5s 退避（首次与每 10 次 warn、其余 debug），退避窗口内不再解析 ELF。判读：mode=fas 行 fps 列全空 + `error resolving symbol` 即此症。② 校验——`fas-freq-mismatch` 大部分是校验时序自噪声（升频先 max 后 min 后立刻读 scaling_cur_freq，内核调频异步、读到旧档）：verify 计时起点 = 目标值最后一次变化，稳定 ≥ `VERIFY_SETTLE`(200ms) 后抽查；QoS 被钳（内核 thermal/限频）时进 `qos_clamped` 态不重写不告警、`force_reapply` 跳过（300s 长窗口才 warn）——防篡改语义不变（口径见 CLG 节）。③ 接管期写 `/proc/sys/kernel/sched_migration_cost_ns`（`FasRulesConfig.migration_cost_ns`，sgame 400000），deactivate 按快照恢复。④ **帧探针失效的开环退化（2026-10-16 契约）**：帧探针失效时（含 uprobe 挂上但事件链路断，见 06 内核实测档案 ChirunoNeko 条），FAS 经 `uses_load_control()`（`controller.rs:231-236`，帧新鲜度 `FRAME_FEEDBACK_STALE=500ms` :15）转**开环**：`update_load_control`（`controller.rs:253-272`）取 CPU util max clamp 成 perf_index——档位死锁在初始值（`evaluate_gear` 仅帧事件驱动，`frame_pipeline.rs:332` 无帧永不执行，玩家开 90/120 帧永不升档）、jank 升档/原生档识别等降档保护全失效（都在帧流水线里）、频率被动跟随 util；温控不受影响（`thermal_perf_cap` 在 load-only 分支保留）。**唯一可见症状 = status.csv fps 列全 '-'**，不查该列开环完全隐形（修复方向未实施：帧静默 WARN + `frame_src=silent` 标记；用户 2026-10-16 裁决不再深究，重开需新证据）。

- **FAS 帧源门控与播放态旁路（2026-09-27 契约，P2-3）**：`monitor::FasSignal` 在 FAS 激活位（生产方 `FasManager`）之外增一个**播放态旁路采样**位，生产方是 `TunedGovernor`——`init_policies` 接管 `playback` 时 `set_playback(true)`、`release()` 清零（只碰这一位，不碰 FAS 激活位，故「FAS 是否接管」的判定不失真）。fps 探针挂载谓词从「FAS 激活」扩为 `frame_source_active()` = **FAS 激活 或（playback 接管 + `diag_active()`）**，`monitor/mod.rs` 的首激活待机与 `fps_probe` 循环顶部同用它：无限等待（稳态 0 周期唤醒），只有「playback 在接管但诊断总闸关着」这一支用 **1s 有界等待**兜底——诊断开关（`meta.dev_record`）改动不经过本信号、没有唤醒通道，挂上探针就每帧过 eBPF 比每秒醒一次贵两个数量级，故诊断关闭期间宁可摘掉。分流：FAS 会话帧全喂控制路径；FAS 未激活时帧只累**离线直方图**（投喂只白烧通道——调度侧对 FAS 未激活的 `FrameUpdate` 本就 no-op），满 1s 落一行 `event`（`decision=playback_fps`、`reason=n=<帧数>;b<档号>=<计数>`，档宽 4ms、窗口内无帧即整行不落；进入/离开旁路时窗口重起，防两段之间被算成同一秒），判读口径见 `.cursor/commands/devimp-log-analysis.md` 判定要点。**播放态不写 status.csv 的 fps 列、不判 jank**：视频没有应用申报的目标帧率，且同进程多 Surface（视频层 24/30fps + 弹幕层 120Hz）混流。**存续条件（2026-09-28 评估）**：该旁路唯一产物是这份离线直方图，而它的立项理由「热压制是否伤播放」已在 `logd_0928-170828` 上答完（中/硬档窗口长尾率与未压制基线同量级 0.0~0.4% vs 0.0%）→ 之后只有**要复核播放态参数**（`tuned_profiles.yaml` 的 hysteresis/down_hold）时才需要它（`tuned_thermal_floor` 已于 2026-09-28 定案不下探：该地板只在 `cap < floor` 即硬档生效，而硬档窗口里播放态的有效上限本就是 0.55，「无长尾」证明的是 0.55 够用、并未覆盖更低的值）；该条台账关闭后按仓库惯例注释掉旁路入口（`// [PAUSED]`，勿删代码）。汇总已固化在 `dvmain.py [7]`（只统计 `n>=15` 活跃窗口 + 长尾率）；已知瑕疵：同 app 多进程交替占坑（实测同秒 `9913→18144→9913` 反复 detach/attach，伴随 `PID 切换失败: perf_event_open failed`）→ 直方图实际只覆盖当前挂载的那个进程。

- 调度能力范围（相对 CLG 的扩展仅频率维度）：per-policy min=max 锁频（PolicyController，1.5s 回读校验防内核覆写）+ 容量加权分频 + util 软封顶 + PID/Jank 帧级响应；FAS 自身不做 cpuset/uclamp/core_ctl——线程摆放复用 boost 布局（fas 亮屏入 boost，见亲和章节），uclamp.max 放开经 fas_affinity_hook（特调同机制但经 `apply_affinity_and_corectl` 按模式同步，见上条）。

### CLG 调频语义（动态上限制）

- CLG 不再锁频 min=max，改为 schedutil 动态上限：接管时把 `scaling_min_freq` 一次性压到硬件最低，之后只写 `scaling_max_freq`（CLG 决定性能上限），schedutil 在 \[硬件最低, 上限] 内按瞬时负载自主调频，空闲间隙内核微秒级降频到地板，消除锁频方案"采样间隔（160ms）内频率下不来"的空转发热（8550 实测主要热源）。

- 上限语义下 `perf_floor/perf_ceil/perf_init` 约束的是上限（空闲实际频率由 schedutil 决定），`perf_floor` 允许被热保护击穿。

- **`util_smoothing`（2026-09-22 新增，CLG 决策负载 EMA）**：抖动负载下 max*util 每 tick 大幅摆动 → target_perf 翻摆、决策方向反复翻转、`scaling_max_freq` 高频改写（8550 QQ 实测大核 2048 tick 反转 458 次）。EMA 滤波（语义同 tuned 的 `util_smoothing`，1.0=关闭、缺省即关闭=逐位等价）后反转降约五成、写频降三成。default/reduce 配 0.5，boost 保持 1.0（响应优先）；α 0.35 可再降抖动但上限均值抬升更多。main* `max_util` 列为双语义：CLG 行写**平滑前**原始值（离线回放可自行试验系数）；tuned（akmode）行写平滑后的决策负载（2026-09-17 起语义变化）——离线回放按行来源/decision 区分，tuned 行不可再当原始 util 二次平滑。字段新增须同步：`config.rs` 字段+缺省+normalize+Default、`feature.yaml` 全部 SoC 文件（SoC 文件整份覆盖根，只改根不生效）、`mdocs/config-example.yaml`。

- **FAS 防篡改强制重写改时间基准（2026-09-24）**：`freq_force_reapply_interval` 语义由「每 N **帧**」改为「每 N **秒**」（默认 30 秒，`normalize()` 钳制最小 1——旧实现写 0 会 `% 0` panic，看门狗反复重启）。旧行为在 120fps 下等效 0.25s 一次、144fps 更密，每次对 3 个 policy 做两次写频 + 两次 `re_unmount()`（实测游戏期约 24 写 + 24 `umount2`/s 且随刷新率放大）→ 现在降至 0.2 写/s、0 `umount2`；`PolicyController::force_reapply()` 里的两次 `re_unmount()` 已删除（它只 `umount2(MNT_DETACH)`、不重开 fd，对写入路径零贡献；`verify_prev_freq` 里那次保留）。副作用：`freq_hold_frames` 的 hold 不再被高频 force 击穿。**防篡改是异常兜底语义**：ChiRi 是设备上唯一的 userspace sysfs 写频调度程序、无竞争者（口径，2026-09-26 依内核源码分析修正；频率实际决策链 = waltgov + vendor hook（OMRG/frame_boost）+ FREQ_QOS 聚合，详见 `agentsdocs/06-kernel.md`「调频路径与竞争者」，ChiRi 写 scaling_max/min 是 clamp 不是频率决策，内核 thermal QoS 钳制是合法态不算篡改），节点被改写属异常态（残留旧模块、手动调试、内核异常态）。
- 全局省电向（2026-09-23 十三轮调参定稿方向：**不封顶，控高频门槛 + 动态范围**——用户口径：除部分特调外任何在用模式保留随时到最高频的能力；控功耗靠升频门槛/速率/平滑，不靠砍上限）。8550 default 现值：up_threshold 0.88、headroom_ramp 0.08、smoothing_up 0.60、up_rate_limit_ticks 3、up_jump_threshold 0.45、slow_up_scale 0.02、down_threshold 0.62、down_rate_limit_ticks 2、down_fast_threshold 0.30、spike_decay 0.10（离线模拟 QQ −3.7~−5.9%，且新参数下出现 cap≥70% 时刻 = 既压均值又保留冲高频）；per_cluster 仅 little `up_threshold: 0.95`——A510 r1 支持 AArch32、DT 能耗模型（coeff 251）低估其能耗，EAS 会把 64 位线程吸进 little，故抬高其升频门槛而非砍上限；affinity 升核 EINVAL 已按 `pin_incapable` 标记跳过。reduce：up_threshold 0.90 + perf_ceil 1.0；boost：up_threshold 0.55。**2026-09-28 天花板全拆**（R2 硬口径）：8550/8475/8998/根兜底的 `reduce` 与 `default` `perf_ceil` 一律 1.0、特调 akmode 0.95→1.0、playback `headroom` 0.85→1.0 且三簇 `per_cluster` 天花板（little 0.90/0.55、big 0.75、prime 0.80/0.50）整段删除；仅 SceneMode 的 `perf_ceil 0.12` 保留（R1 唯一允许不到最高频的轴）。拆掉的稳态降功耗能力由 T11 稳态下探接棒（见下条），交互响应仍由 touch_boost 兜。`module/config/feature.yaml`（根）是未命中 SoC 目录机型的兜底配置与代码默认值来源；**SoC 文件是覆盖层只改根不生效**。ceil 压低后交互响应全靠 touch_boost——「不跟手」应调 boost 而非抬回 ceil。FAS（sgameGlobal 等）仍是唯一 perf_ceil 豁免。历史演进：0920「禁压 ceiling」→ 0922 日常档 ceiling 0.80+per_cluster → 0923 推翻（不封顶），引用 0920/0922 的 CLG 数字一律过期。

- 多线程按核心组独立调度：每个 cpufreq policy 由一个独立的 `CoreGroupWorker` 线程管理，持有各自的 `ClusterState`（频率档位、`FastWriter`、`current_perf`、防抖计数器等），线程内自主完成决策 + 写频，核心组之间完全并行无锁。`CpuLoadGovernor`（`cpu_load_governor.rs`）是 Worker 线程管理器：`init_policies` 枚举系统 cpufreq policy、为每个 policy spawn Worker 线程（写默认调速器、min 压硬件最低、max 按 perf_init 设初始上限）；`release` 停止所有 Worker（Worker 退出前自行恢复系统原始状态）；`reload_config` 停旧 Worker + 用新配置 spawn 新 Worker（等价轻量 init，current_perf 重置到新 perf_init）。

- scheduler_ipc 通过 `on_load_update(&core_utils)` 将负载数据广播给所有 Worker（非阻塞 `try_send`，通道满则丢弃本 tick），Worker 线程内自主决策 + 写频，无需外部 flush。

- 升频 = 平滑抬高上限：ceiling 提升不强制频率跳变，无需读 `scaling_cur_freq` 做余量检查（旧 `clg-up-skipped` 机制已随锁频语义删除）。降频 = 一步到位降上限：防抖确认（`down_rate_limit_ticks`，极低负载命中 `down_fast_threshold` 免防抖）后直接写目标档（`ratio_of_freq` 同步 current_perf），降 ceiling 只收窄 schedutil 区间、不会把实际频率抬上去。`smoothing_down / slow_down_scale / down_fast_mult` 是 Yumi 参数，已随 Yumi 调度本体删除（2026-09-22），feature.yaml 里的同名键一并移除；README.en.md 里仍有其文档，属上游历史参考。

- 热切换配置重放 perf_init：`reload_config` 重置 current_perf 到新 perf_init 并立即写频，避免接管间隙 current_perf 掉到 0 后频率要从地板缓慢爬升数秒（息屏 doze/scenemode 切换同样经此热切换恢复，见「息屏省电与屏幕状态」）；模式变更 / ConfigReload 热重载同理。

- **写频滞回契约（2026-09-24 起）**：决策值到落盘之间有滞回层——CLG `write_dwell_ms` 缺省 320ms（2×160 tick）+ `write_deadzone` 0.03（决策落点在死区内不写频）；tuned 用 `gated_write` 80ms（2×40 tick）。两者可配（钳 0..5000ms / 0..0.2，0=关闭），feature.yaml 模板刻意不写。**豁免**（直接写、不过滞回）：触摸提频（升向）、fast_down（降向）、模式接管/释放、写失败补写；**热保护 clamp 零豁免**（过热有内核温控兜底，若豁免则温度毛刺绕过滞回直写频率、反致策略频繁调整）。背景：改前 little 决策翻转 2.0-2.3 次/s、prime 2.5 次/s 而实际写频仅 0.15 次/s（up_wait/hold 标签振荡），死区进决策层 + dwell 才压得住。三断链教训见 04。
- **方向稳定：死区 gate 双向对称（2026-09-27 修，P1-2）**：`on_load_update` 的 `[dir_stabilize]`——**上边补齐与下边同源的死区 gate**（落点与当前频差 <= `deadzone_band(write_deadzone)` = 写不进 sysfs 的 no-op up → 记 `hold`、不累计 `up_wait`、**不清 `down_wait`**），下边的 `up_wait = 0` 从死区判定**之前**挪到之后。根因改动前只有下边有这条 gate，上边任意 `+1e-6` 的目标抬升都走 up 分支并清零 `down_wait`，于是「本 tick 根本写不出一次频差」的单 tick 噪声也在翻转方向（实测 big 簇 52~86 次/分：coolapk 4.4 min 379 次、1s 内最多 6 次；QQ 达 7 次/s），两个方向的确认进度被互相抹掉。修后方向只由「真能写出一个死区以上频差」的 intent 决定，翻转至少要跨一个 OPP 档；没有新增参数，保守化的旋钮就是 `write_deadzone`（每档死区 = 该值 × 簇硬件最高频）。

- **CLG 高位升频门槛 `[high_zone]`（2026-10-13 新增）**：高位区（perf ≥ `high_zone_gate`）升频除 util×headroom 外还需「≥`high_zone_count` 个核且每核 util ≥`high_zone_util`」共同确认，防单核毛刺把上限推上去（修前高位只受 util×headroom 驱动、到达路径太松）。四参数（`CpuLoadGovernorConfig`，serde default）：`high_zone_gate` 缺省 0.85（perf ≥ 该值进入高位区）、`high_zone_count` 缺省 0（**0 = 关闭**）、`high_zone_util` 缺省 0.90（单核达标占用）、`high_zone_hold_ticks` 缺省 20（被连续阻塞满 N tick 强制放行一次，防错配永久卡死）；normalize 把 gate/util 钳 [0,1]、count > 0 才生效。`ClusterState` 增 `high_zone_wait` + `high_zone_hits()`（本簇 util ≥ 阈值的核数）；`on_load_update` 在 target 计算后插门槛块：old_perf ≥ gate 且 candidate > old_perf 时查 hits —— 达标即清 wait 放行；未达标则 `high_zone_wait += 1`，未到 `hold_ticks` 就**阻塞本 tick、冻结状态早退**（日志 `dev_tgt_perf = old_perf`、`decision = "hz_hold"`，不进 up/down 分支、不动 `up_wait`/`down_wait`），到 `hold_ticks` 强制放行一次并清零重新计数；**降频方向零影响**。门槛只压「决策意图」：不回写 `current_perf`、不碰写频 / 热钳 / 触摸 / CLG-PF 路径，故与「随时可到最高频」硬口径相容（重放自检断言 2000 tick 内 perf==1.0、freq==hw_max，阻塞下靠 hold 兜底收敛）。**回退 = 删三行 `high_zone_*`（`count` 归 0 即关闭）**。8550 reduce/default/boost 三段全部启用 `gate 0.85 / count 2 / util 0.90`（big 4 核取 2 = 50%；little 3 核取 2 = 67%，宁可略紧防 A510 贴顶；boost `perf_init 0.60` 起步即过 gate）。工具链：改 `on_load_update` 决策函数须同步更新 replay 三件套（`scripts/tests/clg_replay.rs` 模板 `ClusterState` + replay 版 `high_zone_hits` + `scripts/devimp/replay_clg.py` fields 提取），否则 scratch 编译失败。

### 稳态下探（T11）与相对能效模型（EEM）

- **稳态下探（T11，2026-09-28 新增）**：`steady_decay_*`（CLG 与特调各一份，缺省 `enabled=true`）。语义 = 对「写侧性能偏移」的**限幅积分控制**，被控量是 util：目标水位 `T = up_threshold × steady_decay_target_ratio`（缺省 0.90），util 高于 `T+band` 就回收（步长 ×2 防过冲）、低于 `T−band` 就加深（步长 `steady_decay_step`，缺省 0.01）、死区内保持；`steady_streak` 需连续 `steady_decay_stable_ticks`（缺省 12，≈2s）稳定才建立，**负载上行（Δutil > band）/ `util ≥ up_threshold` / 极低负载立即清零**。偏移只在 `flush()` 里从 `eff_perf` 减去（再 `.max(perf_floor)`），**绝不回写 `current_perf`**——否则会与升频分支的 `target_perf > old_perf` 比较打架产生振荡；自限性来自「压频 → 真实 util 被 sched_switch 记账反馈抬高 → 触阈值即清零」。为什么不做「稳态就无脑累加」：那会形成 2~4s 的频率锯齿（正是 R6 要防的震荡）。配套特调 `up_confirm_ticks`（缺省 2）= 严格升频审批，目标比当前高时需连续 N tick 才抬升（CLG 侧本就有 `up_rate_limit_ticks`）。全部参数可在 feature.yaml / tuned_profiles.yaml 覆盖。**8550 三模式取点（2026-09-28）**：`reduce` `steady_decay_target_ratio: 0.80`（T = 0.90×0.80 = 0.72，省电档取点更左）、`default` `0.90`（T = 0.79）、`boost` 直接 `steady_decay_enabled: false`——boost 的 `perf_ceil` 拆前拆后都是 1.0，关闭下探后其行为与天花板拆除前逐位一致，且避免 `up_threshold: 0.55` 导致 T = 0.50 把常态负载也下探掉（削响应）。

### 相对能效模型（EEM）：调频封顶 + 线程迁移（2026-10-22 重构，取代 CLG-PF 与 FDP）

> **一句话**：绝对能效曲线不再参与任何决策。单核功耗与单核算力各自归一成**相对向量**，
> 之后全部比较都在无量纲空间里做：**同算力下取更低功耗**，甜点（1 单位功耗换 1 单位算力）
> 是「加核 vs 升频」的判据，线程迁移一律**先扩后缩**。

- **为什么推翻旧 PF（根因，勿再犯）**：旧 `demand_buckets` 桶表的**两个轴都假设整簇满载**——
  功耗轴 `n×active − (n−1)×净idle`、算力轴 `n × cap × f/f_max`。但调度器搬的是**线程**：
  一个线程跑在 6 核簇上是「1 核忙、5 核空闲」，簇频照样拉满。sm8750 big 顶档据此高估 **5.85×**
  （模型 6×3.819=22.91 W vs 实际 0.095+3.819=3.91 W），且首桶 `lo_units`=979 高于单线程满打满算的
  762 → 单线程需求**落在首桶之下**、`bucket_for_demand` 返回 None → 前沿对单线程**从不生效**。
  两轴同比例高估使边际能效恰好不变，所以这个 bug 长期无症状。
- **数据层**：`soc.yaml` 的 `frontier_policy.per_cluster.<簇>` 只保留 `capacity` / `freq_khz` /
  `power_w`（簇级，溯源用）/ **`single_core_power_w`**（单核动态功耗，运行期唯一消费项）。
  `demand_buckets` / `modes` / `anticipate` / `frontier` / `anchors` **已全部移除**（生成器同步）。
  `single_core_power_w` 由 `scripts/frontier_policy.py` 用 `power_w ÷ 簇核数` 生成——**口径显式分列**
  就是为了根绝「单核/整簇混用」（历史事故源）。缺该字段时 `energy_model` 回退到 `power_w ÷ n`。
- **两套向量，用途严格分开**（`src/chiri/energy_model.rs`）：
  - `cap_rel/pow_rel`：**簇内**归一（首档扣 0、末档拉到 1），**只用于求甜点**。纯形状量，任何正常数缩放都不改结论。
  - `cap_glob/pow_glob`：**跨簇**统一标定（功耗 ÷ 全 SoC 最大单核**绝对**功耗、算力 ÷ 最大单核 capacity），
    **用于跨簇比较**。⚠ **绝不能扣首档基线**：首档功耗就是该簇静态开销，扣掉等于让「开一颗核」免费，
    prime 会因动态增量小而永远胜出（回归 `cross_cluster_keeps_static_term` 锁死这条）。
- **甜点判据**：`Δcap_rel/Δpow_rel` 最后一次 ≥ 1.0 的档。**必须用严格 `dp <= 0` 判零增量，不能用
  `f32::EPSILON` 这类绝对阈值**——相对向量整体缩放后增量会同比例变小，绝对阈值会把「不免费」误判成
  「免费」、甜点随缩放漂移（回归 `sweet_point_is_scale_invariant` 已证）。实测甜点：sm8750 big 1786/3533 MHz、
  prime 1958/4320 MHz；sm8550 little 1018/2016、big 1402/2803、prime 1709/3187。
- **成本口径按忙核数**：`cost(gear,k) = pow[gear] × k`、`supply(gear,k) = cap[gear] × k`，
  `k` = 真正承接工作的核数。这是相对旧 PF 的**根本修正**（不再假设整簇满载）。
- **调频侧（CLG，`eem_target_freq`）**——三分支，仍守 `target = min(ratio_freq, eem)` 只降不升：
  1. **甜点档就够用** → 封顶在甜点（零损失：需求本就能满足，再升频纯烧电）；
  2. **本簇占用达 `ceil(n/2)` 核当量** → 允许越过甜点（多核共享同一档位，升频收益覆盖全簇；
     此时迁移也无处可去）。6 核 big 门槛 3.0（单线程最多 1.0 → 单线程永不单独触发）、
     2 核 prime 门槛 1.0（单线程跑满即触发，符合物理：频率域共享、别无他法）；
  3. **别处更便宜** → 仍封顶甜点，把缺口留给 affinity 侧迁移。
  **抗饥饿**：封顶且需求确实超出该档供给时累计 `eem_press_streak`，连续超
  `EEM_CAP_RELEASE_TICKS`(10 tick=10s) 即放弃封顶回比例路径——迁移动不了时不能长期锁性能。
- **迁移侧（`src/chiri/placement.rs` + `affinity::eem_place_round`）**——**不分前后台**：
  所有被采样线程（`util_pct ≥ eem_min_util_pct`，缺省 8%）都参与，`is_key` 除外（关键线程由 boost 独占）。
  * **只越过甜点才迁**：甜点以内频率维度还没用尽，迁移只会打断缓存局部性。
  * **目的地必须接得住**：`dst_load + demand/dst_cap ≤ 1.0` 才迁——只看成本不看目的地负载，
    会把所有线程堆到「模型最便宜」的那颗核上，结果该核过载顶频、反而更费电。
  * **「拉核分摊」的正确层级**：**不在线程放置侧做**。单个线程同时只能占一颗核，加宽它的掩码
    只是让内核在核间弹跳、算力不会变多（曾按「加宽窗口 + k 核重算档位」实现，会把所需频率
    凭空砍半，2026-10-22 审查移除）。分摊由两处实现：① CLG 的 `supply(gear,k)=cap[gear]×k`
    随本簇忙核数增长，同一需求摊到更多核上就落在更低档；② `best_window` 把**不同线程**
    摊到不同核。数学依据：sm8750 big 上 d=0.7 时 k=3(1152MHz) 比 k=1(3533MHz) 省 **68%**。
  * **先扩后缩**（`step_range`）：`0-2 → 0-3 → 1-3`。并集掩码先行，线程全程有核可跑，
    绝不出现「旧核已摘、新核未到」；上行与下行共用同一协议。方向日志口径 = **两端同向移动**才算
    up/down，单端移动（纯扩张/收缩）= hold。
  * **三重防震荡**：甜点阈值 + 相对滞回 `SWITCH_HYSTERESIS`(0.15) + 驻留冷却 `MIN_DWELL`(2s，
    反跳回 ×4)。每轮迁移条数上限 `EEM_MAX_MOVES_PER_ROUND`(4)——同轮负载快照不会因迁移而更新，
    无上限会让大批线程同时判「同一簇最便宜」而齐涌（FDP 的 `max_inserts` 教训）。
  * **cpuset 前置**：纯后台线程的组通常只含小核，掩码写到 big/prime 会被内核**静默裁剪**
    （写返回 0 但掩码没生效）。故跨到更高能级簇时先 `move_group → top-app`，升组失败即本轮放弃。
  * **写后读回校验**：`set_tid_affinity` 成功后回读掩码，发现越界核被裁剪即记 `clipped=true`
    且**不更新记账、不发 INFO 迁移日志**——不校验就会把「内核没答应」记成成功迁移。
- **日志两级**（用户口径）：INFO `affinity-migrate` 只写**线程 + 方向 + 核区间**；
  devimp `@A act=place` 帧写全量（`dst=<lo>-<hi> value=gain=..,settled=..,reason=..,clipped=..`
  `reason` 槽写方向 `up/down/hold`）。CLG 侧 tick 日志键 `clg-eem-tick-log`（带 `eem_khz/sweet_khz/cores`）。
- **与 FDP 的关系**：`eem_placement` 缺省 **true**（`apply` 里 `&& energy_model::available()` 自动兜底，
  无模型机型静默回退），EEM 开启时 **FDP 自动让位**（两套成本模型口径不同，同轮同时决策会互相覆盖）。
  `fdp_*` 配置保留但不再生效，仅作回退路径留痕。sm8550 / zumapro / sm8750 已显式 `eem_placement: true`。
- **验证能力**：`YUMI_SKIP_EBPF=1 cargo check` 可在开发机做**全仓类型检查**（`screen_detect.rs` 的
  `__system_property_get` 已有 `#[cfg(not(target_os="android"))]` 桩）。离线回归
  `scripts/tests/energy_model_regression.rs`（29 项）覆盖甜点判据、缩放不变性、静态项口径、
  先扩后缩、方向判定、选核、k 核分摊数学关系。

### FDP：前沿主导的放置内核（**已被 EEM 取代，2026-10-22**；仅作回退路径与历史留痕）

- **一句话**：后台/批处理线程 promote 时不再按「簇名气（big 优先、钉满溢出 prime）」选目的簇，而是算**双边净收益** `net = [P_src(让出后) − P_src(现状)] + [P_dst(承接后) − P_dst(现状)]`，只有 `net ≤ −fdp_cost_hysteresis_w` 才迁——等价于「**等算力下把这份算力放到边际 mW/Mdmips 更低的地方**」。❌ 不用单边 `ΔP(dst)` 选最小者（其增量恒 ≥ 0，与「不搬」比永远更差，会退化成永不动作）。
- **数据不手抄、单点真相**：`src/chiri/energy_cost.rs` 直接读 `soc_frontier_policy().per_cluster.<簇>.{capacity, freq_khz, power_w}`；逐档簇功耗由 `scripts/frontier_policy.py` 从 `mdocs/sm8550/sm8550-freq-power.md` §3 生成进 `sm8550/soc.yaml` 的受控块（生成时断言「文档档位数 == soc.yaml 档位数」，不一致拒绝落表）。**无该段的 SoC**（无 capacity / 无 `per_cluster` 功耗表）→ 调用方先经 `energy_cost::fdp_available()` 判定，缺数据即**回退原 A3 放置路径**（不是把后台 promote 整段停掉）。表是纯动态项、不含漏电，只作相对排序。**两机表不同源**：8550 是按 `Cx·f·V²` 推算的，zumapro 是 PMIC ODPM 实测净能量并做过漏电校正（`cluster_W = n×active − (n−1)×idle`）；两者都只进相对比较，不可跨机比绝对值。
- **工作点口径（现状实测 + 之后按单核落位）**：现状取该簇**最忙核的 util**（policy 口径，取各核最大值）+ 实测 `scaling_cur_freq`（读不到才用模型），`P_now` 直接查表；之后按「一份需求落到**一个核**」算 `Δu = d / capacity`，再用 schedutil 口径 `f = f_min + u_after × (f_max − f_min)` 并 floor 对齐真实档位。`P(f)` 超线性（§2.1 算力/mW 随频率单调劣化）→ 把目标簇推向高位会自动变贵，这就是「注意目标核当前能效、别因迁移把它拉劣」的代码形态。**不要**用过原点线性 `f = d×f_max/(核数×capacity)`（那假设需求在簇内均摊，对 prime 这类单核簇会算出低于最低档的频率、把 prime 判成「免费」）。
- **边际能效劣化边缘的保守包络（2026-09-28 审查后修）**：现状与之后**必须锚在同一侧**，否则两个不同底相减会算出负的 `ΔP`。修前是「现状取实测、之后取纯模型」，当目的簇因 QoS / 热 / 别的线程尖峰**此刻跑得比模型高**时会得到 `f_after < f_now`、`ΔP_dst < 0`——把承接成本算成免费甚至收益，反鼓励往满簇塞（计划 E2 警告的那条）。
  现在改成：**目的簇** `now = min(实测, 模型)`、`after = max(模型, 实测)`，于是 `ΔP_dst ≥ 模型边际 ≥ 0`，并顺带堵住「簇已在热 ⇒ 加负载免费」的错觉；**源簇** `after = min(模型, now)`，保证 `ΔP_src ≤ 0` 且让出收益宁小勿大。
  实测与模型一致时（无 QoS/热/尖峰干扰）行为与修前**逐位相同**；不一致时只会让迁移更难、不会更易。
  附带：0.85 频率护栏现在比的是包络值（≥ 实测现状），所以「**目的簇此刻就已经贴在劣化区**」也会被拦下（修前只比模型值，会漏这一半）。
- **防拥挤（同轮硬约束，成本项只在跨轮生效）**：每轮每目的簇迁入条数上限 `fdp_max_inserts_per_round`（同轮几十条线程看到同一份快照会一起判「big 最便宜」，必须靠配额兜）+ 迁入后模型工作频率不得超 `fdp_dst_freq_cap_ratio × 最高频`（P-a「任何簇贴顶都不行」的落点）。
- **prime 的资格仍由 `bg_promote_exclude_prime` 把关**：静态表不含漏电/idle-exit（prime 独核被从 rail-collapse 拉起后的静默代价不在表里），且本模型判不出 P-b 的「单核算力需求 >2600 Mdmips 才用 prime」门槛 → 缺省候选只有 big，显式把该开关置 false 才纳入 prime。**注意**：`fdp_enabled=true` 时不再让 `big_pressure` 一票否决 promote（FDP 自己的 0.85 频率护栏已覆盖「别往已热的簇塞」）——否则放开 prime 后，最该分流的「big 已热」场景反而会被整体闸掉、prime 分支近乎不可达（独立审查发现后修）。
- **作用域 / 开关 / 不变式**：只替换**后台/批处理线程 promote** 的目的簇选择（成本模型不含延迟项，前台关键线程与普通前台线程路径一律不动）；`fdp_enabled` 缺省 false、8550 显式 true，关掉后逐位回旧行为；`fdp_enabled=true` 但静态数据缺失时同样回退 A3（`fdp_available()` 判定）。落点仍走既有 `pick_core_pref`（不绕 `MAX_PINS_PER_CORE`/`PINNED_WEIGHT`/过载分散），选核失败即不动（不 move_group、不置 promoted）；三道时间闸与 A3/A6/A10 不变式全部保留。日志 `action=fdp` 带 `src→dst`、两簇 before/after 频率与 net(mW)，供离线核对。

- **`src/chiri/energy_cost.rs` 的文件结构与两个易踩的实现坑（2026-09-28）**：文件头有 `//!` 区块索引，内部按 `[source] [power] [supply] [cost] [probe] [decision] [tests]` 分块；核心是**三个纯函数**——`cluster_power_w`（逐档插值）、`freq_for_policy_util`（schedutil 口径的频率映射）、`net_benefit_w`（双边净收益）。**簇到 cpufreq policy 的映射只发现一次**（policy 目录开机后固定、热插拔不增删 policy），此后每轮只读 3 个 `scaling_cur_freq`，**不再扫 policy0..16**——新增读点时必须走这层缓存，勿在热路径里重新枚举 policy 目录。数据一律读 `soc_frontier_policy()`，不手抄。FDP 实现过程中被打回的两处模型缺陷（均已修，勿回退）：① 频率不能由「簇总算力」过原点反解（那假设簇内均摊，会把单核的 prime 判成「免费」），改为「现状取实测 `scaling_cur_freq` + 最忙核 util，之后按单核落位 + schedutil `f_min + u×(f_max−f_min)` 映射」；② prime 候选资格不能交给成本函数（静态表不含漏电/idle-exit，且判不出 P-b 的 >2600 Mdmips 门槛），仍由 `bg_promote_exclude_prime` 把关。回退 = `fdp_enabled: false`（缺省即 false，8550 显式 true）。

### core_ctl 核心在线接管（ChiRi 专属）

- `src/chiri/core_ctl.rs` 的 `CoreCtlManager`，**三态状态机**（None/Boost/Scenemode，`set_power_state(boost, scenemode)` 统一入口，内部去重可被 2s 周期安全调用；切换时先退出旧状态恢复快照再进入新状态）。调度线程收尾 `release()` 按当前状态恢复。**NONE 与 BOOST 稳态都重试 `offlined` 残留核**（每 2s）——只重试 NONE 的话，scenemode 退出恢复失败后用户随即进入 boost（fas/performance 全时段 boost），prime 会整场游戏保持离线（超大核异常离线实测根因之一，2026-09 修复）。

- **Boost**：把各 cluster 的 core_ctl `min_cpus` 抬到全组常在线（防内核 core_ctl 热插拔与 ChiRi 调频打架），退出恢复快照；只动 min_cpus 不动 max_cpus/busy 阈值。

- **Scenemode 离线核（息屏深度省电）**：`CoreCtl.scenemode_offline` 门控（8550/8475 true，8998 内核 4.4 默认 false）。进入 scenemode 时先解除 boost（min_cpus 抬着会让内核 core_ctl 重新拉起被下线的核——两者互斥由 `apply_affinity_and_corectl` 保证），下线目标由 `scenemode_targets()` 计算：**little 簇除引导核（CPU0 无法热拔出）外的全部核**（2026-09-28 由「只下线 prime」改来，动机见「息屏省电与屏幕状态」），big / prime 常驻低频。**目标簇定位必须按「首核号 + 簇内核数」的区间判据**（`clamp_scenemode_cluster_via_max_cpus`）：该目标列表已 retain 掉引导核，其 `first()` 是 CPU1 而不是 little 簇首核 CPU0，拿它去比 `first_cpu`（恒 0）会**永不命中**（2026-09-28 审查发现的 P0：改「下线 prime→little」时漏改定位，导致 max_cpus 钳制、引导核护栏、min_cpus 压低与两处记账**全是死代码**，scenemode 只剩逐核 offline 与 core_ctl 每 2s 拉锯）（频率上限由 scenemode CLG 配置统一压制）。压制**两步都做**：① 用 core_ctl `max_cpus` 钳住目标簇的在线核数上界（`scenemode_keep_cpus()`：含引导核的簇写 **1**，不含引导核的簇才写 0——0 是 walt_halt_cpus 整簇停摆语义，对含 CPU0 的簇写 0 会把引导核一起停摆），② 逐核写 online=0 落地并回读验证，失败跳过（warn），已在 offlined 中的核防重复登记。两条记账（`max_cpus_applied` / `offlined`）**可并存**、恢复各按快照回写（恢复顺序：先解除 max_cpus 钳制再拉回被下线的核）。**内核约束与护栏（2026-09-28 独立审查后补）**：① 写 `max_cpus=keep` 前先把该簇 `min_cpus` 压到 keep——内核要求 `max_cpus >= min_cpus`，否则写入被拒（本函数只记「未生效」），core_ctl 仍会按 `min_cpus` 把核拉回、与逐核 offline 每 2s 拉锯；退出恢复顺序因此固定为 **max_cpus（解除钳制）→ min_cpus（写回快照）→ 逐核 online**；`min_cpus` 的压低**独立记账**（`min_cpus_lowered`，`restore_scenemode_min_cpus()` 写后读回、失败保留记账由稳态重试兜），不因 `max_cpus` 写不回去而永久停在 1（元审查发现此缺口后补）；进入 BOOST 时清该记账（boost 自己写全组 min_cpus，退出 boost 由 `restore_min_cpus()` 按快照全量恢复）。② 写 keep=1 后回读 `cpu0/online`：万一某内核把保留档也当停摆档，引导核掉线是系统级事故 → 立即回滚 max_cpus、放弃 core_ctl 压制并打 `corectl-reserved-core-halted`（逐核 offline 兜底永远不碰 CPU0）。③ `max_cpus` 节点被厂商锁写时写回恒失败，若把独占死绑在它上面，亮屏后调度服务会长期钉在大核上、业务侧少一颗 big → 核都回来且超 `MAX_CPUS_RELEASE_GRACE`(5s) 即释放独占，钳制残留继续重试。**独占一颗大核给调度服务**（编号最大的 big——little 已整簇下线，没有可独占的小核；代价是业务侧少 1 颗大核——三步实现：① `affinity::exclude_core_from_cpusets` 把该核从全部业务 cpuset 组（top-app/foreground/background/system-background/restricted）的 cpus 移除，其他进程/新进程（继承组掩码）均不可调度到该核；② 自身全部线程移入 cpuset 根组（根组含全部在线核，sched_setaffinity 才不会被原组掩码二次过滤）；③ 全线程自钉到该核）；设备无 /dev/cpuset 时降级为仅自钉。维持期每 2s 纠偏：重新下线被拉回的核 + `exclude_core_from_cpusets` 重写被框架 CpusetManager 加回保留核的组（快照只记首次原始值，防框架中间值覆盖）。**退出恢复 `restore_online` 失败的核必须保留在 offlined 中由 STATE_NONE 分支每 2s 重试**——此前失败即 clear、状态机回 NONE 再无重试路径，写回被内核拒绝的核永久离线；全部恢复后才释放独占（cpuset 快照还原 + 自身线程移回原组 + 解除自钉），重新下线前先把残留核拉回在线（否则 online=0 被跳过记录、核永远失去恢复登记）。

- **scenemode 饱和退出**：常驻核（little 引导核 ∪ 全部 big；判据取每核 util 最大值）max_util 持续 10s ≥ 70%（`SCENEMODE_SAT_UTIL/SECS`，util 是忙时占比与频率无关，饱和即真饱和）→ 视为后台负载压不死常驻核：一次性退回 reduce 的 CLG 配置 + 立即恢复全部在线核 + 释放独占核，并进入 **300s 冷却**（`SCENEMODE_COOLDOWN`，期间 scenemode 入口被门控不得重进，防反复拉锯）；冷却结束后息屏条件仍满足则自然重进。

- **自钉/解钉按成功清单（2026-09-23）**：`pin_self_dedicated` 记录实际钉住的自身 tid 清单（`self_pinned_tids`，只记成功），`unpin_self` 按清单逐个恢复全核（非 ESRCH 失败留清单下次重试）；勿用 `self_pinned` 总开关短路恢复——部分钉定失败时 `self_pinned=false` 会让 unpin 整体跳过，已钉住的线程永久滞留单核掩码。

- 为什么选核排除离线核而不"按需唤醒"：唤醒大核要拉电压轨/重建 L2，为后台线程点亮大核净亏能；直接写 online 会与内核 core_ctl 热插拔打架（对方再下线，ping-pong）。需要更多在线核时的正确姿势是抬 core_ctl min_cpus（Boost 态）。scenemode 是唯一反向使用 online 写入的场景（目标恰恰是让 prime 睡死，小核+大核常驻保住待命响应）。

- cluster 发现：遍历 `get_cpu_policies()` → related_cpus 首个 CPU 的 `/sys/devices/system/cpu/cpuN/core_ctl`（每 policy 一份，天然去重），惰性枚举一次；无节点打点 `corectl-unavailable` 后保持空表（scenemode 直接 sysfs 离线**不依赖** core_ctl 节点，仅受独立配置门控）。

- 配置段 `CoreCtl`（enabled/scenemode_offline）。`scenemode_offline` 在 2026-10-01 前为**死字段**（`mod.rs` 未接线、下线由 `enabled` 无条件驱动），现已在 `apply_affinity_and_corectl` 接线为 `enabled && scenemode_offline` 真门控。
- **`apply_affinity_and_corectl` 末位形参的命名坑（2026-10-01 已修，勿回退）**：该形参原名 `scenemode_offline`，但它实际收到的是调用方的 `scene_mode_active`（**激活标志**），与新配置字段 `CoreCtl.scenemode_offline` **同名不同义**——读代码极易按名字当成开关，写出**极性反转**的判断（「scenemode 已激活」被当成「要下线核」或反之）。现已重命名为 `scene_mode_active`，并在文档注释里显式区分两者：**`scenemode_offline` = 配置开关（是否允许下线核），`scene_mode_active` = 运行期状态（scenemode 是否正在生效）**。改这条链路时务必按语义而非按名字接线；回退 = 还原 `set_power_state(false, config.core_ctl.enabled && config.core_ctl.scenemode_offline)` 的接线并恢复原形参名（不建议）。

- **enable 感知（2026-09-26）**：`CoreCtlCluster.enable: Option<bool>`——真机快照（Alpha07-02）policy0=1/3/0、policy3=3/4/1、policy7=0/1/0（min/max/enable），**仅 big enable=1**：little/prime 的 core_ctl 不可用，读到 enable=false → warn 一次（复用 `max_cpus_warned` 去重，i18n `corectl-enable-off`）后走逐核 offline 兜底；enable=None（读不到）保持「缺失即降级」。**不做 enable 周期重读**（厂商可能动态改，快照仅作省写优化；纠偏 reassert 兜底保留）。

- core_ctl online 读取走 `FastReader`（keep-open File + seek(0) 复用句柄，core_ctl `online_reader()` 与 affinity uclamp 快照共用）：依赖 seek(0)，**不支持 llseek 的 sysfs 节点读失败**；`state()` 首读前误报 Missing。

### 遥测数据源（Telemetry，ChiRi 专属）

- `src/monitor/telemetry.rs` 的 `telemetry_loop` 线程（1s 轮询，仅 `is_chiri_soc()` 时由 monitor/mod.rs 启动）读 PSI（`/proc/pressure/{cpu,io,memory}` 的 some avg10）、GPU busy%（`/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage` → MTK `/sys/kernel/ged/hal/gpu_loading`，缺失为 None）、电池电流/电压。

- **GPU 快照默认关（`GPU_SNAPSHOT_ENABLED=false`，2026-09-22 8550 实测）**：每秒读 Adreno gpuclk / devfreq cur*freq 会把 GPU 从低功耗态唤醒——同场景 playback 1.76→2.59W（**+47%**），而 CPU cpufreq 读取廉价。故 main* snap 的 GPU 4 列恒 `-`（`devfreq_snapshot` 带 allow(dead_code) 保留、`gpu_snapshot_due` 10s 节流留作恢复路径）；曾修三 bug：devfreq/Adreno 单位是 Hz（直写出现 110GHz 荒谬值，>10MHz 按 Hz 处理换算 kHz）、kgsl-3d0 在 devfreq 与 kgsl 两路径同时存在（按设备名去重、devfreq 优先）、governor 只读标准 devfreq（Adreno 私有目录同名节点是纯数字频率值，判无效写 `-`）。telemetry 的 `gpu_busy_percentage` 每秒读**仍在**（采集成本**已评估**：属 logd 侧每秒读，量级远小于 devimp 的 20–28 kB/s；A/B 已按 2026-09-28 R3 口径作废）。

- **msm_performance 参数读数退避（2026-09-26）**：msmp min/max 节点在 PHB110 不可读（即「无锁频盖写通道」），连续 8 次读空进退避、64 tick（≈128s）重试，退避期记 `-`、恢复即清零——勿把恒 `-` 误判为「msm_performance 未启用」。

- 电池读数来源（2026-09-18 重构，用户口径）：**默认走 Android 标准节点**（`/sys/class/power_supply/battery/{current_now,voltage_now}`，ABI = µV/µA）；meta `oplus_chg`（默认 false）打开后**优先读 OPlus 私有节点** `/sys/class/oplus_chg/battery/bcc_parms`（逗号分隔，**0 基下标**：下标 6 = 电芯0电压、8 = 电流、11 = 电芯1电压，即第 7/9/12 个字段；字段按 mV/mA 解读、**原样存不做归一**，两路单位差异全交给校准除数），**节点存在且读到有效值时才只认它**（2026-09-18 用户口径两段：① 同机型的标准通用节点不可信，OPlus 上约 10s 才刷新、单位也不保证——所以私有节点有效时不做回退；② 「存在」本身不等于「有效」，字段缺失/解析失败/全零都算无效，**无效时回退标准节点**，此刻它是唯一还能给数的来源，回退好过整段读数变空，无效原因由 `telemetry-bcc-unusable` warn 打点）：只有节点本身不存在（非 OPlus 机型）或读到无效值时，才走标准节点。**旧的量级启发式（mV/V、mA/A 自动识别）与物理范围门（2–6V / ±30A）已删除**——单位不匹配交给「单位校准」，代码不再猜。OPlus 内核的标准 power_supply 节点约每 10s 才刷新，1s 精度采样必须绕开（私有节点存在的理由）。
- 双电芯与单位校准（同一批 meta 字段）：`oplus_dual_cell`（默认 false，私有节点路径）= **两节并联（2P）口径**：电压取两节**平均**（下标 6 与 11）、电流 **×2**（下标 8 是单节支路电流，整包 = 两节之和；2026-09-18 用户补充）；下标 11 非正视为单电芯布局，电压退回下标 6 且电流不翻倍——**不取电压和**（那是 2S 串压口径，标准节点只报 ~4V 的机器上功率会翻倍）；判据仍是私有节点算出的 P 与「标准节点 V × I」同量级（±20%）；`voltage_double` / `current_double`（默认 false，**仅标准节点路径**）分别把电压 / 电流 ×2（双电芯机型上标准节点常只报单节/单芯值）——**与 `oplus_chg` 互斥**：`chiri::config::Config::load` 推给遥测层时强制 `!oplus_chg && x`，WebUI 打开私有节点时在同一笔写入里清掉这两项并置灰（`state.setBatteryFields` 内联合并，避免「私有开 + 倍压还开着」的中间态被热重载读到）。`voltage_divisor` / `current_divisor`（**各默认 1000000**，须 > 0）：**全链没有内置换算**（2026-09-18 用户要求「去除所有内置校准」），`节点原始值 ÷ 校准值` 直接就是输出——电压得 **V**、电流得**安培**（`batt_current_ma()` 的名字与 status.csv 的 `batt_current_ma` 列名都是历史遗留，值就是安培；该列 3 位小数），`batt_power_w` = 电流值 × 电压 = W。校准值 = 「原始单位 → 输出单位」的除数：**缺省 1000000 正是标准 Android ABI 口径（节点报 µV/µA）；OPlus 私有节点 bcc_parms 报 mV/mA → 实际会算出 1000，由安装脚本装机时按节点电压位数自动写入**（读取层不做任何归一——原先私有节点的 ×1000 与标准节点的 ÷1000 两步都已删除，两张基准靠这两个值区分）——`Telemetry::batt_voltage_v` / `batt_current_ma` 两个访问器执行，`batt_power_w` 仍是 |mA| × V（W = V×A 保持自洽）。**分开的理由**：节点的电压与电流未必同时错单位，共用一个值会让功率按平方变化、也说不清是谁的锅。旧键 `unit_divisor`（曾把两者合一）仍接收，等价于只设电压校准（`MetaYamlFile` 保留该字段 + `parse_disk_meta` 取 `.or(...)`，避免老文件被 `deny_unknown_fields` 判非法整份重置）。六个字段都是「出现即校验、缺省沿用内嵌默认」，经 `telemetry::set_battery_options` 落到进程级原子量（1s 遥测线程与各消费点读它，不每轮解析 YAML）。

- **安装期自动校准（2026-09-20，`customize.sh` 的 `[battery-detect]`）**：包内两个 divisor 默认 **1000000**（标准 Android ABI 的 µV/µA 口径）；安装器按机器把两个值改写成实际口径，否则电压/电流差三个数量级。安装器在「不保留配置」分支里只改 `$MODPATH`（= modules_update 暂存）下**当前机型**那一份 meta.yaml——完整安装由安装器落地、热更新由 `cp -r` 覆盖到 live 目录，**两条路同源**，之后才按分支决定「报安装完成退出」还是「走热更新的复制 + 删暂存 + abort」。**两条路走同一个入口 `apply_divisor_from_raw`**：按**节点电压原始值的位数**推算除数并写入两个 divisor（原始值 n 位 → 除数 = 1 后跟 n-1 个 0，4382→1000、4382000→1000000；电压与电流同值——同一节点两路单位一致），只是原始值来源不同——读到 `bcc_parms` 有内容 → `oplus_chg` 置 true 且取**下标 6（电芯0电压）**；读不到（或读到空）→ 取 `/sys/class/power_supply/battery/voltage_now`。**不做任何硬编码**：私有节点通常报 mV/mA 会算出 1000，万一某机型报 µV/µA 也能自动算成 1000000；缺省 1000000 已是 ABI 标准，这条启发式只在节点单位不是 ABI 标准时才改变它。**首位不是 3/4、读数非数字、字段不足（<7 项）或节点不可用就一律不写**，宁可留默认值让用户在 WebUI 电池读数页改（量级填错会让功率整条曲线失真）。这与上面「daemon 不做量级启发式」不冲突：**daemon 运行期仍然不猜，只有安装器在装机那一刻按节点量级写一次**。另修正两处：① 双电芯判据原来直接 `cut -d',' -f 12` 判非空——**字段不足 12 个时 cut 会把整行原样输出**（无分隔符的行默认透传），单电芯机型被误判为双电芯；现先数逗号确认 ≥12 个字段、且第 12 个字段为正数（与 `read_oplus_bcc` 同口径）；② 热更新末尾「恢复备份」只在**保留配置**那条路执行：选了覆盖却把旧 `config/meta.yaml` / `rules.yaml` 盖回去，会把刚写进去的校准倍数顶掉（回退机型的生效配置就是根上那份 `config/meta.yaml`；ChiRi 机型的生效配置是 `config/<soc>/meta.yaml`，本就不在这条备份链里，行为本就不一致）。

- **启动期一次性单位自检（2026-10-20，`src/monitor/battery_unit.rs`）**：main 在 meta.yaml 自愈（`sync_meta_snapshot`）与首次 `Config::load` 之后异步起 `battery_unit` 线程；只在 `battery/status == discharging` 时按 1s 采样 `batt_power_w()`：连续 15 次 **< 0.1W**（0 视为占位读数不计）→ 两 divisor 修为 **1000**（mV/mA 口径）、连续 15 次 **> 100W** → 修为 **1000000**（µV/µA 口径），命中打 `telemetry-unit-fix-low/high` WARN 后退出；连续 10 次落在 0.1~100W 正常量级也判定除数正确并退出（避免深睡空闲功率 < 0.1W 时误判）；充电/满电/未知态由 power_supply uevent（`screen_detect` 的 netlink 线程 `notify_power_supply`，30s 兜底轮询）唤醒等待，绝不按充电态校准。修正先落 `set_battery_divisors` 原子量，非 nofix 时把 `voltage_divisor`/`current_divisor` 两行一次写回 meta.yaml（触发 Chiri config_watcher 热重载，幂等）。这不是运行期持续启发式：仅在启动后放电态判一次，之后仍按配置值工作。

- 结果写入进程级共享原子量 `telemetry()`（f32 bit pattern 存 `AtomicU32`，与热保护/触摸状态同口径），不占事件通道容量。

- eBPF 扩展探针（`yumi-ebpf/src/main.rs` 的 `handle_sched_wakeup`/`handle_sched_migrate_task`/`handle_cpufreq_transition`，PerCpuArray 计数）由 `cpu_monitor.rs` 仅在 ChiRi 上可选挂载（内核缺 tracepoint 时 warn 一次跳过，不影响主探针），每 2s 读累计值取增量发 `DaemonEvent::BpfStats`（非 ChiRi 机型不发送）。

- chiri scheduler_ipc 以 `TELEMETRY_LOG_INTERVAL=1s` 消费：写 `logs/status.csv`（logger.rs `status_log_snapshot`，1s 一行，功耗精度 1s）+ 20s 一条 debug 摘要 `telemetry-summary`。BpfStats 不刷新 CLG 看门狗心跳（探针失效不影响负载源判定）。

- BCC 失效可见性（2026-09-11）：`bcc_parms` 下标 6/8（电压/电流）缺失或非整数时此前 `?` 静默回退标准节点，「BCC 从未生效、功耗列一直来自 10s 缓存节点」完全不可见（8550 实测 batt_current_ma 99% 在 ±5、74% 为 0）。现 warn 一次 `telemetry-bcc-unusable`（去重）后回退，便于从 daemon.log 确认功耗列口径。

- **daemon.log 追加失败可见性（2026-10-05）**：`logger.rs` 的 `append_open` `.ok()` 与写失败分支原先全程静默；现用 `APPEND_FAIL_STREAK` + `report_append_failure()` 累计失败，首次与每 100 次各打一条 warn（新 i18n key `logger-append-failed`），成功写入即归零。`APPEND_REPORTING` 防告警自身重入递归，报告必须在锁外发。该机制**只解决可见性，根因未定**。

- **未提交 WIP ①（用户工作区，勿擅改）**：Sched 内核参数微调（2026-09-20，借鉴 LittleYouran CTS 的 Scheduler 段）：feature.yaml 新增 `Sched` 段（enabled + params map），`scheduler.rs::apply_sched_params` 按 `SCHED_ALLOWED_PARAMS` 白名单写 `/proc/sys/kernel/<key>`（越界键 warn 跳过、空值跳过，随 apply_system_tweaks 热重载生效）。8550/8475/8998 默认开启 `sched_migration_cost_ns: 200000` / `sched_nr_migrate: 27`；根兜底 `feature.yaml` 无 `Sched` 段、默认关闭，与三 SoC 不一致（骁龙 855 八核经验值：迁移更及时、单轮迁移量收敛）。i18n：`sched-tuning-applied` / `sched-tuning-key-rejected`。DOWN 停摆期间随其余一次性调整一并不下发（进入时按快照还原已写的节点），退出停摆补发。

- **未提交 WIP ②（用户工作区，勿擅改）·子进程名一律预处理为主包名（2026-09-20，借鉴 CTS 的 AppModeConfig::resolve，用户口径）**：`com.xx:push` 本质上是 `com.xx`（子进程与所属包在调度语义上就是同一个应用；厂商框架把 cmdline 首段写成 `pkg:proc` 同理），所以**归一到主包名再放进整个调度计算，下游走原本的算法与流程**，不做进程级粒度区分。归一在两侧各做一次、口径一致：① 前台名在**唯一入口** `app_detect::set_current_package` 归一（此后 `get_current_package()`、ModeChange/PackageSwitch 事件、main\_ 分组、通知、亲和拿到的都是主包名）；② 规则键在 `deserialize_app_modes` 归一（冲突时保留后者并打 `rule-key-conflict`，不静默丢规则）。因此 `determine_mode` 里就是原本的 `app_modes.get(pkg)` 精确查表，**没有回退链**——回退链会让「规则键粒度」与「运行期粒度」两套语义并存。i18n：`app-detect-pkg-normalized`（debug）/ `rule-key-normalized` / `rule-key-conflict`。

- **未提交 WIP ③（用户工作区，勿擅改）**：`module/system.prop` 的 `persist.sys.horae.enable=1→0`，提交前必须确认。

### 2026-09-28 批量改动回退表（生产测试用）

**三条缺一即视为「未定义回退」**：改动 / **可观测的**回退触发条件 / **字段级可逆**的回退动作。判据一律落到可观测项（掉帧 / 滑动手感 / 首帧与启动时长 / 绑定变化率 / 稳态频率 / 功耗分位）；**本项目禁止 A/B 对照**，只能用长时观察，故触发条件必须是「同场景长时观察可见」的量，不能是差值判据。留档：本表 + 本文件各小节的改动理由与口径（2026-10-20 起台账 07 只记「还没做/等拍板」的条目，已落地改动的回退说明一律留在本文件）。

- **天花板全拆（2026-09-28）**：4 份 feature.yaml + tuned_profiles.yaml 的 `perf_ceil` / `headroom` / `per_cluster` 天花板整段删除。**触发**：视频/游戏同场景稳态功耗明显上抬且 T11 未补回，或热档触发频率变高。**回退**：把对应文件的 `perf_ceil` 改回旧值——8550 reduce 0.60 / 兜底 reduce 0.70、default 0.80；8475 reduce 0.70、default 0.80；8998 reduce 0.60、default 0.80；akmode 0.95；playback `per_cluster` little 0.55 · big 0.75 · prime 0.50 + `headroom` 0.85。
- **T11 稳态下探（2026-09-28）**：CLG 与特调各一组 `steady_decay_*`（缺省 `enabled=true`）。**触发**：省电档/均衡档出现周期性卡顿或频率锯齿，或稳态频率反而比改动前更高。**回退**：`steady_decay_enabled: false`（单档即可，CLG 与特调各一处）；或把 `steady_decay_max` 调小。
- **三模式取点（8550，2026-09-28）**：`reduce.steady_decay_target_ratio: 0.80`。**触发**：省电档明显不跟手。**回退**：删掉该一行 `reduce.steady_decay_target_ratio: 0.80`（回缺省 0.90）。
- **CLG-PF 查表（2026-09-28）**：**触发**：出现 `clg-pf-fallback` 告警，或稳态频率下移但掉帧增加。**回退**：`module/config/sm8550/soc.yaml` 生成块内 `enabled: false`（或重跑 `scripts/frontier_policy.py --disable` 再重编；块内禁手改）。
- **A1 后台降权 25（2026-09-28）**：`Affinity.background_uclamp_max_pct` 由 50 降到 25。**触发**：后台同步 / 消息推送时延可感知变差。**回退**：`background_uclamp_max_pct: 35`。
- **A3 后台不进 prime（2026-09-28）**：`bg_promote_exclude_prime: true`。**触发**：后台批处理（下载/软解）明显变慢。**回退**：`bg_promote_exclude_prime: false`。
- **big 饱和保护全模式生效（2026-09-28）**：`big_pressure_any_mode: true`。**触发**：default 下后台大量待 promote 不升核、任务堆积。**回退**：`big_pressure_any_mode: false`。
- **A6 位宽感知（2026-09-28）**：`bitwidth_aware` + `aarch32_clusters`。**触发**：32 位应用关键线程被误判为不可上核、反而变卡。**回退**：缺省已不限制落点（`aarch32_clusters: []`）；彻底不判位宽则 `bitwidth_aware: false`；要恢复限制则填白名单（如 `["little"]`，仅确认系统无转译层的机型才填）。
- **A10 写去重（2026-09-28）**：`write_dedup`。**触发**：掩码漂移未被纠正——`@A` 帧里同一线程 home 与内核实测不一致。**回退**：`write_dedup: false`。
- **F1/F2/F6/F3（2026-09-28 小范围定点修改）**：**触发**：F1 = 退出后 `scaling_governor` 仍为 performance；F6 = 日志 `ok` 但 `min_cpus` 未生效。**回退**：按 commit 反向回退单个文件即可（`git revert` 对应改动，**勿 `git checkout`**）。
- **FDP 放置内核（2026-09-28）**：`fdp_enabled`（缺省 false，8550 显式 true）。**触发**：后台/批处理明显变慢（「该升不升」），或日志 `action=fdp` 的 `net` 符号与真机 `batt_power_w` 走向相反。**回退**：`fdp_enabled: false`（`module/config/sm8550/feature.yaml`）；只想调松不关掉：把 `fdp_cost_hysteresis_w` 由 0.02 降到 0.01，或把 `fdp_dst_freq_cap_ratio` 由 0.85 抬到 0.95。
