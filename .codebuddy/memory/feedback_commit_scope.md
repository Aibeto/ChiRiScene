---
name: "写 commit" 只产出信息，不执行 git commit
description: 用户说「写一份 commit / 写提交信息」时只给文案，执行 git commit 前必须另有明确指令
type: feedback
---

用户说「根据暂存的更改写一份 commit」时，只输出提交信息文案，不要执行 `git commit`。

**Why:** 2026-09-23 我把这句话当成「生成并提交」，直接 commit 了 54 个文件（5505a31）。用户明确纠正：没让你提交。本仓库的长期约定也是「评估与准备 ≠ 批准开工」，写 Git 历史属于不可轻易回退的动作。

**How to apply:** 涉及 `git commit` / `git push` / `git reset` 等会改写仓库历史的命令，一律先给方案（含提交信息文案）并等用户确认；只有用户明确说「提交」「commit 吧」「执行提交」时才动手。git-commit 技能也照此执行——走到「生成 message」为止。
