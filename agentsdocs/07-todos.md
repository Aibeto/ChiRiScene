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

### EEM 相对能效模型（2026-10-22 重构，待真机验证）

- [ ] **真机验证 EEM 迁移收益**：`devimp` 抓 `@A act=place` 帧，用 `scripts/devimp/dvaff.py <dir>`
      看「线程迁移：方向分布 / 决策原因 / 收益分布」三张表。关注点：`clipped=true` 的比例
      （高 = cpuset 没升组成功，迁移没落地）、`cheaper_cluster`/`lighter_cores` 的 gain 分布、
      以及迁移后 CLG 是否真的降档（否则说明甜点封顶没生效）。
- [ ] **`sweet` 位置校准**：当前甜点完全由曲线形状决定（sm8750 big 1786MHz、prime 1958MHz）。
      真机上若发现甜点偏低导致单线程性能不足，先看 `eem_press_streak` 是否频繁触发抗饥饿释放
      （触发 = 封顶确实压住了），再考虑引入 `sweet_bias` 档偏移，**不要直接改判据**。
- [ ] **`single_core_power_w` 的 8750 数值存疑**：sm8750 的参考曲线来自第三方图表折算
      （big 顶档单核 3.819 W、prime 6.196 W），绝对量级未经真机核对。**相对向量使机内决策不受影响**
      （形状正确即可），但**跨簇比较依赖簇间相对比例**，故 prime/big 的比值若失真会改变跨簇落点。
      待有条件时用 PMIC 实测复核（`devimpbin/batch_ingest.py` 链路已就绪）。
- [x] **8650 适配 EEM 已完成（2026-10-23）**：`scripts/curve_anchor.py --soc sm8650` 生成受控块到
      `module/config/sm8650/soc.yaml`，`feature.yaml` 开 `eem_placement: true`。数据源 = 8 Gen 3 实测
      能效曲线（A520/A720/X4 + N4P），口径与甜点见 `06-kernel.md`「8650 的 EEM 已适配」条。
      结构回归 = `src/common.rs` 的 `frontier_config_tests::sm8650_frontier_usable`（断言甜点三档，
      改锚点数据会立刻失败）。
- [ ] **8650 甜点真机校准**：当前 little 1843200 / big 2323200 / prime 2246400 kHz，来自**读图**
      （逐点约读 + `α=0.7` 反解频率轴），误差可漂 ±2 档，**按体质误差接受**（用户 2026-10-23 定）。
      真机若发现单线程性能不足，同 8750 口径：先看 `eem_press_streak` 触发率，再考虑 `sweet_bias`，
      **不要直接改判据**。
- [ ] **8650 逐档电压仍缺**（唯一未闭合的口径）：DT `opp-microvolt` 计数 0，电压只在 EPSS 寄存器
      （`reg_volt_lut=0x200`、`LUT_VOLT=GENMASK(11,0)`、×1000）。取证后可反解真实 V(f) 替换本轮的
      曲线折算，并用 `curve_anchor.py --check` 对账。**这是「代际进步是否被正确表达」的唯一硬校验**——
      dpc 绝对值已被归一化抵消，形状只由 V(f) 曲率承载。
- [ ] **8475 维持默认回退 CLG（2026-10-23 用户决定，非待办）**：`module/config/sm8475/` 无
      `frontier_policy`、`feature.yaml` 无 `eem_placement` → `soc_frontier_policy()` 返回 None →
      `energy_model::curve()` 全 None → 日志前缀仍是 `CLG`（非 `CLG-EEM`）。根因：8475 无可信频表
      （DT 无静态 OPP、无真机 devimp 采集、mainline 无 cape）。若日后要适配，**先解决频表**，再按 8650 同一
      链路（实测能效曲线 → `curve_anchor.py`）补功耗。
- [ ] **`devimpbin/batch_ingest.py` 的负净idle 问题**（历史遗留，未修）：616/851 档
      `净idle < 0`，泄漏校正反号、功耗被放大（最差 exynos7880 little 顶档 +0.558 W = 23%）。
      修法 = `idle = max(0.0, idle - base)`，但需连带重跑 batch_ingest → 文档 §3 →
      `frontier_policy.py` → 37 个 soc.yaml。EEM 下影响限于簇内形状（相对量），优先级低于真机验证。
- [ ] **EEM 与既有 pin/promote 的边界**：当前 EEM 只管「未钉核且非关键」的线程，pin/promote/demote
      路径仍在跑。若真机出现两套机制打架（同一线程反复改掩码），考虑在 EEM 开启时把普通放置
      路径的 `pick_core` 也交给 EEM 统一决策。

### 待裁决（2026-10-24 A02-14 真机分析产出；证据与规则见 `devimpbin/a0214/REPORT-A02-14.md`）

- [ ] **「降低一半功耗」的口径未定（用户目标，保留待拍板，不得由 AI 否决）**：A02-14 五台真机实测
      （79565 放电样本 / 57.762 样本等效 Wh）显示 CPU 动态只占整机 15.8%~35.3%（sm8750 游戏机 64.6%
      例外），**整机 -50% 在调度侧不可达**（即使 CPU 动态清零也只有 -16%~-35%）。三个候选口径：
      ① 整机能耗 → 不可达；② **daemon 自耗减半 → 可达**（基线 52.82~134.78 ms CPU/s，
      zumapro 13.48% 单核最差，playback 段 144.79 ms/s 是 default 段 2.2 倍），需改 `src/` 采样链路；
      ③ 规则覆盖段的 CPU 动态减半 → 方向可达，整机折算仅 -0.3%~-3.5%。**请用户指定口径**。
- [ ] **`com.android.incallui` 息屏通话的归属**：PHB110 上 6.566 Wh（27.7%）居首，但 58.5% 的 snap 行
      `screen_on=0` → activity 采不到（末列 `-`），现有 `.InCallActivity` 的 `activity_params` 不生效。
      属 SceneMode（息屏轴）地盘。待确认：息屏判定与 `scene_mode_delay_secs=3000` 的关系，
      是否给该包加息屏侧覆盖。
- [ ] **`com.tencent.mobileqq/.activity.SplashActivity` 的 boost 是否回收**：该键实测覆盖整个 QQ 会话
      （40262 tick ≈ 长驻，非启动页），PHB110 上 5.372 Wh（22.7%）。但 boost 段 2.39 W（8283 样本）
      < default 段 2.95 W（1817 样本），样本量差 4.6 倍且负载相位不同，**无法判定回收是否省电**。
- [ ] **内核 sched 通道全线失效（2026-10-24 A02-14 实测，5 台机型全部）**：
      `/proc/sys/kernel/sched_migration_cost_ns` 与 `/proc/sys/kernel/sched_nr_migrate` **节点不存在**
      （9 个启动点各报「节点不存在 ×2」）→ `module/config/normal/fas/{sgame,dfm}.yaml` 的
      `migration_cost_ns: 400000` 与 `tuned.rs` 同名通道**从未生效**，yaml 里「实机复核 400000」的 TODO
      因此一直没有结论。已加告警：`fas_manager.rs` 改为逐分支 `log::warn!`（读不到 / 写失败分别报），
      旋钮失效不再静默。**修法仍需先取证**：真机 `ls /proc/sys/kernel/ | grep sched` + `/sys/kernel/debug/sched/`，
      按实际存在的节点改名或换通道；**不猜节点名**。这是 ChiRi 侧唯一能直接压内核搬迁次数的旋钮。
- [ ] **i18n 文案与行为不符（顺手可改）**：`governor-residue-cleanup` 写「已恢复 schedutil」，实际恢复目标是
      `common::cpu_governor_for_policy(pid)`（walt / uag / sched_pixel…）。只改文案，不动行为。
- [ ] **FAS 残留 performance 泄漏已闭环（证据已到，可销那条「待真机验证」）**：真机 9 条
      「检测到残留 performance 调速器」，启动自检已捕获并写回默认调速器。
- [ ] **EEM 同轮承诺账（已实现，待验证）**：`placement::Ledger` 让同轮后续决策看到已承诺的负载，
      取代原「`EEM_MAX_MOVES_PER_ROUND` 硬截断」的齐涌应付（详见
      `devimpbin/a0214/REPORT-A02-14.md` §8.3）。验证途径只有离线重放
      （`scripts/devimp/replay_clg.py` 同法）或下一包真机观察——按 R3 不做 A/B 对照。
- [ ] **FAS 迁移治理（未完）**：实测 fas 段 9703~10007 次/s（default 2728~4327），但 mig/wake 比
      0.45~0.58 与 default 0.46~0.50 同量 → 主因是唤醒量（17k~22k/s 对比 5.7k~8.7k/s）而非策略。
      ChiRi 自身 @A 动作仅 1.24 次/s（占 0.01%），只调自己的放置动不了总量。已落地三项（见
      `devimpbin/a0214/REPORT-A02-14.md` §8.3）：Ledger 承诺账、boost 期 promote 保持禁（分区
      不变式）、home/bg_overload 重钉改保守参数版（debounce ×3、阈值 0.85，用户定调「允许迁移
      但更少更稳」）。其余待上述 sched 通道取证后再定。
- [ ] **性能不足候选（若写即 boost）未落规则**：`org.mozilla.firefox/.App`、`me.weishu.kernelsu/.ui.webui.WebUIActivity`、
      `com.miui.home/.launcher.Launcher`、`com.eg.android.AlipayGphone/…NebulaActivity$Main`、
      `com.tencent.mm/.plugin.webview.ui.tools.MMWebViewUI`、`com.oplus.camera/.Camera`。
      未采纳理由：加宽松档无法回答 R4「省了哪部分功耗」，「race-to-idle 更快完成所以更省」在本批日志
      无完成时间证据（属模型外推）。

### 亲和写失败治理（2026-10-26 落地，待真机复核）

- [x] 已落地：失败退避（pin `REPIN_DEBOUNCE×2^(n-1)` 封顶 60s / place `FAIL_BACKOFF_BASE×2^(n-1)` 封顶 30s）、
      进程级窄域备忘（同 pid 同簇 15s，只对 EINVAL 记账）、`Engine::applied` 回滚锚点 +
      `note_applied`/`abort`、`@A place` 追加 `eff=` 回读掩码。
      取证与数字：`.cursor/docs/2026-10-26-亲和写失败取证与窄域修复.md`。
- [ ] **下轮日志核对三件事**：① `pin fg_pin e22` 是否从 6.4 万量级降到百量级；② `place` 失败是否
      按 (pid,簇) 收敛（预期 ≈30× 降幅，126,555 → ≈4,190 次尝试）；③ `eff=` 回读掩码与 `dst=` 的差集
      落在哪些核 —— 这是判定「cpuset 收窄」还是「核此刻不可用」的决定性数据。
- [ ] **cpuset 感知放置（根治项，待判别后动）**：写前读一次该线程的 cgroup cpuset 并缓存
      （`affinity_cpuset::check_group_cpuset` 已有实现，当前只服务组绑定、单核/放置路径刻意未用），
      把目标区间夹到允许域内。判据：下轮失败若集中在特定 pid 而非特定核号 → cpuset 侧。
- [ ] **活动核探测（可选）**：`sched_getaffinity(self)` 一轮一次拿「在线且活跃」核集（暂停核会被内核
      从掩码里摘掉），作放置域上界；风险 = 守护进程自身 cpuset 可能更窄，须先取证再启用。
- [ ] **EEM 预算观察**：本批日志「有 place 动作的秒里 **78%** 打满 `EEM_MAX_MOVES_PER_ROUND`=4」，
      预算已是真实限流器；放宽会增加迁移数与「更少更稳」冲突，等失败治理生效后按观测定。
- [ ] **本机离线回归缺口**：`scripts/tests/clg_replay.rs`（`CpuLoadGovernorConfig` 未定义桩）与
      `scripts/tests/rust_regression.rs`（`crate::i18n` 缺失桩）在 HEAD 即编译失败，需补桩或显式标注跳过。

