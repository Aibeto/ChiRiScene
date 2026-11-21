---
id: "1ff2d619-195a-4bcd-810e-9667e574a8ba"
title: "ChiRi @S TID 有界采样与 selfcost-build/2 契约（2026-11-21）"
description: "@S 同步快照的有界并行 TID stat 读取、顺序差分和 selfcost-build/2 成本口径；用于后续维护与真机验收。"
status: "active"
created_at: "2026-11-21T07:09:33.927Z"
updated_at: "2026-11-21T07:09:33.927Z"
content_hash: "6563547b297b5afd184d6944abe4229bfbf0e8ae12b323e6eb8320b1e66250a3"
source_paths:
  - "src/chiri/mod.rs"
  - "src/chiri/diag_build_cost.rs"
  - "src/chiri/diag_slice.rs"
  - "agentsdocs/02-convention.md"
  - "agentsdocs/05-maint.md"
  - "agentsdocs/07-todos.md"
session_ids:
  - "session-547e8bd8-29af-40f5-9e93-9b6eb70405c7"
memory_body_ids:
  []
---

# ChiRi @S TID 有界采样与 selfcost-build/2 契约

## 动机与证据边界
九包日志在 `devimpbin/1116-003204/x_1114-010906/daemon.log:4953` 记录单次 `build=7800.8ms`、`write=5.2ms`；`devimpbin/1117-215156/x_1114-202231/daemon.log:2215` 另有 `build=7326.8ms`。同一长尾窗口里 scheduler tick gap 同时偏大，但时间重叠只能说明相关，不能据此把所有停顿归因到某个单独子项。`selfcost` 的 residual 仍含循环外、锁、差分、渲染、yield 和 observer 开销。

## 采样契约
- `src/chiri/mod.rs` 的 `build_aff_snapshot` 继续同步完成；本优化只把 `/proc/<tid>/stat` 读取改为复用有界线程池，不把 render/write 混作 TID worker。
- 固定 4 个采样 worker、每 chunk 16 个 TID、最多 8 个 in-flight chunk。空输入不创建任务；不超过 16 个 TID 时由 caller 直接读取，避免单 chunk 的无效线程/channel handoff。线程池初始化失败时整批退回 caller 串行读取。
- 完整候选集、每秒帧节奏、所有诊断字段、线程退出时 `None` 语义及刷新 anchor 均保留；active 与冷候选刷新补采均每个 TID 至多读一次。样本按输入索引放回，差分仍由 caller 在 `AFF_TH_STATE` 锁内按原稳定 TID 顺序串行应用，并使用原帧时间戳 `now`。不降采样、不截帧、不丢候选。reader panic 会完成已派发读取后在 caller 线程恢复 panic。
- `diag_slice` 的差分循环让出间隔为每 256 个候选；该值只调整 yield cadence，不改候选数、日志字段或刷新周期。

## selfcost-build/2 口径
`src/chiri/diag_build_cost.rs` 将并行 `tid_stat` 的 `count` 记为读取尝试数；wall 和 caller thread CPU 按一次整批区间计量，避免将重叠 worker wall 相加或把 worker CPU冒充caller CPU。worker CPU 用 `worker_cpu_expected_count`、`worker_cpu_valid_count`、`worker_cpu_ns_sum` 独立观测；`worker_cpu_overflow=true` 表示总和不可作为有效总量。worker CPU 不参与父级 caller CPU residual 扣除。缺失时钟、溢出或负 residual 不得伪装成有效零。schema 为 `selfcost-build/2`；旧 selfcost/1 父账未变，父子账不得相加。

## 回归与设备验收
测试锁住稳定顺序、每候选只采一次、worker 上限、panic 时读完已派发任务、≤16 caller 直读，以及批次wall/caller CPU/worker CPU分账和溢出标志。2026-11-21 `cargo test --bin chiri --quiet` 为 287 passed、3 failed、1 ignored；3 个失败位于 `src/chiri/fas_manager.rs:588,683,694`，依赖宿主不可用的 PID/starttime/设备白名单桩，不是本次采样测试。
真机收益尚未证实。单版本目标设备须核对完整快照样本/字段数与帧频不变，`@S build` 和 scheduler tick gap 的长尾，并同时观察 worker CPU、daemon CPU、低负载 `batt_power_w` 与高负载帧间隔；不得用降低采样质量或 A/B 对照证明收益。
