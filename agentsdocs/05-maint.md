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

**按包功耗归因口径（2026-09-27 固化，`scripts/devimp/dvpower.py`）**：功耗**唯一权威源 = `status.csv` 的 `charge` + `batt_power_w`**——产出按 `mode×package` 与按包两张表（n / avg / p50 / p95 / max / 温度 / cap / gpu / psi）、ΣW/3600 的累计 Wh 与各包占比、`thermal_cap_pct` 各档累计秒数。devimp 侧 `batt_p`/`P_avg` 只作交叉验证：本机 `batt_i` 整数化（值域 0/1/2/3），`devimp-analyze.py` 的「严格排除 `batt_i==0`」会删掉约 58% 的 <0.5A 轻载秒 → **系统性高估**（2026-09-27 实测 playback devimp 3.42 W vs status.csv 2.92 W）。放电方向一律按 `charge` 列判，**不用电流符号**。热档「常态还是偶发」看 `thermal_cap_pct` 各档**累计秒数**而非均值/事件条数（2026-09-27 实测：85 档 10422 s / 64%、100 档 5312 s / 33%、40 档 530 s / 3.3% → 热是常态背景）；daemon 因 `devimp/` 触顶 128MB 会多次重启、每次把热状态重置回 100（4.6 h 内 3 次）→ **cap 序列与各档占比都不可跨批次连读**。`--since MMDD-HHMMSS` 对 status.csv 只能按父目录 `x_<MMDD-HHMMSS>/` 批次戳筛（status.csv 的时间戳无日期）。

**定版与 schema**：devimp 头三行 + daemon.log「模块版本:」行定版（详见 02「定版与判读口径」）；目录名不可信、先定版再比数据；机型或系统不同 → 功耗绝对值不可比。

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
