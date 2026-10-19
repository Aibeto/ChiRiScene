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

- **`sm8475` / `sm8650` 仍缺 `[freq_khz]` 段与帕累托前沿块（2026-10-20 批量适配后仅剩的缺口）**：这两个机型**没有 freqbench 实测结果**（`freqbench_tensor/results/` 下无对应目录），本地也无 dtsi，故既算不出逐档能效表、也落不了 `frontier_policy` 与 `[capacity]`。`sm8650` 的 `soc.yaml` 现只有真机 sysfs 测得的部分 capacity 与 `[topology]`，`sm8475` 连 capacity 都没有。**补齐途径按优先级**：① 真机 `scaling_available_frequencies` + `cpu_capacity` 采样（照 `agentsdocs/06-kernel.md` 的「真机 capacity」口径，`sm8650` 已有一半数据：PJX110 的 little 379 / big 923 / prime 1024）；② 补一份这两机的 freqbench 测量（能效模型需要逐档功耗，仅频率表不够落前沿块）；③ 找厂商板级 dtsi 与功耗表。**注意**：`sm8475` 与 `sm8550` 同属 SM8550 平台族的改款（CPU 簇 0-3/4-6/7 同构），若能拿到 `sm8475` 的实测档位，其簇结构与 `sm8550` 一致、可直接套用口径。回退 = 保持现状（无该段时 CLG 走原比例路径，行为与加入前相同）。
- **批量适配机型的未验证面（2026-10-20）**：34 个新机型的 `soc.yaml` 与前沿表由 `devimpbin/{batch_ingest,gen_soc_config}.py` 从 `freqbench_tensor` 实测折算生成，**未做真机调优、参数继承根 `feature.yaml` 默认**；`CHIRI_SOC_HINTS` 已按 B 方案扩到 39 条，即这些机型真机命中就会接管调度。**待办**：真机确认各机型 `[topology]` 与 `cpu_capacity` 实测值是否与折算一致（尤其 legacy 高通 4+4 与 6+2 两类）、确认无机型被误接管（回退 = 从 `CHIRI_SOC_HINTS` 删掉对应片段，不接管 CPU 仅监控，与 `msm8998` 曾经的「暂时下线」同法）。
- **`msm8998` 已恢复（2026-10-20）**：原从 `CHIRI_SOC_HINTS` 移除（「暂时下线」），本轮用 freqbench 实测折算出 capacity（490/1024）、`[freq_khz]`（little 22 档 / big 31 档）与前沿表（20 前沿点、20 桶、指纹 `aa8c6ffe50a3216a`），已加回名单并开启默认启用。**待真机验证**：该机型 capacity 兜底值（490）与真机 `cpu_capacity` 的偏差、PF 落点是否只降不升。
- **`cargo check --workspace` 在本机预失败（与本次改动无关）**：`build.rs:157` panic `yumi-ebpf 编译失败`，stderr 为 `error: the -Z flag is only accepted on the nightly channel of Cargo`。需 nightly 或 `YUMI_SKIP_EBPF=1`；本机无 Android NDK，`--target aarch64-linux-android` 也跑不了（`zstd-sys`/`ring` 的 vendored C 缺 `aarch64-linux-android-clang`）。故本批的 Rust 侧只做了 `cargo fmt --all --check` 与三套 `rustc --test` 回归。

- **历史快照里的 PowerBase 失效引用（2026-10-20 订正，按 AGENTS.md「文件名含日期的 md 不回改」不回改原件）**：`src/chiri/power_base.rs` 已随 `dcae0c4` 删除，两处历史件仍指向它——① `.cursor/plans/2026-10-07-daemon-selfcost-implementation-plan.md:159` 的 C4「PowerBase 电池状态 TTL」条目**整体作废**（不再实施）；② `.mnemon/documents/active/chiri-clg-high-zone-2026-10-13-2fb841f8.md:38` 引用 `power_base.rs:478`。判读这两份件时以本条为准；`devimp-log-analysis.md` 的同款判读指令已就地改正（不在日期文件名内，属活文档）。

- **已知待清理警告（2026-10-01 用户裁决保留，勿顺手改）**：① `xtask/Cargo.toml:8` 的 `chrono` 依赖声明未使用（全仓只在 `xtask/src/main.rs:57` 注释出现，删除会连带改 `Cargo.lock`）；② `src/monitor/telemetry.rs:133/138` 的 `batt_current_ma` / `batt_voltage_v` 已被 `batt_sample` 取代，现为 `dead_code`。两条均不影响功能、不做 A/B；待用户明确批准清理后再删，回退为恢复依赖/函数原声明。

- 文档修订（用户定暂缓，顺手勿改）：README FAS 措辞与断链（README:28 链 `updateWithoutRestart.md` 不存在；`updateWith.md` 已移 `.archive/mdocs/`，恢复链接或删除）；mdocs/socList.md 滞后（只列 8550，实际 8550/8475/8998 三份）；`src/chiri/mod.rs:184`「多实例」注释过时（实际单实例）；affinity.rs 头注释 0.2 vs `PINNED_WEIGHT` 0.4；soc.yaml capacity 注释「厂商板级 DTS 覆盖」建议改弱为「来源待证」。（原列的「feature.yaml『整簇断电』注释 vs 实际只下线 prime」已随 2026-09-28 的离核策略改动一并消解——注释与行为现在都是「下线 little」。）
