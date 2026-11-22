---
id: "d89de73c-a78b-4334-b279-d501bb709545"
title: "ChiRi 决策时延取证埋点 latency/1（2026-11-22）"
description: "决策时延/抖动取证：schema=latency/1 五轨道埋点、读法与坑、下版本四项待改与判据；起因是 chiri 自身仅占单核约 9% 故省电论据弱"
status: "active"
created_at: "2026-11-22T09:36:04.991Z"
updated_at: "2026-11-22T09:36:04.991Z"
content_hash: "862359ed7a0afb23ddb374373295827b77c3e9fbe24cac8fdad50c3524271d38"
source_paths:
  - "src/chiri/diag_latency.rs"
  - "src/chiri/mod.rs"
  - "src/monitor/cpu_monitor.rs"
  - "src/monitor/window_visibility.rs"
  - "agentsdocs/02-convention.md"
  - "agentsdocs/07-todos.md"
  - ".dsh/docs/2026-11-22-决策时延取证埋点.md"
session_ids:
  - "session-51131e41-7158-4821-a2c5-1a95fd2678be"
memory_body_ids:
  []
---

# ChiRi 决策时延取证埋点 latency/1（2026-11-22）

## 口径来源
用户 2026-11-22 定：目标是**降低决策时延与抖动**，不是省电；因即将发版，本版只加零行为影响的取证埋点，语义改动留到下个版本，等新包真机日志定夺。不做同版本 A/B，跨版本同场景对比即可（同 SoC + 同 kernel + 同 `exe` 指纹）。

## 现状证据（devimpbin/logd_1122-160240.tar，9 轮、549 s 墙钟 / 49.2 CPU 秒）
- chiri 自身只占单核约 **9%** ⇒ 省电论据弱，论据是延迟/抖动。
- 逐线程 `/proc/<pid>/task/<tid>/stat` 扫描占自身 CPU **10.8%**（`do_task_stat` 428 + `thread_group_cputime` 324）。
- 唤醒/调度占 **25.3%**（`finish_task_switch` 767 + `__schedule` 532，另有 `cgroup_freezing` 229）。
- `dumpsys` 子进程样本 **6.4%**（不计入 chiri 自身）。
- 线程占比：`scheduler_ipc` 22.4%、`cpu_monitor_ebp` 15.9%、`aff_sample_0..3` 合计 21.8%、`fps_probe` 5.1%。

## 实现
新增 `src/chiri/diag_latency.rs`（`schema=latency/1`，只读时间戳 + 原子计数，不改判定分支）。写入与 selfcost **同一个** `devimp/selfcost_<ts>.log`，按行首 `schema=` 分流；60 s 一窗，只在诊断会话内累计与落盘，尾窗在关闭/停机时冲刷。

轨道：`wake`（定时唤醒迟到量 = 实际等待 − 请求等待，只记超时唤醒，请求等待 <1 ms 跳过）、`work`（收事件到下一轮循环开头）、`scan_fg`（`fg_thread_max_util` 逐线程读 stat）、`scan_procs`（`snapshot_procs`）、`probe`（`collect_one` 单次 dumpsys，含 fork/exec 与回收）。

行格式：`schema=latency/1 session=s<epoch> window_start= window_end= track= count= sum_us= max_us= p50_us= p95_us= p99_us= ovf=`；`wake`/`work` 追加 `slow_us=10000 slow= slow_scan= slow_probe= slow_both= slow_neither=`——慢事件发生瞬间读扫描/查询 in-flight，四类互斥、合计 = `slow`。

读法：分位取直方图上界（50 µs~200 ms 12 档 + 溢出桶），偏保守；`ovf` = >200 ms 样本数；空轨道不落行（缺行 = 无样本，不是 0）；并发自增使窗口切换瞬间 ±1 条可能落进相邻窗口。崩溃恢复路径清空 `last_recv_at`，避免 1 s 退避被记成超长 `work`。

## 下版本四项（agentsdocs/07-todos.md「调度时延 / 抖动」）
1. `/proc` 逐线程扫描 → eBPF 线程级 util（`compute_thread_level_util` 已走 BPF）；判据 `scan_fg` 与 `slow_scan` 同步下降且 FAS 选线程不变。
2. `dumpsys` 探测合并（同 tick 复用一份输出）；前置 = activity/window 判定逐条不变。
3. 定时唤醒合并（单线程截止时间队列）；前置 = 任何一路采样时点不后移。
4. PGO 采样加 `--no-inherit`（去掉继承的子进程样本，纯热度）；可与 2 一起定。

约束：决策签名 / 采样频率 / 字段数不减；禁并行/交错 A/B；验收只认跨版本同场景 `latency/1` 分位 + `slow_*` 占比，不能用 main tick 行间隔代替。

## 验证
`cargo fmt --all --check` 通过、`cargo check -p chiri --offline` 零错误（既有 13 条 warning）、4 个单测通过。真机未验（埋点尚未上机）。
