## [todos] 未竟事项台账（2026-09-26 汇总）

> 历史计划/日志归档后的**唯一 TODO 权威**。开工前先查本文件；完成一项删一项。2026-09-17 审查提出的旧项标注「先核实」——动手前先确认现行代码是否已修。
>
> **口径变更（2026-09-28）**：R3「**永不再新增 A/B 测试**」生效（详见 `04-hard-lessons.md` 的 `[hard]` 首条）。以 A/B 为手段的条目一律作废，改「离线重放 + 同版本观察」。
>
> **2026-10-20 清空**：用户裁决「已完成的全删、其余待办全删」，被删条目若需追溯看 git 历史（`git log -p agentsdocs/07-todos.md`）。
>
> **2026-10-20 二次整理**：本文件只留「还没做 / 等拍板」的条目。已完成项、机制说明、硬件基线、环境限制、定案记录已移到对应子块——FAS 机制与定案见 `03-chiri.md`，禁令与教训见 `04-hard-lessons.md`，历史快照判读口径见 `05-maint.md`，内核 / DT 基线与 dtsi 对账见 `06-kernel.md`，构建与本地验证环境见 `01-overview.md`，`soc.yaml` 与前沿表的生成口径见 `02-convention.md`。

### 待办（性能与探针）

- **FAS governor 残留 performance 的修复待真机验证**：修法与机制见 `03-chiri.md` 的 FAS 小节「governor 残留 performance 与 reset() 的快照陷阱（2026-10-20 修）」。判据 = 下一包 `daemon.log` 是否再出现「检测到残留 performance 调速器」；本机无 Android NDK，android `cargo check` 未跑（见 `01-overview.md` 的本地验证限制）。
### 待办（配置 / 文档 / 杂项）

- **批量适配机型的未验证面**：34 个新机型 + `msm8998` 的 `soc.yaml` 与前沿表由 `devimpbin/{batch_ingest,gen_soc_config}.py` 从 `freqbench_tensor` 实测折算生成，**未做真机调优、参数继承根 `feature.yaml` 默认**；生成口径见 `02-convention.md` 的 `soc.yaml` 条。**待办**：真机确认各机型 `[topology]` 与 `cpu_capacity` 实测值是否与折算一致（尤其 legacy 高通 4+4 与 6+2 两类）、确认无机型被误接管（回退 = 从 `CHIRI_SOC_HINTS` 删掉对应片段：不接管 CPU 仅监控，与 `msm8998` 曾经的「暂时下线」同法）。
- **`sm8475` / `sm8650` 仍缺 `frontier_policy`（`sm8475` 另缺 `[freq_khz]`）**：两机无 freqbench 实测（`freqbench_tensor/results/` 无对应目录），前沿块需要逐档功耗，只频率表不够；`cape.dtsi` 只给 `qcom,freq-domain` 簇编号，频表走 EPSS 硬件 LUT，DT 里没有 `qcom,cpufreq-table`。补齐途径：① 真机 `scaling_available_frequencies` + `cpu_capacity` 采样（`sm8650` 已有 PJX110 little 379 / big 923 / prime 1024）；② 补测 freqbench。已补的硬件基线段（freq/idle/dpc/capacity 与 dtsi 对账）见 `06-kernel.md`「平台基线（sm8475 / msm8998，dtsi 口径，2026-10-20）」。
- **`msm8998` 待真机验证**：capacity 兜底 490/1024 与真机 `cpu_capacity` 的偏差（DT 声明 1024/1536）、PF 落点是否只降不升。基线事实见 `06-kernel.md`「平台基线（sm8475 / msm8998，dtsi 口径，2026-10-20）」节。
- **已知待清理警告（2026-10-01 用户裁决保留，勿顺手改）**：① `xtask/Cargo.toml:8` 的 `chrono` 依赖声明未使用（全仓只在 `xtask/src/main.rs:57` 注释出现，删除会连带改 `Cargo.lock`）；② `src/monitor/telemetry.rs:133/138` 的 `batt_current_ma` / `batt_voltage_v` 已被 `batt_sample` 取代，现为 `dead_code`。两条均不影响功能、不做 A/B；待用户明确批准清理后再删，回退为恢复依赖/函数原声明。
- **文档修订（用户定暂缓，顺手勿改）**：① `README.md:30` 链 `/mdocs/updateWithoutRestart.md` 不存在（`updateWith.md` 已移 `.archive/mdocs/`，恢复链接或删除），同处 FAS 措辞待定；② `src/chiri/mod.rs:173`「fas_manager 提供多实例生命周期管理」与实际单实例（`instance: Option<FasInstance>`）不符；③ `updateInformation/changelog.md` 与 `HotUpdateInfo.md` 仍是空文件、发版时是否填充未定（`update.json` 契约见 01 的目录树）；④ 历史记忆件里的「量产机型只有 8550/8475/8998」「`CHIRI_SOC_HINTS` 只有两条」类表述随 39 条 hints 全面过期（`.codebuddy/` `.trae/` `.cursor/` 的 MEMORY.md 属他环境只读区，只做对账不改写）。
