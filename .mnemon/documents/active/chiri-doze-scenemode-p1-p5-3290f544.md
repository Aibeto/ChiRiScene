---
id: "3290f544-df25-40da-8e41-c802a1089ea6"
title: "ChiRi 调度改进方案定稿口径：doze=scenemode 浅压档 与 P1~P5 编号"
description: "2026-10-11 方案简化定稿后的口径记录：用户确认「doze 即 scenemode 浅压档（同一模式两深度档）」，全文术语统一与跨文件同步清单，删除项不入正文，编号重排为连续 P1~P5 及各项收益/风险，P1 修 defect 细节与验证判据。"
status: "active"
created_at: "2026-10-11T07:34:23.575Z"
updated_at: "2026-10-11T07:34:23.575Z"
content_hash: "b264b49ef87ae91a4b39665f8ca7471a0c84087be8bd4b6a2e80c1f32182fa89"
source_paths:
  - ".cursor/plans/2026-10-11-调度改进方案.md"
  - ".cursor/docs/2026-10-11-息屏路径归因.md"
  - ".cursor/docs/2026-10-11-scenemode门槛归因.md"
  - ".cursor/docs/2026-10-11-分派共同前提.md"
  - ".cursor/docs/2026-10-11-playback-fas-akmode归因.md"
  - ".cursor/memory/MEMORY.md"
  - "src/chiri/mod.rs"
session_ids:
  - "02410e1b-2a47-4976-8654-37468c83487e"
memory_body_ids:
  []
---

# ChiRi 调度改进方案：定稿口径（术语裁决 + P1~P5 连续编号）

本文件记录 `.cursor/plans/2026-10-11-调度改进方案.md` 在「简化计划书」一轮后的**定稿口径**，补足既有归因摘要文档 [4b0f8120-3d38-43b7-b8c9-59e936651c9d] 中仍按旧编号（P9/P10/P13/CR.1）与旧术语（doze）表述的部分。量化内容、DiD 裁决、能量盘子以该文档为准，此处只记口径与编号。

## 一、术语裁决（2026-10-11 用户确认）

**「doze」不是独立机制，它就是 scenemode 的低功耗浅压档。** 同一状态机、同一配置来源 `module/config/normal/scenemode.yaml`，由 `scene_mode_delay_secs` 决定何时从浅压切到深压：

| 档 | 生效时机 | 生效上限 | 源码 |
|---|---|---|---|
| 浅压档（旧称 doze） | 屏幕一灭立即 | `perf_ceil = .min(0.30)`（以 reduce 起底后覆盖 5 字段） | `src/chiri/mod.rs:3723-3732`，三处同款：`:3321-3329`、`:4223-4233` |
| 深压档 | `scene_mode_delay_secs` 到期（现 300 s，下限 60） | `perf_ceil = 0.12` | `src/chiri/mod.rs:4272-4285`，延迟判定 `:4252` |

推论：任何「深压档相对浅压档 / 相对 reduce 的净省电」都是**档间边际**，不是「开不开 scenemode」的差别；深压档覆盖率变化不能被读成「省电模式开关」。另：浅压档相对 reduce 覆盖 5 个字段（`enabled=true`、`perf_floor 0.05→0.0`、`perf_ceil 1.00→min(0.30)`、`smoothing_up 0.50→0.10`、`touch_boost true→false`）。

**落地要求**：方案正文 `doze` 出现 0 次（原 12 处），统一为「scenemode 浅压档 / 深压档」；函数名 `build_doze_cfg` 改名 `build_scenemode_cfg`。跨文件同步口径（口径类改动不能只改一处）：

| 文件 | 改动 |
|---|---|
| `.cursor/plans/2026-10-11-调度改进方案.md` | 全文改写为 305 行 / ~28.1 KB（原 328 行） |
| `.cursor/docs/2026-10-11-息屏路径归因.md` | 标题改「scenemode 浅压档」+ 顶部口径块 |
| `.cursor/docs/2026-10-11-scenemode门槛归因.md` | 顶部口径块 |
| `.cursor/docs/2026-10-11-分派共同前提.md` | 锚点行改写 |
| `.cursor/docs/2026-10-11-playback-fas-akmode归因.md` | 6 处失效的 `P1 #9/#10` 编号引用（指向已删条目）改为直接命名参数 |
| `.cursor/memory/MEMORY.md` | 长期事实条改写为「一个模式、两个深度档」 |

## 二、定稿编号（连续 P1~P5）

删除项不得留在正文；编号重排为连续，不解释「跳号」。旧→新映射：P1 息屏饱和退出→**P1**；P2 亮屏决策密度→**P2**；~~P3 GPU 协同~~ 删；P4 down 回落→**P3**；P5 daemon `@S`→**P4**；~~P6~P10、P13、CR.1~~ 删；P11+P12→合并为 **P5**（a/b）。

| # | 改动 | 收益 (Wh) | 置信度 | 风险 |
|---|---|---|---|---|
| P1 | 息屏饱和退出应回落 scenemode 浅压档，而非 reduce（**修 defect**） | 0.02 ~ 0.07 | 高 | 低 |
| P2 | 亮屏升频决策密度 + 尖峰削顶 | 0.93 ~ 2.88 | 中 | 中 |
| P3 | down 回落加速（与稳态下探共用阈值，非零风险；与 P2 共享秒数不另加） | 0.10 ~ 0.46 | 中高 | 低 |
| P4 | daemon `@S` 帧构建分片 + 尖峰让路 | 0.15 ~ 0.40 | 中 | 低 |
| P5 | 两处零耗电修正：a) `config.rs:1808` `default_scene_mode_delay_secs` 3000 与 `feature.yaml:20` 300 不一致；b) 组掩码兜底写缺 cpuset 校验 | 0 | 高 | 低 |

确认不做（结论收进 §5.1，不在正文单列已删候选）：调 GPU / 削特调 / 再压浅压档。完整版（含 8 条删除项的评估与跨环境对账）留在 `devimpbin/_tmp/_plan_full_backup.md`，不入正文。

### P1 细节（唯一"修 defect"项）

- 位置：`src/chiri/mod.rs:4360-4368`（scenemode 饱和退出分支，实际行域 4359-4381）；现值只取 `get_clg_cfg(&config, "reduce")` 后 `reload_config` / `init_policies` / `release`，整条分支无 scenemode 配置参与 ⇒ 退出后生效 reduce（`perf_ceil=1.00`、`perf_floor=0.05`、`smoothing_up=0.50`、`touch_boost=true`，见 `feature.yaml:203/201/195/215`）。
- 建议：抽公共函数 `build_scenemode_cfg(&config) -> ClgCfg`（即 `:3703-3733` 含 `:3728` 的 `perf_ceil.min(0.30)`）并复用；当前该构造在三处重复。
- 验证判据：取 `max_util≥0.95` 的息屏 tick，此时 `target_perf = clamp(util × headroom, floor, ceil) = perf_ceil`（`cpu_load_governor.rs:494`），`tgt_perf` 直接读出生效上限——little 应不再出现 1.00（现状 C 档占 16%），表现为 ≤0.30。
- 收益依据：C 档（退出后跑到 reduce 上限的息屏秒）2277 s / 0.3496 Wh，其中高负载 1381 s。

## 三、可复用的写作/文档约定（本轮教训）

1. **口径一旦确认，旧术语应从全文消失**：留着旧词再加一条勘误，等于让每个读者每次做一次心智转换；半新半旧的调和说法（如「doze 就是 scenemode 的第一段」）还会暗示存在两个独立机制。
2. **不要被历史编号绑架**：已删条目不应留在正文（读者是要落地的人），更不要为「编号跳号」写解释；想留痕放 `devimpbin/_tmp/` 备份，正文不放。
3. 报告里的编号引用（`P1 #9/#10`）在条目删除后即成哑引用，应改为直接命名参数而非保留编号。
