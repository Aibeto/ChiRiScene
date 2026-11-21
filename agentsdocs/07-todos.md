## [todos] 未竟事项台账（2026-09-26 汇总）

> 历史计划/日志归档后的**唯一 TODO 权威**。开工前先查本文件；完成一项删一项。2026-09-17 审查提出的旧项标注「先核实」——动手前先确认现行代码是否已修。
>
> **口径变更（2026-09-28）**：R3「**永不再新增 A/B 测试**」生效（详见 `04-hard-lessons.md` 的 `[hard]` 首条）。以 A/B 为手段的条目一律作废，改「离线重放 + 同版本观察」。
>
> **2026-10-20 清空**：用户裁决「已完成的全删、其余待办全删」，被删条目若需追溯看 git 历史（`git log -p agentsdocs/07-todos.md`）。
>
> **2026-10-20 二次整理**：本文件只留「还没做 / 等拍板」的条目。已完成项、机制说明、硬件基线、环境限制、定案记录已移到对应子块——FAS 机制与定案见 `03-chiri.md`，禁令与教训见 `04-hard-lessons.md`，历史快照判读口径见 `05-maint.md`，内核 / DT 基线与 dtsi 对账见 `06-kernel.md`，构建与本地验证环境见 `01-overview.md`，`soc.yaml` 与前沿表的生成口径见 `02-convention.md`。

### 待办（性能与探针）

- [ ] **网易云 USB 独占切换卡音：同机时间对齐取证**：30216 / SM8550 PHB110 的 activity 拟合在 1117 包见 PlayerActivity n=69,138、P_avg=2.84W，在 1119 包 n=7,130、P_avg=3.28W；40000 有 rhine.music.database（454 个 main 记录，多为 default），但对应日志没有可归因到 USB 独占、切 APP/系统动画或音频 underrun 的事件，不能据包名套用 playback。现有活动/功耗聚合不说明切换时延。复现时同步记录 USB 独占状态、前后台包/activity 与 mode/PID、切换/动画时间、音频 underrun/xrun、帧间隔、CPU/GPU 负载与频率、PSI 和 batt_power_w；用同一设备会话定位音频卡顿对应的调度动作后再最小修正。

- [ ] **高负载卡顿 / 低负载功耗高：建立同机负载-响应证据**：当前拟合仅给候选：40000 / SM8550 小米包 1116 的国服 sgame n=79,314、P_avg=5.45W、big util=.50、f/max=.57，但 FPS 全空；30216 / SM8550 PHB110 包 1119 的国际服 sgame n=6,092、big over=2.5、util=.63、f/max=.81、P_avg=4.20W，是单包性能不足候选，未与掉帧对齐。同一 1119 包 launcher n=42,631（big util=.32、f/max=.33、P_avg=2.08W）及 Pixez n=9,378（P_avg=1.11W）只是不同 activity 的低负载参照，不是同一连续会话，不能证明低负载功耗偏高。仍需在同一设备和版本按时间对齐工作负载、前台 mode、逐核 util/freq 与 governor 决策（up_wait/up/down/hold）、迁移计数及统计窗口、PSI、GPU busy、FPS/帧间隔、温度/thermal cap 与 status.csv batt_power_w；分别确认高负载掉帧是否供给不足，以及低负载功耗是否由过高频/迁移/守护开销造成。不得跨设备/ROM/SoC横比，缺字段先补取证，不用 cpu_dyn_w 作为物理下界，不新增 A/B。
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
- [ ] **内核 sched 迁移旋钮按设备核验**：30216 / SM8550 的 `devimpbin/1121-001327/x_1120-032533/daemon.log:14-15` 明确报告 `/proc/sys/kernel/sched_nr_migrate` 与 `/proc/sys/kernel/sched_migration_cost_ns` 不存在，因此配置的 `migration_cost_ns: 400000` 对该内核未生效；40000 / SM8475 包没有同类告警，但不能据此认定节点存在或旋钮生效。旧 A02-14 五机也曾报告同节点缺失；`fas_manager.rs` 已逐分支告警。下次按设备/内核版本采集 `/proc/sys/kernel/` 与 `/sys/kernel/debug/sched/` 的实际节点，并对照 `module/config/normal/fas/{sgame,dfm}.yaml`、`tuned.rs` 写入和恢复路径；只适配确实存在的接口，不猜节点名。
- [ ] **i18n 文案与行为不符（顺手可改）**：`governor-residue-cleanup` 写「已恢复 schedutil」，实际恢复目标是
      `common::cpu_governor_for_policy(pid)`（walt / uag / sched_pixel…）。只改文案，不动行为。
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

- [ ] **sm8250 真机复核**：确认 `monitor-cpu-ebpf-unavailable` 出现、CLG 看门狗不再 release、status.csv CPU 列有值。
- [ ] **sched 迁移参数缺失降级**：`sched_migration_cost_ns`/`sched_nr_migrate` 在零售 GKI 不存在
      （CONFIG_SCHED_DEBUG=n，节点只在 debugfs），每次 boot 刷 38 条 WARN。应探测一次、缺失即静默并标不可用。
- [ ] **3 条 FAS 单测需桩**：`activate_*`/`repeated_activate_*`/`same_package_pid_*` 依赖编译期白名单里的
      `game`（实际不存在）+ 真机 sysfs，宿主机恒红；应注入白名单/policy loader 或 `#[ignore]`。

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

- [ ] **复核 C1 `[eem_k_busy]` 真机效果**：`src/chiri/cpu_load_governor.rs:42-43,967-985` 已按 `util > 0.02` 统计忙核数，并以 `k = busy_cores.clamp(1, curve.cores().max(1))` 计算 EEM k；不再把它列为待实现算法。30216/40000 未含 `clg-eem-tick-log`/`eem_khz`，无法核对六核各 50%、双核各 0.7 等负载下 EEM 目标是否落在曲线甜点，也缺同设备 `batt_power_w` 轨迹；保留为真机验证项，不做新 A/B。
- [ ] **复核 C2 `[up_fast]` 与升频确认**：实现见 `src/chiri/cpu_load_governor.rs:691-715`。1107 验收线为 `up_wait <18%`、len3 run <5%、`up` 总次数增幅≤30%；本次 30216 rate=3 固定输入五批 up_wait 21.02%、up +15.6%，单会话王者 up_wait 15.2%、up +9.6%，但 len3 和真实 FPS/功耗未验证。下轮同版本、同场景观察帧间隔与 `batt_power_w`，不得做并行/交错 A/B；实机功耗或帧表现回退时恢复 rate=4。

- [ ] **SM8550 高区门控与单核突发验收**：配置 `high_zone_gate=0.85`、`high_zone_count=2`、`high_zone_util=0.90`（reduce/default/boost）；`devimpbin/1116-003204/x_devimp_1114-010906/main_com.ss.android.ugc.aweme_1113-223222.log:2437` 记录 little `hz_hold`（`over=1, under=1, cur/tgt=.94/.94`），但这是抖音窗口，仅单个门控命中，无逐帧掉帧或 `high_zone_wait` 计数。SM8550 当前 `up_jump_threshold` ≥0.25，而高区内 `target_perf≤1.0` 且 `current_perf≥0.85`，所以高区剩余跃升最多0.15，不能据“挡住显著跳变”判故障。设备验收需同一时间轴记录每核 util、`high_zone_wait/hz_hold`、真实目标/写入频率与逐帧间隔；只有门控等待与长帧重合且确认核数条件错配，才调整高区策略。现有固定输入回放是单核适配，不能作为多核门控验证。
- [ ] **EEM cpuset 可行域真机复核**：验证 `eem_allowed_cache` 的 1s TTL 与 `dst_mask = a.mask ∩ allowed` 在 cpuset 变化时的实际效果；观察换包前后 `aff_action` 的 `clipped`，以及 `result=skip reason=dst_empty`、同一 tid 重试间隔（`FAIL_BACKOFF_BASE` 4s→30s）。确认无无效亲和写、缓存滞后可接受且空集不逐拍重试；旧 1107 比例不作为新版本验收线。
- [ ] **腾讯视频下轮取证（判定线）**：本仓只命中 1030 包的 81.1s **详情页**（触摸 28.1%、无 fps），因此本轮只加 `activity-first` + 三条拉回规则，**未验证播放页**。下轮判据 = 连续 ≥10min、`mode=playback`、touch <1%、big util 不贴顶、`gpu_busy` 10–40%、fps 无长尾；同时确认详情页/启动页落在 `default`。不满足就撤 `special_tuned.yaml` 的 qqlive 条目（回退 = 删一行 + 三条 app_modes）。
- [ ] **接管种子 `perf_init`（未做，P1 流畅度）**：换模式时 `apply_mode_takeover → reload_config/init_policies` 把 `current_perf` 重置为 `perf_init`（default 0.35）并立即写频 ⇒ 切换瞬间上限从 0.35 重新爬坡（旧记录曾报 1107 切换自身 0.9J/次、开应用爬坡 15.3J/次，但本轮未找到原始量测报告，待同机复测确认）。建议种子改为「读 `scaling_max_freq` 或当前 `scaling_cur_freq` ÷ 本簇 hw_max，夹在 `[perf_floor, perf_ceil]`」，无读数才回落 `perf_init`。**注意这是既有契约的取舍点**：`03-chiri.md` 的「热切换配置重放 perf_init」条明确写着「重置并立即写频，避免接管间隙 current_perf 掉到 0 后从地板缓慢爬升」——那条解决的是「掉到 0」，本条要解决的是「从运行值掉到 0.35」。两者不冲突，但**不要直接删掉重放**；改动需与 reduce 档省电口径一起评估（reduce 的 perf_init 0.20 本身就是省电取向）。
- [ ] **@S 全量快照主线程长尾（修复已落地，待设备验收）**：九包 daemon 的 build >50ms 告警 54,706 条、>100ms 4,546 条、最大 7.8008s（1116-003204；每包14–16,219条，仅为阈值日志计数）。devimpbin/1116-003204 的 @S build 2.5350s/4.0228s/7.8008s/3.4191s 是旧包诊断长尾，早于当前快照并行采样与分片优化，不能代表修后设备表现。main tick 仅在决策签名变化或 2s 心跳时落行（src/logger.rs:843,1649-1716），与 build 时间重叠或相邻行间隔都不能作为 scheduler tick 周期或卡顿因果证据。源码确认 build_aff_snapshot 在线程内同步执行（src/chiri/mod.rs:3132-3178），D1 仅异步 render/write（DIAG_WORKER_ENABLED=false，src/chiri/mod.rs:1170）；每16个候选 yield 一次也是该同步路径上的额外调度点（src/chiri/diag_slice.rs）。已改为每256个候选 yield；513候选的让出次数由32降为2，候选、线程/帧语义、1s刷新频率及日志字段均不删减。另将 /proc/<tid>/stat 读取改为复用4线程池、每批16个TID并行采样，按输入顺序串行应用差分；刷新补采也使用同一批次读取，逐TID读取一次，保留所有候选和anchor。selfcost 的 tid_stat 现以批次统计非重叠wall/caller CPU，worker CPU另列，schema升级至 selfcost-build/2。离线实现与回归不等于真机收益。selfcost Residual 含循环外、锁、差分及让出等未拆分成本，不能单独归因；设备验收需确认候选/字段数与采样频率不变；直接测 scheduler 事件入队到处理的墙钟延迟，并与 @S build wall/caller CPU/worker CPU 对齐，不能用 main tick 行间隔代替。待目标设备单版本高负载与低负载日志验收：完整快照样本/字段数不减，@S build 成本与 scheduler 事件入队到处理延迟分别报告 p95/max；记录 worker_cpu、daemon CPU 和 batt_power_w，确认低负载下没有新增的持续采样开销或功耗上扬，并核对高负载收益。不得用降采样或20ms截断换取表面改善，不做并行/交错A/B。
- [ ] **@S 冷进程名称延迟解析设备验收**：SM8150 旧包同一 68s 窗口 `snapshot_procs` wall 863.5ms，其中名称读取与源码路径共同指向冷 `/proc/<pid>/cmdline`/`comm` 是长尾候选；该包不是修复后版本且无 UI 帧时间轴。目标设备记录 `@S` build p95/max 与 scheduler 事件入队→处理延迟，并核对完整 TGID 行数/util 基线、名称语义/字段及采样频率不变；主机回归只证明全量快照和选中 PID 解析，不证明真机延迟或交互改善。
- [ ] **王者国际服微掉帧：对齐 FAS 帧反馈与逐帧证据**：1107 曾有 sgame 帧源静默（FAS 58 次、FPS Monitor 123 次）；30216 的 `devimpbin/1121-001327/x_devimp_1120-032533/main_com.levelinfinite.sgameGlobal_1120-013050.log:6-45` 为 `mode=default`，相邻 `devimpbin/1121-001327/x_1120-032533/daemon.log:2370` 记 PID 10159 在 01:30:49 `default -> fas`，main 从 01:30:50 起，模式/PID 对齐未证。九包中 30216 的 `status.csv` 有 FPS 每秒样本：1112-020657 n=13,321（p10/p50 121.38/123.25）、1113-165328 n=5,050（67.91/123.08）、1117-215156 n=14,872（119.16/123.60）、1119-230718 n=7,111（121.06/124.26）；这些是约 1s 聚合样本，不是逐帧间隔，不能据低分位判定微掉帧。1119 SM8550 会话 daemon.log:2083-2119 有 native 120→60、约3秒后60→120并同秒再 native降档；原因与 native直降没有upgrade cooldown、低perf升档确认后可回摆相符。src/scheduler/fas/gear_state.rs 已让 native 与普通降档共用同档指数退避，首次冷却按配置90帧×2并按目标FPS缩放，重复同档降档递增；显著超帧快速升档仍不受冷却阻挡。controller 回归覆盖首次/重复native降档、冷却期低perf hold、真实超帧快速升档及冷却后再升档。该修复针对源码可证的回摆机制，不等于证明用户感知微卡顿全部由它造成；设备验收需确认档位不再约3秒反复翻转、真实高刷输出可快速升档且逐帧长尾改善，并与CPU/GPU负载、thermal及batt_power_w对齐；未有上述证据前不得宣称主观体验/功耗已验证。40000 小米包 devimpbin/1116-003204 的国服 sgame 有 36,926 个 FAS snap 但 FPS 全空；daemon 日志计数有帧源静默 160、反馈静默 498、恢复 25、uprobe 挂载 236、PID 切换失败 6、持续开环 21（包级文本计数，不等于掉帧数）。国际服唯一性能不足候选在 devimpbin/1119-230718/activity-fit.txt:333（n=6,092、big over=2.5、util=.63、f/max=.81、P_avg=4.20W），未与掉帧时间关联，不足以增加频率下限/最高频锁或新特调。单会话默认档固定输入回放 25,355 tick（screen_on 全未知）：rate=4/jump=0.55 模拟 up_wait 5,194（记录 5,345）；仅 rate 改 3 后 up_wait 3,851（20.5%→15.2%）、up +9.6%，该会话理想频率 sink P95/P99 不变。五批 rate3 合计 up_wait 占比 27.17%→21.02%，一批 P95 +57.6MHz。输入 util 固定、无频率反馈/FPS/功耗，不代表实机效果；SM8550 default 已改 rate=3，待同版本同场景核验逐帧间隔与 `batt_power_w`；jump 0.45/0.35 未见目标会话收益，维持 0.55。开环仍执行负载调频（`load_only = !locked && self.uses_load_control()`），不能称为频率冻结。下轮按 PID/generation/时间戳对齐 raw_foreground_package、snapshot package/PID/generation/starttime、窗口可见性、FAS enabled/ready、cooldown/halted、fallback 激活结果、probe attach/首帧及真实帧间隔，再核对 CPU/GPU 负载和档位；先判定静默属于无帧、进程/Surface 映射或探针事件路径，这部分帧源/身份问题独立于本次已修复的 native 档位回摆；仅在确认探针或状态链路有缺陷后再修闭环，不删身份 guard、不改阈值/gear，也不凭现有包改 `perf_lock`。
- [ ] **bili 两条 activity 规则裁决**：`rules.yaml` 给 `tv.danmaku.bili/.MainActivityV2`、`/BiliMainSearchActivity` 配了 `default`，但 bili 是特调白名单包（无 `activity-first`）⇒ 两条规则**写而不生效**，1107 实测这两个 activity 共 63,587 tick 全走 `playback`。要么给 bili 也加 `activity-first`（搜索页/详情页回 default，有触摸升频），要么删掉这两条规则（承认整包 playback）。**不要只改优先级顺序**——那会一次性改掉所有白名单包的行为。判据 = 这两个 activity 的触摸密度与 fps 长尾，对照 1107 的 `playback` 段。

- [ ] **普通交互卡顿与 QQ Activity 模式抖动：同机对齐取证**：40000 / SM8550 会话 devimpbin/1116-003204/x_1114-010906/daemon.log:353-518 在约 222s 内记录 17 次 QQ default/boost 来回，切换伴随 CLG 按 perf_init 重建（default P=0.35、boost P=0.60；如 :359-376）。当前 module/rules.yaml:23-27 给 QQ 两个 Activity 配了 boost，但设备二进制实际嵌入规则、采样时点与前台 Activity 尚未对齐。app_detect.rs:1043-1117 的模式由当轮采到的 Activity 判定；logger.rs:855-857,931-946,1055-1075 的 Activity 列只是最近一次采样值，ModeChange 事件不携带触发 Activity，不能用事件行显示值证明规则误判。九包普通场景没有逐帧长尾或 UI 动作时间轴，故不能认定这 17 次切换造成卡顿，也不盲加 Activity 滞回或改升频参数。下一次单版本同机记录：二进制/内嵌规则版本、前台包/PID/starttime/generation、模式判定所用 Activity 及采样时间、ModeChange 入队到处理时间、CLG worker stop 发出/各 worker join 完成/新 worker 初始化/实际频率变化的时间，以及切换/启动/返回/滚动动画时间、FrameTimeline 或逐帧间隔、CPU/GPU/PSI/thermal。CLG worker stop 路径现已先置 stop 并向各有界通道非阻塞发送空唤醒包；worker 收包后在处理负载前检查 stop，再退出并 restore，scheduler 仍同步 join。主机回归覆盖阻塞接收被唤醒、接收返回后检查 stop、队列满时由已有消息唤醒，以及 stop 先于接收调用的竞态；未构造真实 sysfs Worker，不代表设备上的退出/restore耗时。下次同机记录 stop 发出、各 worker join 完成（含 restore）及新 worker 初始化/实际频率变化的时间，量出完整墙钟；不能把旧的 recv_timeout 上限当作实际卡顿时长，也不能把唤醒修复当作设备卡顿已消除。先确定 QQ 模式反复切换是合法页面变化、样本时序错位还是配置版本不一致，再判断是否存在无效重建或可证的切换阻塞。采集不新增生产高频采样，不做 A/B。

### 终末地性能优先档 `[perf_lock]`（2026-11-07 落地，待真机复核）

> 用户 2026-11-07 指令：为终末地提供性能增强，忽略功耗开销，以「绝对流畅、绝对不卡不掉帧、尤其没有小卡顿」为唯一目标。机制与取舍见 `agentsdocs/03-chiri.md` 的 `[perf_lock]` 条；配置 = `module/config/normal/fas/endfield.yaml`；回归 = `scripts/tests/fas_perf_tier_regression.rs`。

- [ ] **真机复核**：① `daemon.log` 出现 `fas-perf-lock`；② `[FAS] ... | P:` 全程 1.00、`snap` 里三簇 `cur/max` 恒 100%；③ `fas-gear-switch` / `fas-downgrade-boost` / `fas-native-gear-detected` 零条；④ `fas-loop-open` 期间频率不掉（这是本档的核心收益）；⑤ 观察 `qos_clamped`——若内核 thermal 把 `scaling_max_freq` 压到目标以下会告警但不重写，持续钳制说明得降温（或回退到保留 `core_temp_threshold`）。
- [ ] **GPU 锁频评估（未做，需证据）**：FAS 现在完全不碰 GPU（`src/chiri/gpu.rs` 的 `GpuGuard` 只给 lab contingency 用）。开锁的判据 = 真机日志 `gpu_busy` 长期 ≥95% 且 CPU 已锁最高频仍有掉帧。**风险**：8550 实测读 GPU 节点会把 GPU 拉起（+47% 功耗），锁最高频显著抬 SoC 温度，可能诱发内核 thermal 压 CPU → 更卡。故无证据不开。
- [ ] **sgame/lolm/speedmobile 是否启用 `perf_lock`（按应用分别裁决）**：本次 1119 sgame 120↔60 原生档回摆已由 native 降档共用既有指数退避修复；不能再称该问题只有 `perf_lock` 能解决。1107 的开环、降档 boost 和长时高热负载仍是不同机制，且用户确认帧源静默时开环属预期；不要为消除合法开环而锁频。是否忽略功耗并锁最高频仍需按应用目标与设备热表现单独决定，不能把 sgame 结论外推到 lolm/speedmobile。若启用，`fps_gears` 是否收成单档须按机型刷新率评估；单档会停用齿轮切换，但 `perf_lock` 仍把频率保持在最高档。
- [ ] **`effective_perf_floor()` 的 0.45 硬顶（未改）**：`controller.rs` 的 `(base + fps_bonus).min(0.45)` 让任何 `perf_floor > 0.45` 失效。`perf_lock` 绕过它（写频只认 `perf_ceil`），本轮不动；将来若要做「半锁」（floor 0.8 保底 + PID 微调），需改成可配字段并同步 `normalize()` 的交叉约束段。
