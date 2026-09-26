# SoC 硬编码参数只读审计（2026-09-26）

范围：`src/`（含 `src/scheduler/fas/`、`src/chiri/`）。只读审计，未改任何代码。

背景：项目支持多 SoC（8550/8475/8998），SoC 差异应走 `module/config/{soc}/` 外挂配置（编译期嵌入）。
已知项**不重复报告**：`common.rs::chiri_core_ranges()` 核心组区间硬编码，正在改为读 soc.yaml。

判级口径：
- **外挂** = 随 SoC 变化且改动可能发生；
- **保留** = 全 SoC 统一、纯算法常数、安全白名单、或外挂需同步改逻辑；
- **已配置化** = 实际来源是 feature.yaml / tuned_profiles.yaml 等，只是注释/默认值留在 .rs。

## 分类表

| 文件:行号 | 参数 | 当前值 | 随 SoC 变化？ | 判级 | 理由 | 若外挂建议 |
|---|---|---|---|---|---|---|
| `src/utils.rs:238-241` | CPU 温度 thermal_zone type 匹配名单 | `"soc_max"` / `"mtktscpu"` / `"cpu-1-"` / `"cpu-0-0-usr"` | 是（soc_max=高通平台、mtktscpu/cpu-0-0-usr=MTK、cpu-1-=其他，新平台可能全不命中） | **外挂** | 这是全文件唯一真正的 per-SoC 探测名单；`find_cpu_temp_path` 找不到节点 = CLG 热保护与 FAS 温度护栏的 CPU 温度参考整体失效，新增 SoC 必须改 .rs | feature.yaml Thermal 段新增 `cpu_temp_zone_types: [子串列表]`（子串匹配语义保持）；per-SoC 覆盖放 `config/{soc}/feature.yaml` |
| `src/chiri/gpu.rs:18-20` | GPU sysfs 根路径常量 | `/sys/class/devfreq`、`/sys/class/kgsl/kgsl-3d0`、`.../max_gpu_clk` | 厂商级（高通 Adreno vs MTK vs Mali），非 per-SoC | 保留 | 探测是通用的：devfreq 目录按 gpu/kgsl/mali 名字扫描 + kgsl 存在性守卫 + devfreq 兜底；kgsl-3d0 路径跨高通 SoC 稳定。GPU 节点「各 SoC 不同」的部分（频率档）本来就运行时读 available_frequencies | 无需外挂；若未来出现第 3 类 GPU 厂商再议 `gpu_devfreq_hints` |
| `src/chiri/gpu.rs:140` | devfreq 频率单位启发式 | `v > 10_000_000` 视为 Hz 除 1000 | 否（单位差异非 SoC 差异） | 保留 | 纯单位换算兜底，读数失败自动跳过 | — |
| `src/monitor/telemetry.rs:267-268` | GPU 利用率候选节点 | `kgsl-3d0/gpu_busy_percentage`、`/sys/kernel/ged/hal/gpu_loading` | 厂商级（QCOM/MTK 各一条） | 保留 | 与 gpu.rs 同类：按存在性取首个可读者，仅遥测用途、失败自动失效；跨厂商兜底清单不是 per-SoC 参数 | 无需外挂 |
| `src/monitor/telemetry.rs:212` | OPlus 私有电池节点 | `/sys/class/oplus_chg/battery/bcc_parms` | 否（OPlus 厂商级，字段下标 6/8/11 随双电芯布局分支） | 保留 | 存在性守卫 + 回退标准 power_supply 节点；双电芯分支由 `oplus_dual_cell` 配置控制，不依赖 SoC | 无需外挂 |
| `src/utils.rs:397-416` | SysPathExist 特性探测路径 | qcom/mtk perfmgr、`hi6220-ufs/ufs_clk_gate_disable`、`/sys/block/sda/queue/scheduler`、cpuidle governor | 厂商/机型级存在性探测 | 保留 | 全部是 `path_exists` 探测，用于启用/停用对应功能，缺失即优雅降级；不承载调参数值 | 无需外挂 |
| `src/common.rs:101` | `CHIRI_SOC_HINTS` 探测链 | `["8550","8475","8998"]` + soc0/machine、getprop 5 源 | 是（每加一个 SoC 要追加） | 保留 | 探测链与配置目录选择器一体：**外挂自身需要先知道命中哪个 SoC**，鸡生蛋；新增 SoC 本来就是一次发版（带新 config/{soc}/），顺手追加 hint 不构成负担 | 保持现状；`matched_soc_hint()` 的返回值继续作为 soc.yaml 的 key |
| `src/main.rs:447-448` | 常规采样间隔 | ChiRi 160ms / 非 ChiRi 200ms | 否（按模式而非按 SoC 分） | 保留 | 160ms 是 ChiRi 模式节奏，与 CLG dwell（2 tick）、MODE_FILE_REWRITE 5s、遥测 1s 等多处逻辑互相咬合；按 SoC 拆分需全链路同步改 | 若未来确需 per-SoC，应作为 soc.yaml `topology.sampling_ms` 单一来源下发，禁止在 main.rs 写分支 |
| `src/chiri/power_base.rs:26,29,32` | `REWRITE_INTERVAL`(5s) / `PERF_DEADBAND`(0.05) / `MAX_DOWN_STEP`(0.25) | 常量 | 否 | 保留 | 防篡改重写节奏与振荡抑制算法常数；真正随 SoC/机型变化的 PowerBase 项（`target_power_w`、`up_headroom_below`、`down_scale`）已全部在 `PowerBaseConfig`（feature.yaml，`config.rs:1117-1185`，含 clamp） | — |
| `src/chiri/mod.rs:1186` / `src/chiri/fast.rs:16` | MODE_FILE / fast_lock 防篡改重写 | 5s | 否 | 保留 | 与 power_base 同一防篡改口径，事件驱动约定统一值 | — |
| `src/chiri/affinity.rs:91` | `BG_UCLAMP_REASSERT` | 60s | 否 | 保留 | 值未变不写 + 强制再断言间隔（约定层硬规则本身的示例值） | — |
| `src/chiri/affinity.rs:149-209` | 重钉/升降簇阈值族 | `REBALANCE_INTERVAL` 2s、`OVERLOAD_MARGIN` 0.15、`RETURN_COOLDOWN` 16s、`REPIN_DEBOUNCE` 8s、`MIN_MIGRATE_INTERVAL` 4s、`CORE_OVERLOAD_UTIL` 0.70、`PINNED_WEIGHT` 0.4、`MAX_PINS_PER_CORE` 3、`PROMOTE_UTIL_PCT` 25、`LITTLE_HIGH_WATER` 0.70、`BIG_HIGH_WATER` 0.90、`KEY_BIND_RELEASE_WATER` 0.50、`FG_BUSY_UTIL_PCT` 30、`FG_BUSY_RELEASE_UTIL_PCT` 15、`DEMOTE_UTIL_PCT` 5、`DEMOTE_STREAK` 3、`THREAD_STALE` 30s | 弱相关 | 保留 | 均为单核/单线程粒度的算法阈值（滞回、防乒乓、两窗防抖），不随簇数/簇布局结构性变化；组归属由 `chiri_core_ranges()`（已知项）提供。`MAX_PINS_PER_CORE` 注释提「4 个性能核」是 8550 实测背景，常量本身 per-core、SoC 无关 | — |
| `src/chiri/affinity.rs:85` | `BG_UCLAMP_MAX_CODE` | `u64::MAX` | 否 | 保留 | 语义哨兵值（清除 uclamp 限制），不是调参 | — |
| `src/chiri/scheduler.rs:133-142` | `SCHED_ALLOWED_PARAMS` 白名单 | 7 个 `/proc/sys/kernel/` 键 | 否 | **保留（确认）** | 安全白名单，防止 feature.yaml 注入任意内核参数写；值本身来自 feature.yaml Sched 段，白名单只管「允许写哪些键」 | — |
| `src/scheduler/fas/` 各文件 | FAS 算法常数 | `fps_window.rs:5 WINDOW_SIZE`=120 帧、`policy_controller.rs:13 VERIFY_SETTLE`=200ms | 否 | 保留 | 帧窗/验证等待，纯算法 | — |
| `src/chiri/mod.rs:13-47` | 主循环节奏常数 | `CLG_STALE_MAX` 5s、`EVENT_POLL_MS` 1s、`THERMAL_CHECK_INTERVAL` 2s、`TELEMETRY_LOG_INTERVAL` 1s、scenemode 三常数、`TUNED/FAS_COOLDOWN` 300s、`THERMAL_UNPRESS_STEP` 0.15 | 否 | 保留 | 事件循环 deadline 取 min 体系的统一节奏，跨 SoC 一致 | — |
| `src/chiri/mod.rs:272-281` | GPU 快照开关 | `GPU_SNAPSHOT_ENABLED=false`、间隔 10s | 否 | 保留 | 行为开关（8550 实测唤醒 GPU +47% 功耗才默认关），不是 SoC 参数；开启逻辑本来就按存在性探测 | — |
| `src/chiri/tuned.rs:11` / `fas_manager.rs:33` | `MIGRATION_COST_PATH` | `/proc/sys/kernel/sched_migration_cost_ns` | 否 | 保留 | 内核 ABI 稳定路径；**写入值本身已配置化**（见误报澄清） | — |
| `src/chiri/governor.rs:17` | `FALLBACK_GOVERNOR` | `"schedutil"` | 否 | 保留 | 内核通用 governor 名，非 SoC 参数 | — |
| `src/rhine.rs`、`src/down.rs` | 实验室/降级模式 | — | 否 | 保留 | rhine 模式定义全部来自嵌入的 rhine-init.yaml，代码只有状态机 | — |
| `src/chiri/config.rs:916-920` 等 | Thermal 默认值（41/45/75/85°C、cap、hyst） | serde default 函数 | 弱相关 | 已配置化 | `ThermalGuardConfig` 全字段可被 feature.yaml Thermal 段覆盖，.rs 里只是缺省兜底（且带 clamp/一致性校验） | — |
| `src/chiri/config.rs:1034-1066` | uclamp 上限默认（85 等） | serde default | 弱相关 | 已配置化 | `top_app_uclamp_max_pct` / `background_uclamp_max_pct` 来自 feature.yaml Affinity 段 | — |
| `src/chiri/config.rs:684` / `fas_types.rs:234` | `migration_cost_ns` | `Option<u64>` 默认 None | 弱相关 | 已配置化 | FAS/tuned 接管时的写入值全部来自配置（tuned_profiles / FAS rules），代码无 400000 之类数值；未配置即不动该节点 | — |
| `src/chiri/config.rs:204` 起 | CLG 全部参数（rate limit ticks、EMA α、死区、dwell、per_cluster 覆盖…） | 结构体 | 弱相关 | 已配置化 | `CpuLoadGovernorConfig` + `per_cluster` 按 little/big/prime 分簇覆盖，簇归属经 `chiri_core_ranges()`；8550 数值在 feature.yaml，不在 .rs | — |
| `src/chiri/tuned.rs`（全文） | tuned 参数组 | — | 弱相关 | 已配置化 | 除 `MIGRATION_COST_PATH` 外无常量，参数组按模式名从 `Config.tuned_profiles` 分派 | — |

## 建议外挂项汇总

仅 1 项强建议：

1. **`src/utils.rs:238-241` CPU 温度 thermal_zone type 名单** → feature.yaml `Thermal` 段新增
   `cpu_temp_zone_types`（子串列表，保持现有 `contains` 匹配语义，缺省回退现名单）；
   per-SoC 覆盖放 `config/{soc}/feature.yaml`。理由：名单混合高通/MTK 各自的 zone 命名，
   新 SoC（尤其 MTK 新命名）不命中即失去 CPU 温度参考，是真实会发生的改动点。

2 项可选（改动可能性低，建议先不动）：

- `src/chiri/gpu.rs:18-20` + `src/monitor/telemetry.rs:267-268`：GPU 节点路径按厂商枚举。
  当前「devfreq 名字扫描 + kgsl/ged 存在性探测」已是通用方案，只有出现第三类 GPU 厂商
  （新 private 节点）时才值得外挂为 `gpu_busy_nodes` 候选清单。
- `src/main.rs:447-448` 采样间隔：目前按模式不按 SoC 分；若未来要 per-SoC 化，必须同时
  改 CLG dwell/看门狗等「tick」语义的换算，属于逻辑级改动，不宜简单外挂一个数。

## 误报澄清（查过、确认不是硬编码问题的项）

后续审计请勿重复上报以下内容：

1. **`migration_cost_ns` 默认 400000**：src/ 中不存在该数值（全文 grep 无 400000/400_000）。
   写入值是 `Option<u64>`，来自 tuned_profiles.yaml / FAS rules；`.rs` 里只有内核路径字符串。
2. **affinity 的 85 uclamp、热保护 41/45/75/85°C**：均为 feature.yaml 可覆盖的 serde 默认值
   （`config.rs` d_aff_* / d_batt_* / d_thermal_*），不是硬编码。
3. **CLG / tuned / power_base 的调参数值**：全部已配置化（feature.yaml 的 CLG/PowerBase 段、
   tuned_profiles.yaml 的参数组），`.rs` 内只剩 `PERF_DEADBAND`/`MAX_DOWN_STEP`/
   `REWRITE_INTERVAL` 这三个纯算法/防篡改常数。
4. **电池温度 0.1°C 单位问题**：已改为运行时预识别（`BatteryTempScale`，utils.rs:269 起），
   代码注释里的「8550」只是事故背景，不是分支逻辑。
5. **CLG 簇探测**：policy/affected_cpus/available_frequencies 均运行时扫 sysfs（policy{} 模板），
   无簇数或簇 ID 硬编码；簇归类经 `chiri_core_ranges()`（已知项，不在本报告重复）。
6. **SCHED_ALLOWED_PARAMS**：预期保留，确认为安全白名单（键名单，不含值）。
7. **大量 `/sys/devices/system/cpu/cpufreq/policy{}` 路径**：通用模板路径，全 SoC 一致，非硬编码参数。
8. **rhine / down 模块**：模式定义来自嵌入 yaml，代码不含 SoC 数值。
9. **CPU 温度探测函数里的 E0532 修复注释**（utils.rs:236）：历史注释，与 SoC 无关。
