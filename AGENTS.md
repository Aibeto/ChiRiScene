# AGENTS.md

> 上次更新时间：2026-09-26

本文档为 AI 编程助手（Cursor / Claude Code / Trae 等）在本仓库工作时的指导文件。
2026-09-20 起按块拆分：本文件只作**索引引导**，正文位于 `agentsdocs/` 子块文件。

> ** 协作文件按环境分文件夹写入**
>
> - `agentsdocs/*.md`：本文件拆分出的**指导正文**（原 AGENTS.md 各区块，Grep `\[tag\]` 定位，勿通读全文）。
> - **哪个环境的助手，就写入哪个环境的文件夹**：Cursor → `.cursor/`、CodeBuddy → `.codebuddy/`、Trae → `.trae/`；
>   只写自己环境的，勿写别人的。每个环境文件夹内结构一致：
>   - `memory/`：跨会话记忆，注意它是一个**目录**而不是单一文件——
>     `MEMORY.md` 放长期事实（就地更新、保持精简），`YYYY-MM-DD.md` 是按日期追加的工作日志（只追加不重写）。
>   - `docs/`：计划、评估等 AI 产出文档（`mdocs/` 只放项目原有文档，勿把 AI 产出放进去）。
> - **任何文件都不离开项目文件夹**（否则后期整理找不到）；工具自动写到项目外的副本
>   （如 Cursor plans 目录）复制回项目对应环境文件夹后删除原件。
> - 各环境 `memory/` 互读不互写：`.codebuddy/memory/MEMORY.md` 存有 2026-09-13–09-23 统一口径时期
>   积累的长期事实（工具链口径、守护进程契约），各环境开工都读、只读。
> - 旧的 `.workbuddy/` 已废弃合并，其下只剩重定向说明文件，不再写入任何内容。
>   凡属架构约定、契约数字、命令口径的变更，`agentsdocs/` 与本环境 `memory/MEMORY.md` 都要同步；
>   具体某次改动的经过只写本环境当日日志。
>   **开始工作前互相阅读**：读本文件 + 本环境 `memory/MEMORY.md` 与部分 `YYYY-MM-DD.md`
>   - `.codebuddy/memory/MEMORY.md`（历史长期事实）。
> - **2026-09-26 沉淀归档**：所有记录性 md（memory 工作日志、plans、分析报告、内核分析、TechAnalyze）
>   已并入 `agentsdocs/01~07`，历史原件移至 **`.archive/`**（镜像原相对路径，如 `.archive/.codebuddy/plans/`）
>   ——TODO 台账唯一权威 = `07-todos.md`，内核机制权威摘要 = `06-kernel.md`；
>   归档/保留清单见 05 的「沉淀与归档」。

## 区块索引

| 区块                                     | 子块文件                                                       | 内容                                                                                                                                                                                             |
| ---------------------------------------- | -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `[overview]` `[tree]` `[stack]` `[cmds]` | [agentsdocs/01-overview.md](agentsdocs/01-overview.md)         | 项目概述、目录结构、技术栈、常用命令                                                                                                                                                             |
| `[convention]`                           | [agentsdocs/02-convention.md](agentsdocs/02-convention.md)     | 代码约定：架构事件流 / 日志 / 配置嵌入与热重载 / 实验室 / i18n / Rust 风格 / 版本发布 / WebUI                                                                                                    |
| `[chiri]`                                | [agentsdocs/03-chiri.md](agentsdocs/03-chiri.md)               | ChiRi 调度子系统：特调 / FAS / CLG 语义 / Thermal / 触摸升频 / 息屏省电 / 极速 / 亲和迁移 / core_ctl / 内核 Sched 参数 / 遥测（DOWN 停摆的下发与还原见 `chiri/scheduler.rs` 与 02 的 DOWN 小节） |
| `[hard]` `[lessons]`                     | [agentsdocs/04-hard-lessons.md](agentsdocs/04-hard-lessons.md) | 硬性约束、经验教训                                                                                                                                                                               |
| `[maint]` `[analysis]`                   | [agentsdocs/05-maint.md](agentsdocs/05-maint.md)               | 本文档维护要求 / AI 协作分工 / 日志分析链路与判读口径 / 沉淀与归档指引                                                                                                                           |
| `[kernel]`                               | [agentsdocs/06-kernel.md](agentsdocs/06-kernel.md)             | SM8550 内核机制分析：平台基准 / DT 对账 / 调频竞争者 / WALT-EAS / Thermal / cpuidle-core_ctl / 真机实测档案                                                                                       |
| `[todos]`                                | [agentsdocs/07-todos.md](agentsdocs/07-todos.md)               | 未竟事项台账（唯一 TODO 权威）：性能 backlog / 内核取证 T1-T10 / 调度机制验证 / WebUI 工具 / 配置文档杂项                                                                                        |

快速定位：`Grep '\[tag\]' agentsdocs/` 打到具体子块文件再读对应小节，勿通读全文。
