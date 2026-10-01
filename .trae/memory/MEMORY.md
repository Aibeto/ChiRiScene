# ChiRi 项目记忆（Trae 环境）

> 长效事实：就地更新、保持精简。架构约定 / 契约数字 / 命令口径的**正文一律在 `agentsdocs/`**，
> 本文件只记「本环境开工必须先知道、且不在 agentsdocs 里的结论」与最新契约指针。
> 历史统一口径期（2026-09-13 ~ 09-23）的长期事实在 `.codebuddy/memory/MEMORY.md`（各环境互读不互写，只读参考）。

## 2026-10-02：devimp 产出与 ChiRi 调度线程解耦（非 ChiRi 也能开 devimp_main）

- **根因**：devimp 全部写入点都在 `src/chiri/mod.rs` 的 `scheduler_ipc` 1s 块内；非 ChiRi 不启动
  调度线程 → 一行都不写（DOWN 停摆表象亦源于此，ChiRi 真 down 时 1s 块未被 `!halted` 门控、devimp 照写）。
- **修法（用户口径「devimp_main 必须始终能独立开启」）**：新增 `chiri::start_diag_thread(config_path)`，
  `main.rs` 仅在**调度器未启动分支**起它（线程名 `diag_writer`，与调度线程互斥、无重复写入）。每秒
  `main_snap` + `build_aff_snapshot`（已解耦，传空被管表）+ `aff_snapshot`，屏幕/模式变化写 `main_event`；
  `meta.dev_record` / `devimp_top_n` 每 5s 重读（热重载）。只读快照、不写任何 sysfs。
- **非 ChiRi 取值口径**：mode 列 = `app_detect::last_determined_mode()`（空回退 rules `global_mode`）；
  thermal cap=100 / clg=0 / wakeups·migrations·freq_trans=0（均 ChiRi 专属语义）；CPU/GPU 频率照采。
- **配套**：`build_aff_snapshot` 签名与 `AffinityManager` 解耦（非 ChiRi 传空被管表）；`monitor/mod.rs` 的
  telemetry 线程由「仅 ChiRi」放开为**全 SoC 启动**（`main_snap` 数据源）；`logger::note_write` 仍非 ChiRi
  早退（不自动重启，目录靠 128MB 轮转 + `enforce_dir_limits` 约束）。
- 逐文件改动清单、i18n 新增 key 与校验结果见当日日志 `.trae/memory/2026-10-02.md`；契约正文见
  `agentsdocs/02-convention.md` 的「开发诊断日志（devimp/）」条。

## 2026-10-01：scenemode 改为不下线（修法3）+ 前台不落 little + 功耗尺子结论

- **scenemode 抖动定量实证（`logd_1001-045147`，A08-08/8550）**：26 次进入中 **20 次（77%）** 在 11~414 s 内被
  `持续顶满（little util 100%）→ 退回 reduce + 300s 冷却` 撤销；有效驻留 ≈3.6 h、冷却禁入 ≈1.7 h。
  根因 = 09-28「下线 little 簇」把负载挤到**唯一在线的引导核 CPU0** → 单核顶满 → 自我撤销。
- **修法3（8550 生效）**：息屏 **不下线任何核**，保留全核消除单核瓶颈，省电交回 `scenemode.yaml`
  的 `perf_ceil 0.12` + 后台 uclamp。软回退 = `CoreCtl.scenemode_offline: true`。
- **`CoreCtl.scenemode_offline` 曾是死字段**（`config.rs` 标 `#[allow(dead_code)]`、`mod.rs` 未接线，
  下线由 `enabled` 无条件驱动）→ 已接线为 `enabled && scenemode_offline` 真门控；`Default` 仍 `true`。
- **命名坑（已修）**：`apply_affinity_and_corectl` 末位形参原名 `scenemode_offline`，实收调用方 `scene_mode_active`
  → 已重命名 `scene_mode_active`，与新配置字段显式区分（同名不同义易致极性反转）。
- **`normal_fg_exclude_little: true`（8550）**：亮屏 UI 功耗偏高（launcher 2.61 W vs 基线 1.9~2.4 W），
  A510 能效差 + DT 模型低估 → 收窄 normal 前台掩码。原「待 32 位核位图」阻塞按用户口径作废
  （转译层下全核可跑）。
- **功耗尺子结论：准，问题不在 ChiRi**。① 安装链 `customize.sh` 探测到 bcc_parms → 写 `oplus_chg: true`
  - divisor **1000**（`meta.yaml` 的 `1000000` 只是模板缺省）；daemon.log 首读数快照逐包印证
    V=3.69~4.32 V、I=0.2~5.5 A，与 PHB110 电池相符。② `dvpower [8]` 与 status.csv 几乎相等
    （FAS 3.95 vs 3.95、Alipay 9.60 vs 9.60、闲鱼 5.18 vs 5.23）→ 脚本「devimp 系统性高估」注释
    **与实测自相矛盾**（本机电流已到 0.2~5.5 A，整数化不再抹轻载秒）。**已修（2026-10-01）**：
    `scripts/devimp/dvpower.py` 四处注释改为「历史口径（仅轻载场景/旧包适用），勿外推」。
- **契约数字变更（2026-10-01，正文见 `agentsdocs/`）**：
  ① `status.csv` 23 列 → **25 列**（末两列 `daemon_utime_ms`/`daemon_stime_ms` = daemon 自身 utime/stime×10，
  经 `logger::self_cpu_ms()` 读 `/proc/self/stat`，用于自测量基线）；
  ② `feature.yaml` 取值由「命中 SoC 取整份、否则取根」改为**根为基底 + SoC 按字段深合并**
  （`common::embedded_feature_str()` + `merge_yaml`：map 递归、非 map 整体替换）；
  **副作用**：8550/8998 无 `vector:` 段 → 现继承根的 vector，极速档影响待真机复核（8475 本就有段）。
  另：两处 fps/cpu 监控 runtime 由 `Runtime::new()`（multi-thread）改 `new_current_thread`，tokio 特性 `rt-multi-thread`→`rt`。
- **非 ChiRi SoC 恒为停摆（2026-10-01）**：`main.rs` 调度器未启动分支调 `down::project_unsupported` —— 只写
  `current_mode.chr = down` 投影 + 置进程级 `DOWN_ACTIVE`，**不写 `down.chr`**（输入文件仍归用户；该分支下
  `on_startup`/`down_watcher` 不起，down.chr 读写本就不生效）。WebUI 侧 `deviceKind` 由两态改三态
  `chiri | unsupported | unknown`（读到 active_config.chr 但无 `/` 才是 `unsupported`）；高级设置的停摆开关
  `downFixed = deviceKind === 'unsupported'` 置灰恒显示停摆（提示 key `config.down.hint.unsupported`）。
  正文见 `agentsdocs/02-convention.md` 的 DOWN 小节。

## 2026-10-01（续）：devimp 诊断口径 + 两处探针定案 + 核秒评估

判读包 = `devimpbin/1001-190148`（A08-11/10811、8550/PHB110、kernel 5.15.180-android13、两批次
`x_*_1001-190143` / `x_*_1001-154231`）。**注意 `devimpbin/` 被 .gitignore，Grep 目录级会跳过，须显式给文件路径**。

- **三结论回写（用户 item 9「确认」）**：
  ① **热档中档解除点修复已生效**：8550 `hysteresis_mid_c: 1.5` → 中档解除点 `41.5℃` > 软限 41℃。
     实证 cap=60 温度带 **min 41℃**（0928 为 40）、驻留 **1251 s**（0928 5152 s），解除后直接交软档 0.85。
  ② **CLG-PF 全程生效**（0928「头号待办」capacity-missing 回退已在 1001 消失）：两批次均打
     `[CLG-PF] 帕累托前沿查表已启用 | 指纹=92bcedf210e6aed1 桶数=46`（与 soc.yaml 逐字一致），
     **全程无 `clg-pf-fallback`**（capacity-missing / bucket-out-of-range / cluster-target-missing / unknown-cluster）。
  ③ **FDP 动作率基线 2/270 = 0.74%**：`action=fdp` 270 帧 = Skip 268（hysteresis 157 + dstfreq 111）+ Move 2
     （`little->big f_src=1785`）；本包统计为旧脚本产物，dstfreq 111 含老口径高估（下包起 blocked 位组合可分离）。
- **两处探针定案（用户 item 10/11「检查是不是节点写错」）**——**都不是玄学、都非本机字符串写错**：
  ① `handle_cpufreq_transition` 挂载恒失败 = **category/name 写错**（旧结论「内核无 CONFIG_CPU_FREQ_TRACEPOINTS」证伪）：
     原 `("cpufreq","cpufreq_transition")` → 路径不存在；mainline 该 tracepoint 在 **power** 子系统、v4.6 起改名
     `cpu_frequency`。已改 `("handle_cpufreq_transition","power","cpu_frequency")`，待下包复核 `freq_trans` 起计数。
  ② touch-boost 4 节点全 ENOENT = **`/sys/module/cpu_boost/` 整目录不存在**（模块未加载），非字符串写错；
     功能由 ChiRi `on_touch()` 接管、`apply_disable_touch_boost` 无害跳过。真实 input boost 挂载点仍未知（07 T9）。
- **核秒口径评估（用户 item 4，只评估未实现）**：核秒**不能**从 aff `t` 行积分（槽级差分 + 冷长尾降采样 +
  每 30 帧刷新帧放大行数）。最省来源 = `cpu_monitor` 每 tick 已在算的 `CoreState.busy_diff`（ns）；加累计量仅
  **每核每 tick 一次整数加 + 一次存储**（≈50 次加/秒，无新 syscall/文件读）。落地走 **status.csv 末尾追加一列**
  （≈+1 MB/天，守「列只在末尾追加」）；`@S` 逐核行约 13 MB/天、会抵消差分省字，不取。**结论：开销可忽略。**
- **devimp 探针可诊断性两修（用户 item 6「修」）**：① `energy_cost.rs` 的 `act=fdp` blocked 出参由「压成单一
  reason」改为**位组合**（`quota+dstfreq` 等，新增 `nodata`），离线可还原每个约束各拦下多少候选；② `utils.rs`
  新增 `write_nodes_verbose`（每节点 `Ok`/`Err(Some(errno))`/`Err(None)`），`affinity.rs` bg-uclamp 写按**真实
  errno** 记账（原一律 `e0` 导致「半个候选组不存在」被读成 50% 写失败）；③ `dvaff.py` 成功率表加 `miss` 列、
  从分母剔除 `e2/e20`（节点缺失，机型无该节点属常态）。

## 2026-09-28：天花板全拆 + T11 稳态下探 + 帕累托前沿落点（CLG-PF）

- **天花板全拆（硬口径）**：`reduce`/`default` 的 `perf_ceil` 在 8550 / 8475 / 8998 / 根兜底一律 `1.0`；
  特调 akmode `0.95→1.0`；playback `headroom 0.85→1.0` 且三簇 `per_cluster` 天花板整段删除。
  **唯一保留的天花板是 SceneMode（`perf_ceil 0.12`）**——它是唯一允许不到最高频的轴。
  代价：视频/游戏稳态频率抬升、功耗回升；补偿手段 = T11 稳态下探。
  **回退 = 把对应 yaml 的 `perf_ceil` / `headroom` 改回旧值**（字段级可逆，无代码改动）。
- **T11 稳态下探**：CLG 与特调各一组 `steady_decay_*`（缺省 `enabled=true`）。语义 = 写侧偏移的
  **限幅积分控制**，被控量是 util，目标水位 `T = up_threshold × steady_decay_target_ratio(0.90)`；
  偏移只在 flush 里从 `eff_perf` / 写入比例减去，**绝不回写 `current_perf`**（否则与升频分支互相打架 →
  2~4s 锯齿）。特调另有 `up_confirm_ticks`（缺省 2）= 严格升频审批。细节见 `agentsdocs/03-chiri.md`。
- **CLG-PF（仅 8550）**：结果向量表在 `module/config/8550/soc.yaml` 的受控生成块内
  （`scripts/frontier_policy.py --target soc --write`；`--check` 逐字节对账；**块内禁手改、块外逐字保留**）。
  落点不变式 `target_freq = min(ratio_freq, pf_freq)`——**只下调、绝不上抬**，且仅在稳态查表、缺数据回退。
  非 8550 的 soc.yaml 无该段 → `soc_frontier_policy()` 返回 None → 自动走原比例路径。
- **生成器已扩展**：每个需求桶带 `lo_units/hi_units`（真机容量单位）与 `little_khz/big_khz/prime_khz`；
  `model_fingerprint` 移入块内；新增 `--disable`。复算 7 项锚点全过（46 点前沿、起手 3652.8 Mdmips/W、
  prime 冲顶段 599.946）。**指纹仅作日志/离线对账标识，运行期不做硬校验。**
  `per_cluster.<簇>` 还带**逐档簇功耗 `power_w`**（与 `freq_khz` 同下标对齐，生成时断言档位数一致），
  供 FDP 的放置成本模型使用——**表的唯一权威是文档 §3 → 生成器 → soc.yaml → 运行期，严禁手抄进 Rust**。
- **FDP（前沿主导放置内核，2026-09-28 用户明确要求做）**：`src/chiri/energy_cost.rs` 用**双边净收益**
  `net = [P_src(让出后)−P_src(现状)] + [P_dst(承接后)−P_dst(现状)] < 0` 给**后台/批处理 promote** 选目的簇
  （等价于「等算力下放到边际 mW/Mdmips 更低的地方」）。现状取实测（该簇最忙核 util + `scaling_cur_freq`），
  之后按单核落位 + schedutil `f_min + u×(f_max−f_min)`；配 `0.85×最高频` 硬护栏与每轮每簇迁入配额。
  `fdp_enabled` 缺省 false、8550 开 true；**prime 资格仍由 `bg_promote_exclude_prime` 把关**（静态表不含漏电/idle-exit）。
  回退 = `fdp_enabled: false`。

## 本环境约定（Trae）

- 产出落点：AI 文档 → `.trae/docs/`；计划 → 项目内（**任何内容都不得离开项目文件夹**）；记忆 → `.trae/memory/`。
- 子代理：允许并行、数量不设上限；只读调研尽量一次铺开；**写同一文件必须串行**。
- **子代理产出必须逐行复核**：本会话实际发生两次「顺手把整份注释重新缩进」的越界改动
  （`core_ctl` / `cpu_load_governor` / `tuned` / `common` / `fps_monitor` 合计约 250 行纯空白差异），
  会污染 diff。发现后按「只差空白取 HEAD、实质改动取工作区」的方式还原，实质改动逐字保留。
- 用户口径：最高性能发布不得受限；**除已适配帕累托前沿的机型（当前仅 8550）外一律回退原逻辑**；
  **永久禁止新增 A/B 对照实验**（判据只能是逻辑必然 + 模型折算 + 功能回归观察）。
