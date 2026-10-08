## [maint] AGENTS.md 维护要求

每次对话结束前，回顾本次会话内容，评估是否需要更新本文件：

- 新增/删除/重构了模块、目录或关键文件 → 更新「目录结构」

- 引入了新的依赖、工具链或构建命令 → 更新「技术栈」「常用命令」

- 确立了新的代码约定或踩坑经验 → 更新「代码约定」（可新增「经验教训」）

- 文件描述与实际代码不符 → 立即修正

## [agents] AI 协作分工（2026-09-26 定稿）

**分析数据与改进调度由主线程（主代理）亲自进行；子代理只负责解包和整理数据**——日志判读结论、调度算法改动、代码逻辑修改不下放给子代理（防多代理上下文割裂导致的误判与重复劳动；2026-09-26 内核分析与优化执行会话的教训）。子代理的合规用途：

- 解包日志（dvrun.py 管道）、文件清单/批量信息采集
- 独立代码审查（只读，结论回传主线程裁决）
- 大批量机械性检索

**原始数据整理也是子代理的活**：对 devimp 原始日志的 grep/统计/抽样由子代理完成，产出**整理后的摘要文件**；主线程只读摘要下结论。主线程**严禁任何形式的全量读取**——包括失控的大输出 grep（必须带 head_limit 与精确路径）。

**devimpbin 检索陷阱**：该目录被 gitignore，ripgrep/Grep 默认跳过 → **glob 搜索静默返回假阴性**。必须用 `rg -uu --no-ignore` 或 Grep 工具指向显式文件路径；grep 一律带 head_limit，禁止无界输出。

派子代理前在 prompt 里写明：终端调用上限、scratch 单文件路径、收尾清理要求；派发后定期（隔一段时间）检查子代理是否正常产出（看其 scratch/目标文件的时间戳），失效的及时终止。另两条实测教训：子代理 prompt 要写明「**不要在 extract 输出目录放日志重定向**」（extract 会删掉重建同名目录，重定向文件随之蒸发）；devimpbin/ 禁现场写一次性 probe py，分析一律走预置脚本 `dvrun.py`。

## [analysis] 日志分析链路与判读口径（2026-09-26 沉淀）

**分析入口唯一**：`/devimp-log-analysis` 命令（正文唯一副本 `.cursor/commands/devimp-log-analysis.md`，2026-09-23 由 skill 迁移而来，**勿再建 skill 副本**）；脚本套件 `scripts/devimp/`（dvrun/dvextract/dvlz4/dvmain/dvaff/dvstatus/dvpower/dvcommon + `scripts/devimp-run.cmd`，dvrun 阶段 = extract/analyze/main/aff/status/**power**）。**改 scripts/devimp/ 任何脚本必须同步命令文件「预设脚本」节**（双向同步义务）。dvlz4.py 以「帧头内容大小 = 解出长度」作编码器一致性闸门（lz4_flex 新产物帧头不带 content size 时跳过该闸门，兜底靠 dvextract 的 tar magic 校验）。

**按包功耗归因口径（`scripts/devimp/dvpower.py`）**：功耗权威源是 `status.csv` 的 `charge` + `batt_power_w`；放电方向不用电流符号。`sum(W)/3600` 明确标记为样本等效 Wh，只有等间隔 1s 采样才等同覆盖能量。短区间积分使用有效 dt，不跨 type/charge/mode/package/screen 边界，不外推长缺口和末行；coverage、gaps、max gap 与未知归属区间同时报告。热档行数是样本数，不能等同完整墙钟秒数。devimp 的 `batt_i` 整数化会使排除零电流的统计高估轻载功耗，`batt_p/P_avg` 仅作交叉验证。跨重启 cap 序列不能连读。`dvstatus/dvpower/dvenergy` 支持文件、批次目录和 root；root 默认含全部批次，`--since` 按父目录归档戳筛，无戳报错。

**分解口径**：`cpu_dyn_w` 是未标定动态模型，不是逐秒物理下界；`resid_w` 包含外围、静态和模型误差，保留负值并报告比例。`dvenergy` 总口与模型动态基线取同一低分位子集，各自扣基线再相减得到未解释增量。按来源连续段、同包、模式、屏幕分组，充电/无效数据切段；回归截距不是实测外围地板，低 R² 不能单独归因外围噪声。缺列与全缺值分开提示；FPS 全空不证明 FAS 从未接管；版本身份按解析字段去重，忽略缺失占位而不是按带时间戳整行去重。自检入口 `python -B scripts/devimp/test_accounting.py`。

**定版与 schema**：devimp 头三行 + daemon.log「模块版本:」行定版（详见 02「定版与判读口径」）；目录名不可信、先定版再比数据；机型或系统不同 → 功耗绝对值不可比。

**新记录口径：selfcost 成本摘要（2026-10-07，`devimp/selfcost_<ts>.log`，schema `selfcost/1`）**：每个 `block`（`build`/`write`/`summary`）一行，字段固定为 `schema session window_start window_end block count cpu_valid_count wall_ns_sum wall_ns_max cpu_ns_sum wall_buckets slow_count clock_errors write_errors`（空格分隔 `key=value`）。**读法**：`wall_ns_sum / count` 是墙钟均值，`cpu_ns_sum / cpu_valid_count` 是 CPU 均值——**CPU 分母取 `cpu_valid_count`**，为 0 即该块 CPU unavailable，**不得**把同时输出的 `cpu_ns_sum=0` 当「CPU 耗时为 0」；`wall_buckets` 是 9 个逗号分隔计数（上界 1/2/5/10/20/40/50/100 ms 含上界 + 超界桶），**只报桶范围、不报分位数**；`build`/`write`/`summary` 三段互不重叠，**不能相加**成一个「总同步成本」，也不从进程 CPU 里相减（`/proc/self/stat` 是整进程）。`slow_count` 是墙钟 ≥50ms 的次数，属**纯告警量、不入成本账**（`SLOW_NS` 只限告警）；`clock_errors`/`write_errors` 非零说明该窗口测量不完整或 CPU 分母被污染，须一并报告而非静默丢弃。**状态说明（截至 2026-10-07）**：selfcost 及其配套改动（`diag_cost.rs`/`diag_worker.rs`/`logger.rs`/`chiri/mod.rs`/`power_base.rs`）**尚未经任何编译或测试验证**（Android target 未安装、无设备授权；host `cargo check` 因**两个既存错误**无法全量编译——`screen_detect.rs` 的 Android 专有 `libc::__system_property_get`，以及 `src/chiri/mod.rs:213` 暂存的 FAS 改动 `fas_package_ready` 的 `and_then` 类型不匹配）；本口径描述的是**设计约定**而非实测结论，在取得编译与设备证据前不得据此声称收益或占用降幅。**先前缺陷（本轮已修，尚未验证）**：① 会话结束时 `aff_cost_flush` 之后无下一次 flush，`Summary` 块耗时不会落盘（周期窗口则被归到下一个窗口）——现补**会话结束尾窗排空**；② 写失败后立即 `reset()` 会清零 `write_errors`，失败窗口的结构化摘要丢失——现改为 **window reset 保留错误计数**。**「已修」不等于「已验证」**，两点仍待编译/运行确认。落地经过与回退指引见 `.trae/docs/2026-10-07-selfcost-phase-bcf-results.md`。

**selfcost 读法补充（2026-10-07 第二轮修订，均未验证）**：① **桶边界**按**纳秒精确比较** `wall_ns <= 上界_ms * 1_000_000`、**上界含**、**不做 ms 向下取整**（旧实现先把 ms 向下取整再比，`1.999ms→≤1ms`、`100.999ms→≤100ms`，桶范围系统性低估，已修，并作废了固化旧行为的测试）。② **Summary 行的 `window_*`** 是该写入**实际发生**的区间（`pending_summary` 连同窗口边界留存），**不是被报告时的当前窗口**——按窗口对账时以 `window_*` 为准。③ **worker 故障**：panic 时队列内已入队帧可排空回退到同步路径，但**正在处理的那一帧可能已部分写出、无法回滚**，须按「诊断故障 + 可能部分写入」记录，**不得读作零丢帧**。④ 排空路径 `aff_snapshot_drain` **不受诊断开关门控**（只要求 `devimp/` 目录已存在），与「接收新采样」是两个门。**运行证据**：本轮新增测试**一律未运行**（真实运行过的仅 `diag_cost.rs` 16 passed、`diag_worker.rs` 15 passed、阶段 A Python 30 passed）；全量编译仍被两个**既存**错误阻断（Android target 未安装；`src/monitor/screen_detect.rs:352` 的 `libc::__system_property_get`、`src/chiri/mod.rs:213` 暂存 FAS 改动 `fas_package_ready` 的 `and_then` 类型不匹配）。

**selfcost 读法补充（2026-10-07 第三轮修订，均未验证）**：① **回退出口**——**已接收帧的唯一正确出口是 `aff_snapshot_drain`**（无 `diag_active()` 门控、用**采样时点** ts、返回是否写出成功）；worker panic 排空与 `mod.rs` 提交失败回退都走它；`aff_snapshot` **只用于默认同步路径**（写出时刻 ts、受门控），**不要把两者混称「同步路径」**。② **缺帧账**：`lost_frames` = **真丢/无法回滚**（panic 在飞帧〔partial 风险〕+ 排空 drain **返回 false**），`recovered_frames`（新增查询接口）= 排空 drain **返回 true**（写出成功回退），二者**互斥**；**仍不得读作零丢帧**。③ **身份隔离升级为四重**：(包名 / PID / 进程 starttime〔`/proc/<pid>/stat` 第 22 字段〕/ 配置指纹〔覆盖全量配置的稳定指纹，实现见 `diag_config_identity` doc〕)，身份与 ts **一起在采样时点冻结**（`SampledFrame`）、**不在 build 之后读**；D1 启用前置随之更新为四重。**运行证据**：本轮新增/修改的测试与 `mod.rs` 接线**一律未运行**；真实运行过的仍只有 `diag_cost.rs` 16 passed、`diag_worker.rs` 15 passed、阶段 A Python 30 passed；全量编译仍被两个**既存**错误阻断（Android target 未安装；`src/monitor/screen_detect.rs:352` 的 `libc::__system_property_get`、`src/chiri/mod.rs:213` 暂存 FAS 改动 `fas_package_ready` 的 `and_then` 类型不匹配）。

**selfcost 读法补充（2026-10-07 第四轮修订，均未验证）**：① **配置指纹层级保留**——旧实现把 pretty-Debug 输出**整体排序**，丢掉字段所属层级，交换两个 profile 的参数值即可得到**完全相同**的指纹（**确定性碰撞**，不是「极低概率」），实际配置变化却不递增代际；现改为 `canonical_debug`：按缩进还原层级树、**只对同一父节点下的直接子项排序**，层级与父节点自身行不变。指纹仍**不含版本信息**（`Config` 无版本字段、仅 `Debug`），日后若补 `Serialize` 应改用 `serde_yaml::to_string`。② **采样快照一致性**——`SampledFrame` 在**采样入口一次性取齐** `ts / fg_pid / fg_starttime / config_identity / generation / sequence`，同一 tick 内**先**「身份变化→bump 代际」**后**构造快照；`dispatch` **不再**重新读取代际（消除「新 PID 配旧 generation」「旧配置指纹配新 generation」）。快照只保证**采样那一刻**彼此一致；采样到 dispatch 之间的前台切换会让整帧略滞后于真实当前前台（设计取舍）。③ **panic 排空顺序**——排空改为**逐帧 `catch_unwind`**（写入器二次 panic 只损失**当帧记账**、不中断排空、不让其余已入队帧失去记账）；`submit` 在 `failed` 态**先等待 `drain_finished` 再返回 `Err(task)`**，以此保证**先旧后新**——**不要**说成「文件锁保证顺序」（锁只保证单帧原子写入）；等待**有界**，超时仍返回 `Err` 但须记 error 声明顺序可能被破坏。**运行证据**：`diag_cost.rs` 16 passed、`diag_worker.rs` **15 passed**、阶段 A Python 30 passed；本轮 `mod.rs` 接线与 `logger.rs` 测试**未运行**。

**selfcost build 子账读法（2026-10-09，代码已落、验证待定）**：同一 selfcost 输出中新增 `schema=selfcost-build/1 parent=build`，分 `get_thread_tids`、`tid_stat`（读 stat 含解析）、`snapshot_procs`、`sched_affinity` 与 `residual`。`count` 包括所有实际失败调用，CPU/墙钟均值分别除以 `cpu_valid_count`/`wall_valid_count`；分母为 0 表示该账不可用，不把缺时钟、负残差或溢出读成零成本，须同时报告 `clock_errors`/`residual_errors`/`write_errors`。旧 `selfcost/1` 父账不变，子账只解释 build 内部，**父子绝不可相加**。residual 包含组装/渲染及未归子块的 observer 成本，子块端点读钟也有部分自身成本，不能把 residual 全归为业务热点。60s 输出与关闭尾窗复用现有 selfcost，会话独占；窗口 reset 保留 errors，session reset 全清。worker 仍默认关闭，不降采样、不摘 K3。本轮验证限制见下段，独立审查完成，无剩余阻断发现；编译/运行验证仍受阻，静态审查与旧轮测试记录都不是本轮测试通过证据，真机成本与收益待观察。

**本轮验证限制（主线回传）**：Android `cargo check` 已尝试，只有 `Compiling chiri`、无 `Checking chiri`；`build.rs` eBPF 的 `-Z` 仅 nightly 允许，当前 stable 导致失败。纯模块 `rustc --test` 因项目内 TMPDIR 只读权限失败，测试未执行；两真实模块已由主线加入 `scripts/tests/rust_regression.rs` 持久入口，尚未重跑。`ReadLints totalFiles=0` 不构成有效验证。`mod.rs` 单测 `aff_cost_record` 缺第 7 参已补 `None` 并经同一独立审查员复核有效；独立审查完成，无剩余阻断发现，编译/运行验证仍受阻。

**不接受「计划完整 = 优化完成」（计划 §10）**：专项计划的复选框全部填满**不等于**优化已交付——每阶段必须记录**实际改动 + 静态/测试/设备验收状态 + 受阻项**；未验证项一律标「未验证」，收益未测得时写未验证，不把「计划写全」当作「优化完成」或「编译/测试通过」。未知的非诊断成本可继续未知，但已授权的确定任务必须给出完成或受阻原因。

**判读口径（易踩）**：

- BpfStats（wakeups/migrations）是 2s 增量，换算÷2；over/under = 过 up / 低于 down 的核数；@A 的 `result=e0` = errno 不可得，非成功。
- main\_ 第 15 列 cur_freq_khz（决策选频）与第 16 列 max_freq_khz（上限）不可混算；tick 行 cur_freq_khz 是 CLG/特调动态上限非实际频率。
- cap=85 ≠ 已压制（豁免带覆盖仅 2~5% tick）；cap85 功耗差未做场景归一化，不可直接读作限幅节省。
- 触摸地板只在 Worker flush clamp 生效、不反映在 tgt 列；tuned 路径 over/under/tgt 恒 0 是不写列非异常；FAS 段无 tick（脚本输出 `-` 非 0.0）。
- psi_cpu / gpu_busy / over_cores 只被日志消费、不进任何调度判定。
- **压制判定只看电池温度**（2026-10-04 定案，`mod.rs` 的 cap 分支已不构造 CPU 阶梯）：`cpu_temp` 列（status.csv / snap / `thermal_change` 事件文本）是**遥测**，它缺失或恒 `-` **不代表热保护失效**，判「为什么不压制」只看 `batt_temp` 与 `feature.yaml` 的 `batt_*_temp_c`；反过来说 CPU 温度再高也不会触发 cap。
- **跨机型比 tick 行数/日志体积前，先除掉 8650 的 2× 因子**（2026-10-04 前的包）：8650 把 policy2/policy5 都判成 `big`，而旧版 tick 节流只按 cluster 名做 key，两个 policy 互相刷新签名 → 去重失效、双倍落盘（`devimpbin/1003-213457` 实测 playback big 50 行/簇/秒 vs little 6.67）。修复后 key = policy id + cluster 名。
- daemon.log 是 fluent 本地化文案**非 key 字面量**（搜 key 恒假阴性，先查 `module/config/i18n/zh.ftl` 再搜）；含 U+2068 隔离符需先剥离；Grep/rg 对部分 x\_\*/daemon.log 读不动（编码异常，positive control 零命中一律按未检索处理），rg 本机可能撞 Windows Store stub——用 PowerShell StreamReader 显式枚举兜底。
- daemon 重启会重置热保护 cap=100 → 跨重启 cap 序列不可连读；`scaling_governor` 写 EPERM = 内核本就 schedutil（OEM 锁 governor）。
- `dvaff.py` 的存活集合与 tid 复用口径（2026-09-28 起与写入端契约对齐）：**按帧头 `full=1` 的刷新帧收敛**——连续两次刷新帧都没出现的 tid 视为已退出，从绑定轨迹与合并态统计中移出（旧包无 `full=1` 则不收敛，行为与旧版一致）；`t` 行 **`pid` 变化 = tid 被别的进程复用**（写入端只落一条零槽标识行），此时该 tid 的逐槽合并态整条作废重建，绝不沿用旧进程的 comm/core/pin；**无法解析的 `t` 行**计入报告与摘要的「无法解析丢弃 N」（槽名/槽序与写入端不一致时整行丢弃，N 突然变大先查帧格式而不是解读数据）。`--groups` 非法（非数字 / 端点倒置）与 `--core-top` 越界（须 1..64）直接报错退出 2，不再静默产出空簇或反向切片。

**采集纪律**：**不新增 A/B 对照实验**（2026-09-28 口径：判据只有「逻辑必然 + 模型折算 + 功能回归观察」；"关/开某配置比功耗"属对照实验，一律不做）。采集态自身开销的**正确口径**：8745 `tgtop` 的**负载占比**（待机 logd 17.44% / launcher 15.82% / 游戏 9%），**不是功耗占比**——旧表述"launcher ~18%、视频 ~14%"是转述漂移，勿再引用（详见 `04-hard-lessons.md`）；devimp 滤充电只能 `batt_i<0`；功耗以 status.csv 的 charge+batt_power_w 为准（main* batt_i 列疑似整数化）；跨批次不可比；dev_record 开启约 42min 必撞 128MB 门限强制重启一次（正常现象）。

**沉淀与归档（2026-09-26 已执行）**：全部记录性 md 已沉淀进 `agentsdocs/01~07` 并归档至 **`.archive/`**（镜像原相对路径，如 `.archive/.codebuddy/plans/`）——memory 工作日志（`.codebuddy/.cursor/.trae/memory/YYYY-MM-DD.md`）、计划（`.codebuddy/plans/`、`.cursor/docs/plans/`、`.trae/documents/`、`.trae/specs/`）、分析报告（`.codebuddy/docs/`、`.cursor/docs/` 根与 `kernel-analysis/`）、`mdocs/TechAnalyze.md` 与 `mdocs/updateWith.md`、`.workbuddy/` 整目录。归档后：TODO 台账唯一权威 = `agentsdocs/07-todos.md`，内核机制权威摘要 = `agentsdocs/06-kernel.md`。**保留原位**：README、webui/README、`mdocs/ModeInfo.md` / `socList.md` / 三个 `*-example.yaml` / `8550/sm8550.dtsi`、`updateInformation/`（发版契约文件，空文件待填）、`.codebuddy/memory/MEMORY.md` 与 `.cursor/memory/MEMORY.md`（跨环境长期事实索引）、`.cursor/commands|rules|skills/`、`.trae/rules/`、submodule（AppOptR、LittleYouran_CTS_3）内部文件。仓库内指向归档件的路径已统一改为 `.archive/...`。
