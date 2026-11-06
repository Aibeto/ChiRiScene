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
- [x] **首轮复核（2026-10-30，29 批 / 52.8M 行）**：① `pin fg_pin e22` **63,856 → 60**（1,064×，超额）；
      ② `place` **62,531 → 6,353**（9.8×，合计 19.6×，未达 30×，缺口几乎全在 place）；
      ③ `eff`/`dst` 差集 **94.0% 落在核 0-2**，但 `t` 行统计小核各承担 11-12% 驻留、285 个含小核的 pid 里
      93 个确实拿到过 ⇒ **不是 cpuset 收窄、也不是核离线，是「间歇可用」** —— 故 cpuset 感知放置
      （下条）暂不立项，改做「静默裁剪记账」（见下）。簇级震荡 0、@A 峰值 656/s < 基线 734。
- [ ] **cpuset 感知放置（根治项，待判别后动）**：写前读一次该线程的 cgroup cpuset 并缓存
      （`affinity_cpuset::check_group_cpuset` 已有实现，当前只服务组绑定、单核/放置路径刻意未用），
      把目标区间夹到允许域内。判据：下轮失败若集中在特定 pid 而非特定核号 → cpuset 侧。
- [ ] **活动核探测（可选）**：`sched_getaffinity(self)` 一轮一次拿「在线且活跃」核集（暂停核会被内核
      从掩码里摘掉），作放置域上界；风险 = 守护进程自身 cpuset 可能更窄，须先取证再启用。
- [ ] **EEM 预算观察**：本批日志「有 place 动作的秒里 **78%** 打满 `EEM_MAX_MOVES_PER_ROUND`=4」，
      预算已是真实限流器；放宽会增加迁移数与「更少更稳」冲突，等失败治理生效后按观测定。
- [ ] **本机离线回归缺口**：`scripts/tests/clg_replay.rs`（`CpuLoadGovernorConfig` 未定义桩）与
      `scripts/tests/rust_regression.rs`（`crate::i18n` 缺失桩）在 HEAD 即编译失败，需补桩或显式标注跳过。

### EEM 模型审计与调频/放置修复（2026-10-30 落地，待真机复核）

报告：`.cursor/docs/2026-10-30-EEM能效模型审计与调度改进.md`；审计明细 `devimpbin/_x1030/model-audit.md`。

- [x] **逐核供给下限** `energy_model::demand_with_core_floor`（+ `[per_core_floor]` 调用点）：不均载荷下
      `demand = max(Σ, busiest×k)`；均匀载荷逐位不变（两条回归锁死）。
- [x] **抗饥饿改挂起** `eem_cap_suspended`：原「10 封 1 放」占空比 91% ≈ 长期欠供；并订正「10 tick = 1.6s 不是 10s」。
- [x] **`[k_symmetric]`+`[supply_gate]`**：`best_placement` 增 `cores` 参数（单线程传 1、簇级传 k），
      `cheaper_elsewhere` 传本簇 k 并要求候选 `supply ≥ demand`、在线核数 ≥ cores（修 k 核 vs 1 核的口径错配）。
- [x] **换前台重置负载画像** `[fg_reset]`（`refresh_fg_change`/`reset_load_state`，不动 current_perf）。
- [x] **静默裁剪记账** `note_stripped`：写返回 0 但回读整簇被摘 → 与 EINVAL 共用 15s 窄域备忘（只记整簇）。
- [ ] **下轮核对**：① 同 pid 同簇「写 ok 但整簇被摘」是否不再重复出现；② `settled=true` 占比是否从 19.0% 上升；
      ③ 不均载荷场景（`max_util≈0.85` 且本簇其余核有小负载）下 `cur_freq/sweet` 是否抬升；④ fg 切换后首帧
      `up_wait` 是否不再继承旧方向（`deb_up` 首行应为 1）。
- [ ] **跨簇可比性校准**：8750/8650 的簇级瓦数是单核线性外推（顶档求和 35.31 W vs 实测整机 7.03 W，5×），
      且同一表按单核/整簇口径结论相反（big 便宜 38% vs prime 便宜 85%）。需实测校准或明确标注「不可跨代比较」。
- [ ] **开应用爬坡 15.3 J/次（占屏亮能量 4.0%）**：换模式时 CLG 重建 + `write_value_force` 写 init 频 →
      低 util 桶过频 +7…29%。是否改为「保留 current_perf、只重放上限」需先评估与热切换契约的冲突。
- [ ] **净 idle 符号错误**（`n×active−(n−1)×idle` 的 idle 被重复扣基线）：29/39 SoC 受影响、exynos7880
      little 顶档 +1.700 W（69%），但**三台在役机不走该路径**，改它会移动 11 个簇的甜点 → 等对应机型日志。
- [ ] **`dp ≤ 0 → eff = INF`**（22 簇）会把甜点抬到「功率持平」档，应改为「该档不算收益」。
- [ ] **Ledger 幻影承诺**（commit 在落盘前、abort 不回滚，`placement.rs:513-516`/`:429-437`）：仅影响一轮内
      后续线程排序且方向保守，暂留；要改须先补「同轮撤销」回归。
- [ ] **腾讯视频**：本轮判定「不加特调」（样本 81.1 s、详情页非播放页、playback 会丢 touch boost）。
      待有 ≥10 min 真实播放会话日志再评估。

### 日志 ERROR 取证与负载源兜底（2026-10-31 落地，待真机复核）

报告：`.cursor/docs/2026-10-31-日志ERROR取证与负载源兜底.md`。

- [x] **procfs 负载源兜底** `[load_source]`：eBPF `RING_BUF` 需内核 ≥5.8，sm8250/4.19 上 `Ebpf::load` 必失败，
      旧实现只打一条 `monitor-cpu-crashed` 就结束线程（daemon 活着但永不发 SystemLoadUpdate，CLG/tuned
      看门狗 68 分钟 release 26 次）。现 Err 分支起 `start_proc_stat_fallback`：逐核 /proc/stat 差分、
      节拍与主路径一致、不发假 BpfStats、连续失败 >50 拍交回看门狗。
- [x] **FAS 开环通知门控** `[open_loop_notify]`：实测 417 次开环/178 次恢复/207 条通知，中位 1 s、p90 7 s，
      全是退游瞬态。改为持续 10 s 才通知一次；PID 替换与去激活清计时。
- [ ] **sm8250 真机复核**：确认 `monitor-cpu-ebpf-unavailable` 出现、CLG 看门狗不再 release、status.csv CPU 列有值。
- [ ] **sched 迁移参数缺失降级**：`sched_migration_cost_ns`/`sched_nr_migrate` 在零售 GKI 不存在
      （CONFIG_SCHED_DEBUG=n，节点只在 debugfs），每次 boot 刷 38 条 WARN。应探测一次、缺失即静默并标不可用。
- [ ] **3 条 FAS 单测需桩**：`activate_*`/`repeated_activate_*`/`same_package_pid_*` 依赖编译期白名单里的
      `game`（实际不存在）+ 真机 sysfs，宿主机恒红；应注入白名单/policy loader 或 `#[ignore]`。
- [ ] **`[selfcost] @S` 22,397 条**（devimp 开时，50ms 阈值）：下一步给 @S 分块计时，定位是 `/proc/<pid>/task/*`
      扫描还是 uclamp/掩码回读，再决定优化点。

### 清理与回退（2026-10-31 三条改动后遗留）

- [ ] **删除 core_ctl 的 scenemode 下线机器**：2026-10-31 起 SceneMode 与 reduce 同口径（`apply_affinity_and_corectl`
      不再传 `scenemode=true`，`CoreCtlConfig.scenemode_offline` 标废弃并加 `#[allow(dead_code)]`），
      `src/chiri/core_ctl.rs` 的 `[scenemode]` 段（`scenemode_targets`/`scenemode_keep_cpus`/`max_cpus_applied`/
      `max_cpus_warned`/`restore_online`/`reserved_core`/`self_pinned_tids` 等约 400 行）已成不可达代码。
      下个版本整段删除，同步删 `03-chiri.md` 的 core_ctl 小节留痕与六份 feature.yaml 的 `scenemode_offline`。
      回退路径：恢复 `apply_affinity_and_corectl` 的 scenemode 专用分支（`set_power_state(false, enabled && scenemode_offline)`）。
- [ ] **`max_freq_pin` 真机复核**：看门狗 stale-release 后确认 `[release] max-freq pin` 落盘、各 policy
      `scaling_min_freq=scaling_max_freq=cpuinfo_max_freq`；再确认下一次接管（CLG init/reload、FastLock、Tuned）
      打出 `[release] max-freq unpin` 且频点回到系统快照（防止永久钉最高频）。
- [ ] **`screen_on_value` 真机复核**：`debug.tracing.screen_state=4`（DOZE_SUSPEND）时确认判息屏、
      `=1` 时判亮屏、属性缺失仍按亮屏兜底；顺带核对 `customize.sh` 安装期改写只在实测 1 时发生。

### vWdog 撞线取证与息屏静默档（2026-10-31 落地，待真机复核）

取证包：`devimpbin/SYSTEM_LAST_KMSG@c6cca54b…@16_40_59`、`…@cf831898…@23_40_47`、`…@f6f7b34f…@2026_11_01_08_53_37`；报告 `devimpbin/1031-kmsg/FINDINGS.md`、`devimpbin/1031-2340/FINDINGS_2340.md`、`devimpbin/1101-0853/FINDINGS_1101.md`。

- [x] **两份 minidump 同型定位**：`QCOM Apps Watchdog bark` → `qcom_wdt_bark_handler` ← GIC ← `cpuidle_enter_s2idle`；
      vWdog 206 tick ÷ 6.365202 s = 30.899 ms/tick（32.36 Hz，32.768 kHz/1024）⇒ 客侧 bark 20000 ms 未生效，
      **实际窗口 ≈200 tick ≈6.18 s**；挂起路径 6.225553 s + s2idle 空转 139.612 ms 即撞线。两份是不同构建
      （`phantom-oki cc067c49…` / `abogki537459655-4k`）⇒ 不是单版本回归。
- [x] **第三份 dump 同型确认（2026-11-01 08:53:37）+ 6.226 s 拆分订正**：第三次崩溃仍是
      `bark Now=4597.287886 / last pet 4590.939412`（Δ **6.348474 s**）、`vWdog-time since last pet: 206`
      （30.82 ms/tick ⇒ 32.45 Hz）、栈 `cpuidle_enter_s2idle+0x98/0xb0 ← do_idle ← cpu_startup_entry`
      逐帧相同，内核与 10-31 23:40 同构建 `cc067c49…` ⇒ **三次崩溃、两个构建、同一失败链**。
      **订正**：上文与 `FINDINGS_2340.md` §2.4 把 `last pet → s2idle 首条打印` 的 6.225553 s 判成
      "挂起路径耗时"不成立——printk ring 文本区仅 ~2.0 MB 而该 debug 打印 ~129 B/轮，16231–16236 条正好填满，
      **ring 里永远只留最后 ~106–140 ms**（10-31 实测跨度 139.612 ms = 16236×8.5 µs、本次 105.871 ms =
      16231×6.458 µs，均与 ring 深度吻合）。现状只可断言"pet 后 ≥6.24 s 无 printk 留存"，其中 CPU 做什么
      只能强推断（≈98 万轮 s2idle 空转 × 24 IRQ/轮 ≈ `Top10 irqs Total 23624040/6.348 s`）⇒ 见下方
      "6.24 s 不可观测窗口"条目。
- [x] **息屏静默档（L1）** `[screen_off_quiet]`：`TELEMETRY_LOG_INTERVAL_SCREEN_OFF=5s` /
      `MODE_FILE_REWRITE_INTERVAL_SCREEN_OFF=30s` / `EVENT_POLL_MS_SCREEN_OFF=2s`（thermal 2s 不动）；
      推送事件即时打断，`CLG_STALE_MAX=5s` 断投判定不受影响。
- [x] **亮屏时延打点**：`screen_detect::mark_detect()/detect_latency_ms()` + screen 事件分支
      `scheduler-screen-latency` INFO（判定→落地，预期 <250 ms）。
- [x] **F4 息屏保活锁（wake_guard，2026-11-01 落地）** `[wake_guard]`：息屏向 `/sys/power/wake_lock` 写
      `chiri_screen_off`（**不写内核超时**：该 sysfs 的超时单位在分支间是秒还是毫秒无保证，写错只会静默提前释放；
      改为「启动盲释放清上一实例遗留 + 屏幕事件/DOWN 停摆/配置热重载三处对齐 + 重启清除」兜底），
      PowerManager 的挂起尝试因 `pm_wakeup_pending()` 直接吃 -EBUSY ⇒ 根本不进 s2idle，绕开 ~6.2 s 撞线窗口。
      `meta.yaml wake_guard_enabled` **缺键按机型**（`common::default_wake_guard_enabled`）：仅 sm8750 默认 true
      （老安装 meta.yaml 不含该键也生效，因为 `sync_meta_snapshot` 对合法文件原样不动、不会补写新键），其余机型 false；
      模板 `config/sm8750/meta.yaml` 写 true、根 `config/meta.yaml` 写 false；WebUI 开关在 **SceneMode 下方**；
      节点缺失（内核无 `CONFIG_PM_WAKELOCKS`）静默停用并打一条 info。**代价**：息屏不进 s2idle，待机功耗上升。
- [ ] **真机复核 wake_lock**：`grep -c scheduler-wake-guard-hold logs/daemon.log` 应≈息屏次数；
      息屏时 `cat /sys/power/wake_lock` 应含 `chiri_screen_off`、亮屏后消失；同时量待机电流（兜底代价）。
      注意这是**唯一**能挡住"干净挂起撞线"的应用侧手段——10-31 23:40 那份只有 2 次 freeze abort、WLAN 故障面≈0，
      照样崩，故 L2 观测与 L3 降噪只能降概率。
- [ ] **真机复核息屏档**：`grep scheduler-screen-latency logs/daemon.log` 抽样确认亮屏时延未被拖慢；
      对比改造前后息屏段 `Freezing user space processes aborted` 次数（两份 dump 实测 39 → 2）。
- [ ] **L2 挂起可观测**：`CLOCK_BOOTTIME - CLOCK_MONOTONIC` 差值 + 亮屏后读 `/sys/power/suspend_stats`
      （success/fail/last_failed_dev）写 `logs/daemon.log`，用于判「这次挂起用了多久、卡在哪个设备」。
- [ ] **L3 临界避让 + 降噪**：连续失败/长挂起后息屏期主动降低事件密度；并确认 eBPF ring buffer
      是阻塞 poll 还是 40/160 ms 定时轮询（决定唤醒源清单）。
- [ ] **6.24 s 不可观测窗口定位（真正的根因，口径已按 ring 深度订正）**：console 已被 `suspend_console()`
      静音且 s2idle debug 打印把 ring 冲刷殆尽（见上方订正条），需 ftrace `power/suspend_resume` +
      `irq_handler_entry` 或 `/sys/power/suspend_stats/*` 落 pstore；只在驱动侧能收敛。
      附：三份 dump 的 `CPU0 next event is …` 均越界（4597 那份 460451274096161、10-31 那份 127872281363636），
      可能是"定时器被编程成过期值 ⇒ 补 pet work 永不触发"的真线索，需 `clockevents` 侧交叉验证。
- [ ] **B2 关掉 s2idle 路径的 debug 打印对**：vendor cpuidle 在每次 `s2idle_enter` 前后打
      `suspend ns / suspend cycles / resume cycles`（~129 B/轮 × 15 万轮/s ≈ **19 MB/s**），2 MB printk ring
      每 ~106 ms 自毁一次 ⇒ 崩溃现场被自己的日志吃掉。应默认关或降为 `pr_debug`；这属于独立缺陷，
      不修 A 系列也应修。
- [ ] **WLAN 半挂死收敛**（第三份 dump 唯一外部推力）：`NETDEV WATCHDOG: transmit queue 0 timed out
      5340 ms`（4582.8）+ `Host TSF sync is not initialized` 88 次 + `vdev is NULL` 84 次 +
      `osif_twt_enable_complete_cb: obsolete request` 33 次 + `wake interrupt received on irq 299` 39 次，
      配合 `adsp_sleepmon … LPI duration 0`（283 s）⇒ 挂起从未成功，41 次重试 / 15 次 freeze abort /
      9 次 device-fail。期望行为：`cnss_pci_suspend` 遇 TX 超时或 vdev NULL 立即 abort + 重载固件，
      `hdd_restart_tsf_sync_post_wlan_resume` 在 resume 前重同步而非只报错。



### 1107 包分析产出（2026-11-07 落地，待真机复核）

> 证据与机制见 `.cursor/docs/2026-11-07-EEM的k口径缺陷与快速切应用延迟治理.md`；数据底盘 = `devimpbin/1107-011543`（sm8550，8 批，20,556,921 行，46.77h）。

- [ ] **复核 C1 `[eem_k_busy]`（EEM k = 忙核数）**：`clg-eem-tick-log` 的 `cores` 应为「util>0.02 的核数」而非 `ceil(Σutil)`；重点看两类场景是否恢复升频：① 6 核各 50%（旧口径 k=3 永远欠供、封在甜点）② 两核各 0.7。判据 = 这两类场景 `eem_khz` 回到甜点以上、且同一时段 `batt_power_w` 不升。回退 = 恢复 `let k = (occupied.ceil() as usize).max(1)`（`cpu_load_governor.rs`）。
- [ ] **复核 C2 `[up_fast]`（升频速率闸判据前置）**：1107 基线 default 档 `up_wait` 占 tick **30.6%**、`up_wait→up` 中位 **478ms**、len3 run 42,723、白等 run 37.9k。目标 = `up_wait` 占比 **<18%**、len3 占比 **<5%**、`up` 总次数增幅 ≤30%（防升频次数爆炸）。另看 `decision=up` 的相邻频差是否变大（跳变一次给足的直接后果）。
- [ ] **复核 C3 `[eem_allowed_cpus]`（规划池可行域）**：1107 基线 `place` ok 里 `clipped=true` **38.3%**、被摘核 **86.1% 落在核 0-2**、65 个 (pid,簇) 对 60s 内被摘 ≥3 次。目标 = `clipped` 降到 <5%、被摘小核占比降到 <20%。注意 `affinity-eem-allowed` 若在部分机型上频繁变化（cpuset 抖动），说明读 cpuset 本身成了热路径，需加缓存（TTL 1s）——**2026-11-07 自审已做**：`EEM_ALLOWED_TTL=1s` 的 `eem_allowed_cache`，且写掩码与可行域**求交**（簇内被排除的核既不写也不计 `clipped`），空集直接 `eem.abort` 不发 syscall。
- [ ] **腾讯视频下轮取证（判定线）**：本仓只命中 1030 包的 81.1s **详情页**（触摸 28.1%、无 fps），因此本轮只加 `activity-first` + 三条拉回规则，**未验证播放页**。下轮判据 = 连续 ≥10min、`mode=playback`、touch <1%、big util 不贴顶、`gpu_busy` 10–40%、fps 无长尾；同时确认详情页/启动页落在 `default`。不满足就撤 `special_tuned.yaml` 的 qqlive 条目（回退 = 删一行 + 三条 app_modes）。
- [ ] **接管种子 `perf_init`（未做，P1 流畅度）**：换模式时 `apply_mode_takeover → reload_config/init_policies` 把 `current_perf` 重置为 `perf_init`（default 0.35）并立即写频 ⇒ 切换瞬间上限从 0.35 重新爬坡（1107 实测切换自身 0.9J/次、开应用爬坡 15.3J/次）。建议种子改为「读 `scaling_max_freq` 或当前 `scaling_cur_freq` ÷ 本簇 hw_max，夹在 `[perf_floor, perf_ceil]`」，无读数才回落 `perf_init`。**注意这是既有契约的取舍点**：`03-chiri.md` 的「热切换配置重放 perf_init」条明确写着「重置并立即写频，避免接管间隙 current_perf 掉到 0 后从地板缓慢爬升」——那条解决的是「掉到 0」，本条要解决的是「从运行值掉到 0.35」。两者不冲突，但**不要直接删掉重放**；改动需与 reduce 档省电口径一起评估（reduce 的 perf_init 0.20 本身就是省电取向）。
- [ ] **`@S` 快照预算闸（未做，P1 流畅度）**：`selfcost` 8 会话 88,987 帧、成本 **68.9% 在 `tid_stat`**（443 次/帧 × 0.06ms），`mean_build_ms ≈ 13.1ms + 61.8µs×tids` ⇒ 1500 tid ≈ 106ms，最长 10.29s（/proc 被 D 状态拖住）。两条一起做：① `meta.yaml devimp_aff_secs` 1→2（零代码，先降频次）② `build_aff_snapshot` 的 tid 循环加 20ms 墙钟预算，超时即截断并在帧头记 `full=0`（口径变更要同步 `05-maint.md` 判读说明）。
- [x] **`status.csv` 加 `daemon_rss_kb`（可观测性缺口，2026-11-07 自审轮完成）**：`STATUS_HEADER` 末尾追加第 28 列；`logger::self_rss_kb()` 读 `/proc/self/status` 的 `VmRSS`（kB，与页大小无关，失败返 0），`status_log_snapshot` 加参。旧 27 列归档按缺列读（WebUI 解析同时接受 27/28，`STATUS_COLUMN_COUNT=28`，mock 与单测同步）。**用法**：只看跨会话趋势（同包两批的 p50/p95 斜率），不看单点；下轮 `dvstatus` 若需按包统计再补列名。
- [ ] **FAS 帧源静默 58 次（王者细微卡顿的候选根因）**：1107 包 `[ERROR] [FAS] 帧反馈静默，进入开环` 58 次、`[FPS Monitor] 帧源静默` 123 次，全部落在 `com.levelinfinite.sgameGlobal`；FAS 转开环后「档位不再调整」，只能靠负载控制。待查：探针 attach 后为何 Ns 无帧事件（SurfaceFlinger 层名/进程重启/choreographer 失效），以及开环期间是否应改用「负载 + 掉帧估计」的降级闭环。
- [ ] **bili 两条 activity 规则裁决**：`rules.yaml` 给 `tv.danmaku.bili/.MainActivityV2`、`/BiliMainSearchActivity` 配了 `default`，但 bili 是特调白名单包（无 `activity-first`）⇒ 两条规则**写而不生效**，1107 实测这两个 activity 共 63,587 tick 全走 `playback`。要么给 bili 也加 `activity-first`（搜索页/详情页回 default，有触摸升频），要么删掉这两条规则（承认整包 playback）。**不要只改优先级顺序**——那会一次性改掉所有白名单包的行为。判据 = 这两个 activity 的触摸密度与 fps 长尾，对照 1107 的 `playback` 段。
- [x] **`energy_model::best_placement` 文档与实现不符（2026-11-07 自审轮完成）**：`(false,false)` 改为取**供给更大**者（`cand.supply > b.supply + f32::EPSILON`），供给并列才保留先到者；`(true,true)` 仍按 cost+`hysteresis`。回归 `placement_prefers_max_supply_when_none_can_supply`（demand=2.0 / cores=1，断言取各簇 top 档最大单核供给）。生产侧 `cheaper_elsewhere` 已有 `p.supply >= demand` 闸，无行为变化。
- [x] **`narrow_skip` 多槽（2026-11-07 自审轮完成）**：按簇分键 `narrow_pid: HashMap<(i32, CoreGroup), Instant>`，`narrow_skip` 查 `(pid, cluster)`；新增 `remember_narrow` 统一 insert + 超 512 条按到期清理；`note_narrow`（仅 EINVAL）与 `note_stripped`（回读缺簇）共用。跨簇并集掩码不再能绕过另一簇的禁令。

### 2026-11-07 自审轮修复（C1–C4 落地后自查发现，均已改 + 回归）

> 审查对象 = 上一条 1107 包的 C1–C4；判据 = 不改变已验证行为的前提下补齐机制缺口。回归全绿：`cargo fmt` / `cargo check -p chiri`（exit 0，13 条既有告警）/ `cargo test -- special_tuned_tests activity_tests` 30 passed / 离线 energy_model 40、affinity_cpuset 19、app_mode_activity 10、clg_tuning 5、diag_slice 6。

- [x] **activity-first 漏 `dynamic_enabled` 闸**（`src/monitor/app_detect.rs`）：原判据只看 `activity_first && has_activity_rule`，而 activity 规则只在 `config.dynamic_enabled` 时生效 → 关闭动态模式后白名单应用**先让位、再落到 global_mode**，特调整包丢失。改为 `special_yields_to_activity(activity_first, dynamic_enabled, has_activity_rule)`，测试五断言含「动态关时让位即掉出特调」。
- [x] **`eem_allowed_cpus` 每拍读 cgroup**（`src/chiri/affinity.rs`）：`apply` 160ms 一拍、每次 2 个 seq_file；加 `eem_allowed_cache: Option<(Instant, Option<Vec<usize>>)>` + `EEM_ALLOWED_TTL=1s`。
- [x] **写掩码未与可行域求交**（同文件）：C3 只按簇剔除，簇内部分核被 cpuset 排除时仍照写并被静默摘。新增 `dst_mask`（`a.mask ∩ allowed`），空集 `eem.abort` 不发 syscall；`mask_write_allowed`/`set_tid_affinity`/`clipped`/日志 dst 全用 `dst_mask`，`clipped` 以「实际写下」为基准。
- [x] **`status.csv` 的 28 列契约同步到 WebUI**（`webui/src/data/status-csv.ts`、`tests/data-parse.test.ts`、`src/dev/mock-shell.ts`、`README.md`）：解析器原先按「字段数 == 27」过滤，加列后会**整文件丢行**；现同时接受 27/28（旧归档 `daemonRssKb=null`），mock 与单测同步。
- [ ] **待真机复核（自审轮不写结论）**：① 1s TTL 是否让 cpuset 变化在 1s 内滞后（切换应用瞬间允许集变宽/变窄）——看 `aff_action` 的 `clipped` 是否在换包 ±1s 出现尖峰；② `dst_mask` 求交后空集路径：已加 `result=skip reason=dst_empty` 留痕，且 `abort` 的 `fail_streak` 会走 `FAIL_BACKOFF_BASE` 退避（4s→30s），理论上不会每拍重试——下轮看 `skip` 计数与同一 tid 的重复频率即可定论。

### 终末地性能优先档 `[perf_lock]`（2026-11-07 落地，待真机复核）

> 用户 2026-11-07 指令：为终末地提供性能增强，忽略功耗开销，以「绝对流畅、绝对不卡不掉帧、尤其没有小卡顿」为唯一目标。机制与取舍见 `agentsdocs/03-chiri.md` 的 `[perf_lock]` 条；配置 = `module/config/normal/fas/endfield.yaml`；回归 = `scripts/tests/fas_perf_tier_regression.rs`。

- [x] **`perf_lock` 落代码**：`src/fas_types.rs` 新增 `perf_lock: bool`（`#[serde(default)]`，缺省 false）；`policy_mgmt.rs::apply_freqs` 在 util 软封顶之后、`thermal_cap` 之前 `self.perf_index = ceiling; effective_perf = ceiling;`，且 `let load_only = !locked && self.uses_load_control();`（开环分支按 util 下探，与锁频互斥）；`controller.rs::adjust_target_for_util` 与 `frame_pipeline.rs::apply_steady_decay` 在锁频档直接 return（`apply_steady_decay` 顺带由私有改 `pub(super)` 供单测）；`load_policies` 打 i18n `fas-perf-lock`，`reload_rules` 支持热重载。
- [x] **终末地改性能档**：`perf_lock: true` + `fps_gears: [60.0]`（单档 ⇒ `next_gear/prev_gear` 恒 None，齿轮机整段不执行，掐掉「降档加速压到 0.90」与「原生档 60↔120 翻面」）+ per-app `target_fps: [60]`；`core_temp_threshold: 0.0` 保留（FAS 侧不降频，热量交内核 thermal）。
- [x] **验证基线**：`cargo fmt` 干净；`cargo check -p chiri` exit 0（13 条既有告警）；新增 4 条单测全过（`perf_lock_keeps_max_freq_under_util_cap_and_jank` / `_bypasses_load_only_demand` / `_freezes_target_fps_offset` / `_skips_steady_decay`）；离线 `fas_perf_tier_regression` 4 passed；全量 `cargo test -p chiri --bin chiri` = **278 passed / 3 failed**（既有 `fas_manager::tests` 三条环境依赖）。
- [ ] **真机复核**：① `daemon.log` 出现 `fas-perf-lock`；② `[FAS] ... | P:` 全程 1.00、`snap` 里三簇 `cur/max` 恒 100%；③ `fas-gear-switch` / `fas-downgrade-boost` / `fas-native-gear-detected` 零条；④ `fas-loop-open` 期间频率不掉（这是本档的核心收益）；⑤ 观察 `qos_clamped`——若内核 thermal 把 `scaling_max_freq` 压到目标以下会告警但不重写，持续钳制说明得降温（或回退到保留 `core_temp_threshold`）。
- [ ] **GPU 锁频评估（未做，需证据）**：FAS 现在完全不碰 GPU（`src/chiri/gpu.rs` 的 `GpuGuard` 只给 lab contingency 用）。开锁的判据 = 真机日志 `gpu_busy` 长期 ≥95% 且 CPU 已锁最高频仍有掉帧。**风险**：8550 实测读 GPU 节点会把 GPU 拉起（+47% 功耗），锁最高频显著抬 SoC 温度，可能诱发内核 thermal 压 CPU → 更卡。故无证据不开。
- [ ] **是否套用到 sgame/lolm/speedmobile（待裁决）**：1107 包 sgame 的三条掉频来源（开环锯齿 58 次、降档加速 P→0.90 三十条、原生档 60↔120 翻面）都被 `perf_lock` 一并解决，但电竞长时场景热负载更高，「忽略功耗」需用户定。套用 = 该应用 yaml 加 `perf_lock: true`，`fps_gears` 是否收成单档按机型刷新率决定（单档会让齿轮机失效，但 `perf_lock` 仍把频率钉在最高）。
- [ ] **`effective_perf_floor()` 的 0.45 硬顶（未改）**：`controller.rs` 的 `(base + fps_bonus).min(0.45)` 让任何 `perf_floor > 0.45` 失效。`perf_lock` 绕过它（写频只认 `perf_ceil`），本轮不动；将来若要做「半锁」（floor 0.8 保底 + PID 微调），需改成可配字段并同步 `normalize()` 的交叉约束段。
