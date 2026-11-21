---
id: "fbd690bd-354d-4ad5-a9a7-1e49772b3ac6"
title: "ChiRi CLG Worker 停止与唤醒契约"
description: "记录 CLG worker stop、负载通道唤醒、恢复和 join 的稳定生命周期语义，以及尚待设备测量的退出时延。"
status: "active"
created_at: "2026-11-21T12:27:21.361Z"
updated_at: "2026-11-21T12:27:21.361Z"
content_hash: "da93d526fbcd1e1a58f2b13eac941708fca802fce69250a2361f937d23e61fbe"
source_paths:
  - "src/chiri/cpu_load_governor.rs"
  - "agentsdocs/03-chiri.md"
session_ids:
  - "session-547e8bd8-29af-40f5-9e93-9b6eb70405c7"
memory_body_ids:
  []
---

# ChiRi CLG Worker 停止与唤醒契约

CLG worker 通过有界负载通道等待事件，空闲时 `recv_timeout` 最长等待 1 秒。停止时，`CpuLoadGovernor::stop_workers` 先以 Release 发布停止标志，再对每个 worker 非阻塞发送一个共享空负载唤醒包，之后同步 join。若队列已满，现有消息也会唤醒接收；通道断开表示 worker 已退出。

`CoreGroupWorker::run` 在接收返回后以 Acquire 检查停止标志；若已停止，则不处理收到的负载/唤醒包、不执行该轮 flush，直接退出并在退出路径恢复原 policy。Join 因而覆盖 worker 退出及其恢复完成。此机制避免纯空闲 worker 让停止路径被 1 秒接收超时拖住，同时保持调度策略、阈值和采样不变。

主机回归只验证阻塞接收能被唤醒并让 join 快速结束；设备上的 stop 到全部 join/restore 完成的墙钟耗时，以及与交互卡顿的因果关系仍需同机时间戳和逐帧证据确认，不得宣称体验已解决。

来源：`src/chiri/cpu_load_governor.rs`、`agentsdocs/03-chiri.md`。
