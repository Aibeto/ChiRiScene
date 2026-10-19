## [todos] 未竟事项台账（2026-09-26 汇总）

> 历史计划/日志归档后的**唯一 TODO 权威**。开工前先查本文件；完成一项删一项。2026-09-17 审查提出的旧项标注「先核实」——动手前先确认现行代码是否已修。
>
> **口径变更（2026-09-28）**：R3「**永不再新增 A/B 测试**」生效（详见 `04-hard-lessons.md` 的 `[hard]` 首条）。下列以 A/B 为手段的条目**不再按 A/B 执行**，改为「离线重放 + 同版本观察」：[C3] 构建剖面、真机 A/B 整体、T7 uclamp.max=85 / T8 clamp_heavy 恒钳、telemetry gpu_busy 采集成本——逐条待裁决去留，其中涉及"采集开关对照"的直接作废。
>
> **2026-10-20 清空**：用户裁决「已完成的全删、其余待办全删」，台账仅保留禁令类条目（本文件现有全部内容）。被删条目若需追溯看 git 历史（`git log -p agentsdocs/07-todos.md`）；真机验证类待办改由 `03-chiri.md` / `06-kernel.md` 的机制描述与日常 devimp 判读承载。

### 性能与探针（perf-report backlog）

- **【已修 2026-10-20】FAS 异常退出后 governor 残留 performance**：`src/scheduler/fas/policy_controller.rs` 的 `reset()` 原先无条件按 `orig_governor` 快照写回，而该快照若取自 GovernorGuard 已切 `performance` 之后就会把 performance 写回去（23:14:40 新一轮启动仍报 4 条「检测到残留 performance」即此路径）。现改为：快照等于 `performance` 时改用 `common::cpu_governor_for_policy(policy_id)` 回落到该 policy 的默认选型，并打新 i18n key `fas-governor-reset-performance`（中英已对称）。**未真机验证**：本机无 NDK，android `cargo check` 未跑；判据 = 下一包 daemon.log 是否再出现「检测到残留 performance 调速器」。

- **[E1] 内容比对短路已定案不做（2026-10-01）**：`app_detect.rs` 显式注明「刻意不做内容未变则复用」——冷启动期间 pid 先入组、exec 后 cmdline 才可读，缓存会漏检这类前台切换。

- **已定案不做（勿重评）**：U3 cpu_monitor 侧、A5b、panic=abort、换 hasher/parking_lot、热路径值缓存、事件化否决四项（cgroup.events poll / PSI / thermal uevent / timerfd 全量合并）、换语言与双进程拆分。

### 配置 / 文档 / 杂项

- **历史快照里的 PowerBase 失效引用（2026-10-20 订正，按 AGENTS.md「文件名含日期的 md 不回改」不回改原件）**：`src/chiri/power_base.rs` 已随 `dcae0c4` 删除，两处历史件仍指向它——① `.cursor/plans/2026-10-07-daemon-selfcost-implementation-plan.md:159` 的 C4「PowerBase 电池状态 TTL」条目**整体作废**（不再实施）；② `.mnemon/documents/active/chiri-clg-high-zone-2026-10-13-2fb841f8.md:38` 引用 `power_base.rs:478`。判读这两份件时以本条为准；`devimp-log-analysis.md` 的同款判读指令已就地改正（不在日期文件名内，属活文档）。

- **已知待清理警告（2026-10-01 用户裁决保留，勿顺手改）**：① `xtask/Cargo.toml:8` 的 `chrono` 依赖声明未使用（全仓只在 `xtask/src/main.rs:57` 注释出现，删除会连带改 `Cargo.lock`）；② `src/monitor/telemetry.rs:133/138` 的 `batt_current_ma` / `batt_voltage_v` 已被 `batt_sample` 取代，现为 `dead_code`。两条均不影响功能、不做 A/B；待用户明确批准清理后再删，回退为恢复依赖/函数原声明。

- 文档修订（用户定暂缓，顺手勿改）：README FAS 措辞与断链（README:28 链 `updateWithoutRestart.md` 不存在；`updateWith.md` 已移 `.archive/mdocs/`，恢复链接或删除）；mdocs/socList.md 滞后（只列 8550，实际 8550/8475/8998 三份）；`src/chiri/mod.rs:184`「多实例」注释过时（实际单实例）；affinity.rs 头注释 0.2 vs `PINNED_WEIGHT` 0.4；soc.yaml capacity 注释「厂商板级 DTS 覆盖」建议改弱为「来源待证」。（原列的「feature.yaml『整簇断电』注释 vs 实际只下线 prime」已随 2026-09-28 的离核策略改动一并消解——注释与行为现在都是「下线 little」。）
