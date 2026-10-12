---
id: "2fb841f8-152d-4682-80a9-87d85c3a46bb"
title: "ChiRi CLG 高位升频门槛（high_zone）机制与全性能可达审计（2026-10-13）"
description: "2026-10-13 CLG high_zone 升频门槛机制定稿：设计、配置、回退、验证数字与全性能可达审计结论，含提交 ebfe0c3。"
status: "active"
created_at: "2026-10-12T20:49:03.890Z"
updated_at: "2026-10-12T20:49:03.890Z"
content_hash: "c63cf181d9abc4d8b376a15331ca8aef07d3149f18e23964cf34a49754ecb8ad"
source_paths:
  - "src/chiri/cpu_load_governor.rs"
  - "src/chiri/config.rs"
  - "module/config/8550/feature.yaml"
  - "scripts/tests/clg_tuning_regression.rs"
  - "scripts/tests/clg_replay.rs"
  - "scripts/devimp/replay_clg.py"
  - "webui/src/views/LogsView.svelte"
  - ".cursor/memory/MEMORY.md"
  - ".cursor/memory/2026-10-13.md"
  - ".trae/rules/git-commit-message.md"
session_ids:
  - "3f1eb7f6-a9b6-4600-9b11-a0afaaa01200"
memory_body_ids:
  []
---

# ChiRi CLG 高位升频门槛（high_zone）与全性能可达审计（2026-10-13）

已提交 `ebfe0c3`（feat(chiri/clg): 添加高位升频门槛并关闭状态表温度列，9 文件 +195/−6）。相关：P1~P5 编号口径见 `3290f544-df25-40da-8e41-c802a1089ea6`；R2「天花板全拆」/ R7「取点不封顶」否决边界见 `4b0f8120-3d38-43b7-b8c9-59e936651c9d`。

## ① 全性能可达审计结论

未发现「正常模式无法用满任意核心」的封顶类缺陷；真正松动点在高频区升频门槛，已修。

| 层 | 检查 | 结论 |
|---|---|---|
| 配置 | 全部 `perf_ceil`/`headroom`（8 文件 46 处） | 全 1.0/≥1.0，唯一天花板 = scenemode 0.12（息屏轴，符合 R2） |
| 算法 | 热压制 `flush`（src/chiri/cpu_load_governor.rs:601） | 只 clamp 写侧、不回写 `current_perf`，窗口解除即全速，无卡死路径 |
| 算法 | PowerBase 档位（src/chiri/power_base.rs:478） | 档位是「上界取点」非永久封顶：overload 50% 核 ≥99.5% 占用持续 3s 越过功耗闸门、5s 防篡改重写、触摸瞬时升档，符合 R7 |
| 门槛 | 升频决策（修复前） | 风险点：perf ≥85% 后升频只看 `util×headroom`，单核毛刺即可推高上限——路径太松，非到不了满频 |

## ② high_zone 机制（src/chiri/cpu_load_governor.rs:508，`[high_zone]` 块）

- 触发：`current_perf ≥ high_zone_gate`（8550 = 0.85）且目标继续上抬时，需本簇「≥N 核 util ≥ high_zone_util」确认（8550：big/prime 2/4=50%，little 2/3≈67% 略紧防 A510 贴顶）
- 未确认 → 冻结 tick（decision=`hz_hold` 早退，不动 `up_wait`/`down_wait`，降频零影响）；连续阻塞满 `high_zone_hold_ticks`（20）强制放行一次并重新计数，防确认条件与负载形态错配永久卡高位
- 只压决策意图：不封顶、不回写 `current_perf`、不碰写频/热钳/触摸/PF 路径 → R2/R7 合规，随时可到硬件最高频
- 启用面：`CpuLoadGovernorConfig` 新增 `high_zone_gate/count/util/hold_ticks` 四字段（serde 缺省 count=0 关闭），normalize 钳 [0,1]；8550 reduce/default/boost 三段 `gate: 0.85 / count: 2 / util: 0.90`（module/config/8550/feature.yaml）；其他机型 count 缺省 0 = 关闭，行为逐位不变
- boost 段保留响应优势：`up_threshold` 0.55 < 0.85，起步 perf 0.60 直接过 gate
- 回退：删 8550 三段的对应行（count 归 0）即机制关闭

## ③ 验证（本机 android check 需 NDK，本机无，CI 有——按逻辑审查）

- replay 构造性自检：little/big/prime 三簇全过，含新断言「单核阻塞 + hold 兜底 2000 tick 内仍精确到 perf=1.0 与 hw_max」
- 真实数据回放（logd_1011-000909）：EXIT 0，`hz_hold` 3660 tick 正常记账
- 契约/回归：clg_tuning 5/5（新增 `high_zone_gate_knobs`）、rust_regression 34/34、affinity_cpuset 17/17、diag_slice 6/6、svelte-check 0 错、vitest 88/88

## ④ 同 commit 附带：WebUI 温度列关闭

`webui/src/views/LogsView.svelte` 状态快照表删电池温度列（th+td），zh.ts/en.ts 删 `logs.status.batt` key。数据解析契约（`battTemp` 字段、status.csv 列）原样保留，纯展示层关闭，随时可恢复。

## ⑤ 提交规范（同步自 .trae/rules/git-commit-message.md）

type(scope) 中文祈使句主题行，scope 用子作用域（如 chiri/clg）；长提交信息先 Write 到 `devimpbin/_tmp/*.txt` 再 `git commit -F`。
