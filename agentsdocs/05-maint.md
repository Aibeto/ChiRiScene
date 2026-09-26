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

**分析入口唯一**：`/devimp-log-analysis` 命令（正文唯一副本 `.cursor/commands/devimp-log-analysis.md`，2026-09-23 由 skill 迁移而来，**勿再建 skill 副本**）；脚本套件 `scripts/devimp/`（dvrun/dvextract/dvlz4/dvmain/dvaff/dvstatus/dvcommon + `scripts/devimp-run.cmd`）。**改 scripts/devimp/ 任何脚本必须同步命令文件「预设脚本」节**（双向同步义务）。dvlz4.py 以「帧头内容大小 = 解出长度」作编码器一致性闸门（lz4_flex 新产物帧头不带 content size 时跳过该闸门，兜底靠 dvextract 的 tar magic 校验）。

**定版与 schema**：devimp 头三行 + daemon.log「模块版本:」行定版（详见 02「定版与判读口径」）；目录名不可信、先定版再比数据；机型或系统不同 → 功耗绝对值不可比。

**判读口径（易踩）**：
- BpfStats（wakeups/migrations）是 2s 增量，换算÷2；over/under = 过 up / 低于 down 的核数；@A 的 `result=e0` = errno 不可得，非成功。
- main_ 第 15 列 cur_freq_khz（决策选频）与第 16 列 max_freq_khz（上限）不可混算；tick 行 cur_freq_khz 是 CLG/特调动态上限非实际频率。
- cap=85 ≠ 已压制（豁免带覆盖仅 2~5% tick）；cap85 功耗差未做场景归一化，不可直接读作限幅节省。
- 触摸地板只在 Worker flush clamp 生效、不反映在 tgt 列；tuned 路径 over/under/tgt 恒 0 是不写列非异常；FAS 段无 tick（脚本输出 `-` 非 0.0）。
- psi_cpu / gpu_busy / over_cores 只被日志消费、不进任何调度判定。
- daemon.log 是 fluent 本地化文案**非 key 字面量**（搜 key 恒假阴性，先查 `module/config/i18n/zh.ftl` 再搜）；含 U+2068 隔离符需先剥离；Grep/rg 对部分 x_*/daemon.log 读不动（编码异常，positive control 零命中一律按未检索处理），rg 本机可能撞 Windows Store stub——用 PowerShell StreamReader 显式枚举兜底。
- daemon 重启会重置热保护 cap=100 → 跨重启 cap 序列不可连读；`scaling_governor` 写 EPERM = 内核本就 schedutil（OEM 锁 governor）。

**采集纪律**：任何 A/B 必须同 dev_record 状态（采集自身 launcher 场景占 ~18%、视频 ~14%）；devimp 滤充电只能 `batt_i<0`；功耗以 status.csv 的 charge+batt_power_w 为准（main_ batt_i 列疑似整数化）；跨批次不可比；dev_record 开启约 42min 必撞 128MB 门限强制重启一次（正常现象）。

**沉淀与归档（2026-09-26 已执行）**：全部记录性 md 已沉淀进 `agentsdocs/01~07` 并归档至 **`.archive/`**（镜像原相对路径，如 `.archive/.codebuddy/plans/`）——memory 工作日志（`.codebuddy/.cursor/.trae/memory/YYYY-MM-DD.md`）、计划（`.codebuddy/plans/`、`.cursor/docs/plans/`、`.trae/documents/`、`.trae/specs/`）、分析报告（`.codebuddy/docs/`、`.cursor/docs/` 根与 `kernel-analysis/`）、`mdocs/TechAnalyze.md` 与 `mdocs/updateWith.md`、`.workbuddy/` 整目录。归档后：TODO 台账唯一权威 = `agentsdocs/07-todos.md`，内核机制权威摘要 = `agentsdocs/06-kernel.md`。**保留原位**：README、webui/README、`mdocs/ModeInfo.md` / `socList.md` / 三个 `*-example.yaml` / `sm8550.dtsi`、`updateInformation/`（发版契约文件，空文件待填）、`.codebuddy/memory/MEMORY.md` 与 `.cursor/memory/MEMORY.md`（跨环境长期事实索引）、`.cursor/commands|rules|skills/`、`.trae/rules/`、submodule（AppOptR、LittleYouran_CTS_3）内部文件。仓库内指向归档件的路径已统一改为 `.archive/...`。

