---
name: 项目内容一律留在项目文件夹内
description: 任何产出（记忆/计划/文档/产物/临时脚本）都不得落到项目目录之外，包括工具默认的全局存储路径
type: feedback
---

**规则**：本项目的任何内容——记忆、计划、评审文档、分析产物、临时脚本——**只能写在 `e:\code\ChiRi\` 之内**。发现工具默认把东西写到用户主目录（全局存储、全局计划目录、全局记忆目录）时，必须搬回项目并删除源件的全局副本。

**Why:** 2026-09-28 用户就此严厉纠正（"我说过多少遍了，这个项目的内容绝对不允许离开项目文件夹"）。此前实际发生过的外溢：① 计划被工具写进 `C:\Users\Aibeto Zhu\AppData\Roaming\CodeBuddy CN\User\globalStorage\tencent-cloud.coding-copilot\plans\`；② 记忆写入 `C:\Users\Aibeto Zhu\.codebuddy\projects\e-code-ChiRi\memory\`；③ 该目录下的 `plans/`、`todos/`、`file-changes/` 三处旧会话归档同一类问题（用户明确说这些是垃圾，不要搬进项目）。

**How to apply:**
- 产出落点：AI 文档 → `.codebuddy/docs/`；计划 → `.codebuddy/plans/<会话id>/`；记忆 → `.codebuddy/memory/`；日志分析产物 → `devimpbin/`；临时脚本用完即删。
- 用任何"默认写用户目录"的能力（计划工具、记忆工具、快照）之前先确认落点；事后必须盘点足迹（哪些文件在项目外），把项目内容搬回项目内并清理全局副本。
- 只搬**内容本体**（计划正文、记忆条目）；工具的过程性垃圾（todos.json、file-changes 快照、旧会话归档）不要搬进来，汇报里也按"项目内/项目外"分列。
