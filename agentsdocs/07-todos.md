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

- **`sm8475`（cape）/ `sm8650` 的 dtsi 已取到，硬件基线段补齐（2026-10-20 收口）**：原条目「本地无 dtsi、缺 `[freq_khz]`」已不成立，订正如下。
  - **dtsi 来源**：`sm8650` 用仓库内 `AYNTechnologies/linux/arch/arm64/boot/dts/qcom/sm8650.dtsi`（**含完整静态 CPU OPP 表**，与原注释「EPSS 无静态表可抄」相反，注释已就地改正）；`msm8998` 用 `android_kernel_oneplus_sm8550/.../msm8998.dtsi`（同件亦在 sm8650 仓）；`sm8475` 不在 mainline（`git.kernel.org` 上 `sm8475.dtsi`/`cape.dtsi` 均 404，torvalds/linux-next/stable 三处一致），已从 `OnePlusOSS/android_kernel_modules_and_devicetree_oneplus_sm8475`（分支 `oneplus/sm8475_s_12.1_oneplus_10t_5g`）取 `kernel_platform/qcom/proprietary/devicetree/qcom/cape.dtsi`，落在 gitignored 的 `devimpbin/web/cape.dtsi`。**平台代号：cape = SM8475，waipio = SM8450，kalama = SM8550**。
  - **已写入**：`sm8650` 补 `[freq_khz]`（little 25 档 307200-2265600 / big 47 档 460800-3148800 / prime 49 档 480000-3302400）+ `[idle_us]`（exit 750 / min_res 6700 / 簇态 2350）+ `[dpc]` {100, 238, 588}；`sm8475` 补 `[capacity]` {1024, 2253, 2386} + `[idle_us]`（exit little 750 / big·prime 1550；簇态 [2500, 3500]）+ `[dpc]` {100, 257, 509}；`msm8998` 补 `[idle_us]`（取深态 sleep_1：exit little 4562 / big 2027）。
  - **仍未落 `frontier_policy`**：这两机目标机型**无 freqbench 实测**（`freqbench_tensor/results/` 无 `sm8475`/`sm8650` 目录），前沿块需要逐档功耗，频率表不够。**`sm8475` 的 `[freq_khz]` 也仍缺**——`cape.dtsi` 只有 `qcom,freq-domain = <&cpufreq_hw N 4>` 给簇编号，频表走 EPSS 硬件 LUT，DT 里没有 `qcom,cpufreq-table`（与 sm8650 厂商 dtsi 不同）。补齐途径：① 真机 `scaling_available_frequencies` + `cpu_capacity` 采样（`sm8650` 已有 PJX110 真机 little 379 / big 923 / prime 1024）；② 补测 freqbench。
  - **口径注意**：`sm8475` capacity 取自 DT（1024/2253/2386），与 8550 的教训一致——厂商板级 DT 常与真机 `cpu_capacity` 不符（8550 DT 326/693/1024 vs 真机 280/855/1024），故仅作 sysfs 读不到时的兜底。`msm8998` DT 声明 1024/1536，与实测折算 490/1024 比值不同（1:1.5 vs 1:2.09），但均为相对值、消费方只排序，不冲突。回退 = 删对应段，缺段即走真机 sysfs。
- **批量适配机型的未验证面（2026-10-20）**：34 个新机型的 `soc.yaml` 与前沿表由 `devimpbin/{batch_ingest,gen_soc_config}.py` 从 `freqbench_tensor` 实测折算生成，**未做真机调优、参数继承根 `feature.yaml` 默认**；`CHIRI_SOC_HINTS` 已按 B 方案扩到 39 条，即这些机型真机命中就会接管调度。**待办**：真机确认各机型 `[topology]` 与 `cpu_capacity` 实测值是否与折算一致（尤其 legacy 高通 4+4 与 6+2 两类）、确认无机型被误接管（回退 = 从 `CHIRI_SOC_HINTS` 删掉对应片段，不接管 CPU 仅监控，与 `msm8998` 曾经的「暂时下线」同法）。
- **`msm8998` 已恢复（2026-10-20）**：原从 `CHIRI_SOC_HINTS` 移除（「暂时下线」），本轮用 freqbench 实测折算出 capacity（490/1024）、`[freq_khz]`（little 22 档 / big 31 档）与前沿表（20 前沿点、20 桶、指纹 `aa8c6ffe50a3216a`），已加回名单并开启默认启用。**待真机验证**：该机型 capacity 兜底值（490）与真机 `cpu_capacity` 的偏差（DT 声明 1024/1536）、PF 落点是否只降不升。**同轮补 `[idle_us]`**：本地厂商 dtsi `android_kernel_oneplus_sm8550/.../msm8998.dtsi` 存在（原「无本地 dtsi」不成立），取深态 sleep_1 → exit little 4562 / big 2027；该 dtsi 未声明 `dynamic-power-coefficient`（4.4 内核时代不用 EAS dpc 口径），`[dpc]` 保持缺省。
- **`cargo check --workspace` 在本机预失败（与本次改动无关）**：`build.rs:157` panic `yumi-ebpf 编译失败`，stderr 为 `error: the -Z flag is only accepted on the nightly channel of Cargo`。需 nightly 或 `YUMI_SKIP_EBPF=1`；本机无 Android NDK，`--target aarch64-linux-android` 也跑不了（`zstd-sys`/`ring` 的 vendored C 缺 `aarch64-linux-android-clang`）。故本批的 Rust 侧只做了 `cargo fmt --all --check` 与三套 `rustc --test` 回归。

- **历史快照里的 PowerBase 失效引用（2026-10-20 订正，按 AGENTS.md「文件名含日期的 md 不回改」不回改原件）**：`src/chiri/power_base.rs` 已随 `dcae0c4` 删除，两处历史件仍指向它——① `.cursor/plans/2026-10-07-daemon-selfcost-implementation-plan.md:159` 的 C4「PowerBase 电池状态 TTL」条目**整体作废**（不再实施）；② `.mnemon/documents/active/chiri-clg-high-zone-2026-10-13-2fb841f8.md:38` 引用 `power_base.rs:478`。判读这两份件时以本条为准；`devimp-log-analysis.md` 的同款判读指令已就地改正（不在日期文件名内，属活文档）。

- **已知待清理警告（2026-10-01 用户裁决保留，勿顺手改）**：① `xtask/Cargo.toml:8` 的 `chrono` 依赖声明未使用（全仓只在 `xtask/src/main.rs:57` 注释出现，删除会连带改 `Cargo.lock`）；② `src/monitor/telemetry.rs:133/138` 的 `batt_current_ma` / `batt_voltage_v` 已被 `batt_sample` 取代，现为 `dead_code`。两条均不影响功能、不做 A/B；待用户明确批准清理后再删，回退为恢复依赖/函数原声明。

- 文档修订（用户定暂缓，顺手勿改）：README FAS 措辞与断链（README:28 链 `updateWithoutRestart.md` 不存在；`updateWith.md` 已移 `.archive/mdocs/`，恢复链接或删除）；mdocs/socList.md 滞后（只列 8550，实际 8550/8475/8998 三份）；`src/chiri/mod.rs:184`「多实例」注释过时（实际单实例）；affinity.rs 头注释 0.2 vs `PINNED_WEIGHT` 0.4；soc.yaml capacity 注释「厂商板级 DTS 覆盖」建议改弱为「来源待证」。（原列的「feature.yaml『整簇断电』注释 vs 实际只下线 prime」已随 2026-09-28 的离核策略改动一并消解——注释与行为现在都是「下线 little」。）
