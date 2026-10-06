# daemon 自耗归因 · 阶段 B/C/D(部分)/E 实施记录（2026-10-07）

> 依据 `.cursor/plans/2026-10-07-daemon-selfcost-implementation-plan.md`（§2 约束 / §5 阶段 B / §6 阶段 C / §7 阶段 D / §8 阶段 E / §10 阶段 F）。
> 本文件是**实施记录**（做过什么、验证状态、回退指引），**不是**计划复述。
> 阶段 A（离线工具修订 + 30 项测试 + 结果报告）见 `.trae/docs/2026-10-07-daemon-selfcost-results.md`，此处不重复。
>
> **总体验证状态：代码层「静态类型检查通过」，测试层「部分独立运行通过」，其余未验证。**
> 本轮 **Android target 未安装、无设备授权**；host `cargo check -p chiri --tests`（`YUMI_SKIP_EBPF=1`）因**两个既存错误**无法完成全量编译——`src/monitor/screen_detect.rs:352` 的 Android 专有 `libc::__system_property_get`（host 编译固有缺失），以及 `src/chiri/mod.rs:213` 已暂存的 FAS 改动 `fas_package_ready` 的 `and_then` 类型不匹配。**这两个错误均非本轮引入**；在本轮全部新增/改动代码与 `#[cfg(test)]` 模块中，编译输出**零 error、零 warning**。
>
> **真正运行过的测试（独立 harness，脱离全量编译）**：
> - `src/chiri/diag_cost.rs`：`rustc --edition=2024 --test --extern libc=...` → **16 passed / 0 failed**，含 `thread_cpu_clock_is_readable_and_monotonic`（在本机真实读 `CLOCK_THREAD_CPUTIME_ID`）、`bucket_compares_upper_bound_at_ns_precision`（ns 精确桶边界）、`absorb_*`（延后 Summary 聚合）、`reset`/`reset_session` 语义、`has_pending`、`summary_line` 字段序。
> - `src/chiri/diag_worker.rs`：用最小 `log`/`crate::logger` shim 编译运行 → **15 passed / 0 failed**，含 `writer_false_marks_lost_and_does_not_advance`（只有写出成功才推进 `last_written`）、`panic_marks_inflight_lost_and_recovers_drained`、`panic_drain_double_panic_counts_each_frame_lost`（二次 panic 逐帧记账不中断）、`submit_after_panic_waits_for_drain_then_returns_err`（先旧后新）、`submit_wait_timeout_still_returns_err_task_intact`、`failed_after_panic_blocks_submit`、`submit_after_channel_closed_returns_err_task`、`fifo_preserves_order_and_drains_on_shutdown`。
> - `src/logger.rs`：新增的 `drain_allowed_requires_existing_dir`、`aff_snapshot_and_drain_blocks_identical_same_ts` **已静态通过类型检查，但未单独运行**（logger 依赖多，无法脱离全量编译）。
> - 阶段 A 的 Python：`python3 -m unittest discover -s scripts/devimp -p 'test_selfcost.py'` → **30 passed**。
>
> **仍未验证**：C1/C2/C3/C4 的字节与决策对拍、selfcost 实际落盘与轮转、@S 采样延迟与 worker 队满回压实测、电池 TTL 行为延迟、真机任何观察。严禁把本文件读作「编译通过 / 收益已兑现」。
>
> **第二轮审查修订（上轮，详见 §7）**：6 条缺陷已按 §7 修复，**状态一律「已修 / 未验证」**——其中 ② 为**部分修复**（worker panic 时正在处理的那一帧可能已部分写出、无法回滚）。`diag_cost.rs` 与 `diag_worker.rs` 的新增测试已用独立 harness **实际运行通过（16 + 15）**；`logger.rs` 新增测试与 `mod.rs` 接线**未运行**。
>
> **第三轮审查修订（上轮，详见 §8）**：3 条缺陷已按 §8 修复，**状态一律「已修 / 未验证」**——① 两处回退统一改用 `logger::aff_snapshot_drain`（无门控、采样时点 ts）；② 在飞帧 `FrameId` 入账 + 排队帧按 drain 返回值分流 `recovered_frames` / `lost_frames`；③ 身份升级为 (包名 / PID / starttime / 配置指纹) **四重隔离**、在**采样时点冻结**。**本轮新增/修改的测试与 `mod.rs` 接线一律「未运行」**；真实运行证据仍只有 `diag_cost.rs` 16 passed、`diag_worker.rs` 15 passed、阶段 A Python 30 passed。

> **第四轮审查修订（本轮，详见 §9）**：3 条缺陷已按 §9 修复，**状态一律「已修 / 未验证」**——① 配置指纹由「整体排序（丢层级、**确定性碰撞**）」改为 `canonical_debug`「保留层级、只排同级子项」；② `SampledFrame` 扩为一次取齐的一致快照（身份 + `generation` + `sequence` + 配置指纹，**先在采样入口冻结**，`dispatch` 不再取代际）；③ 排空改**逐帧 `catch_unwind`**（二次 panic 仅损失当帧记账）+ `submit` 在 `failed` 态**等待排空完成再返回 `Err`**（保证先旧后新）。`diag_worker.rs` harness 已实测 **15 passed**。

## 1 各阶段实际改动清单

### 1.1 阶段 B：原同步路径直接测量

| 文件 | 函数 / 项 | 改动 | 验证 |
|---|---|---|---|
| `src/chiri/diag_cost.rs`（新建） | `CostStats` / `BlockStats` / `Block::{Build,Write,Summary}` | 线程局部成本累计器：每段累计 `count`/`cpu_valid_count`/`wall_ns_sum`/`wall_ns_max`/`cpu_ns_sum`/`slow_count` + 固定直方图 | 未验证 |
| 同上 | `thread_cpu_ns()` | 读 `CLOCK_THREAD_CPUTIME_ID` 绝对读数（`tv_sec*1e9+tv_nsec`）；`clock_gettime` 失败返回 `None`，**不兜底为 0 或进程时间** | 未验证 |
| 同上 | `WALL_BUCKETS_MS = [1,2,5,10,20,40,50,100]` + 超界桶 | 固定墙钟直方图共 9 桶，上界含；`bucket_index` 用整数除法向下取整 | 未验证 |
| 同上 | `SLOW_NS = 50_000_000` | **仅用于节流告警**，全部调用一律入账，不设入账门槛 | 未验证 |
| 同上 | `summary_line()` | 固定字段 schema：`schema/session/window_start/window_end/block/count/cpu_valid_count/wall_ns_sum/wall_ns_max/cpu_ns_sum/wall_buckets/slow_count/clock_errors/write_errors` | 未验证 |
| `src/logger.rs` | `selfcost_write_line` / `selfcost_close` | 写 `devimp/selfcost_<ts>.log`；自愈/轮转/记账同 devimp 口径 | 未验证 |
| `src/logger.rs` | `diag_prune` | 前缀过滤扩展为 `main_`/`aff_`/`selfcost_`（`DEVIMP_KEEP_FILES` 合并计数） | 未验证 |
| `src/chiri/mod.rs` | @S 同步路径 | 加 `before_build`→`after_build`→`after_write`（+ `Summary`）分块计时；每 60s 或会话结束输出 selfcost 摘要 | 未验证 |

### 1.2 阶段 C：确定的实现优化（部分子项）

| 文件 | 函数 / 项 | 改动 | 验证 |
|---|---|---|---|
| `src/logger.rs` | `aff_token` | 返回 `Cow<'_, str>`：无空白/控制字符时借用、需替换时逐字符应用原映射（Unicode/空串/真值 `-`/NaN 占位语义保留）。**借用路径只在「不落盘的省行路径」上真正省下分配**；落盘时 `build_aff_snapshot` 以 `Cow::Owned(...into_owned())` 存入 `th_rows`，**单次构造本身仍会分配** | 未验证 |
| `src/logger.rs` | `main_tick` | 接收**原始数值** `Option<f32>`/`Option<u32>`；签名仍按**格式化后字符串**比较以保序；展示字段延迟到**确定落行后**才格式化 | 未验证 |
| `src/chiri/mod.rs` | `aff_token` 三处调用点 | 线程 comm / `p` 行 / 补位 `p` 行改**惰性构造**；每 PID 首条线程名称兜底改为**显式处理**。**收益仅限「减少需要构造 token 的线程数量」**（每帧每线程构造 → 每 PID 首条 + comm 真变化 + 首见 + 刷新帧），**单次构造本身仍会分配** | 未验证 |
| `src/chiri/mod.rs` | `cpu_freq_snapshot` | policy 发现结果加缓存（非空才缓存、60s 低频重扫失败保留旧缓存、不跨节点读持锁、实际频率值每次照读）。**新增/删除 policy 最多延迟 60s 才被发现**——**不是「零行为变化」**；前置条件：平台拓扑引导期定型 + 该延迟边界已获批准，否则本项受阻 | 未验证 |
| `src/chiri/power_base.rs` | `battery_state`（`BATT_STATUS_TTL = 2s`） | 电池状态读取加 TTL：只缓存**有效值**（`"-"` 不缓存、失败不刷新、`init`/`release` 清缓存）。**改变压频判定行为**：PowerBase 可能按「过期最多一个 TTL 加下一次调用间隔」的充放电状态做压频判定，**不是「零行为变化」**；需**单独授权 + 行为延迟验收**，未获批准则本项不实施 | 未验证 |

> **C1 测试**（空白/控制字符/Unicode/空串/`-`、刷新/差分/首见/首条兜底、优化前后 aff 字节流对拍）**未写、未跑**。
> **C2 测试**（舍入边界/负零/非有限值/reason 变化/时间门槛/policy2/5 同簇独立状态/缓冲溢出）**未写、未跑**。
> **C3 测试**（首次失败恢复/空列表恢复/TTL 边界/重扫失败/增删/排序/节点读取失败、静态拓扑逐字节一致）**未写、未跑**。
> **C4 测试**（放电/充电切换/TTL 边界/读取失败/未知值/退出重入/告警去重）**未写、未跑**。

### 1.3 阶段 D：诊断去阻塞与线程化（部分）

| 文件 | 项 | 改动 | 验证 |
|---|---|---|---|
| `src/chiri/diag_worker.rs`（新建） | `DiagWorker` | 单线程（线程名 `diag_render`）+ 有界 FIFO（`CAPACITY = 2`）诊断渲染/写入 worker | 未验证 |
| 同上 | `FrameTask` / `FrameId{generation, sequence}` | 帧任务含采样时点 `sampled_at`、`rows`、`full`；上一轮增补 `fg_pid` 与 `config_identity`（供代际身份对账，见 §7 ⑥）；**本轮增补**：身份与 ts 在**采样时点冻结**（`SampledFrame`），身份升为 (包名 / PID / starttime / 配置指纹) 四重（见 §8 ③）；并记录**在飞帧 `FrameId`** 供 panic 缺帧账（见 §8 ②）；`@S` 帧格式不变。只做「已渲染行 → 单次原子落盘」，**不访问可变 `AffinityManager`/控制器/sysfs 调度节点** | 未验证 |
| 同上 | `submit` / `backpressure_count` | 队满**阻塞**交付（保真优先回压，不覆盖、不丢帧）；回压次数记 `AtomicU64` | 未验证 |
| 同上 | `shutdown` | 排空已提交任务后退出、幂等（重复调用不 panic、不二次 join） | 未验证 |
| 同上 | 启用状态 | **默认不启用**——只在调用方 `start()` 时创建线程，`mod.rs` 不自动启动 | 未验证 |
| `src/logger.rs` | `aff_block_write` | 转发 `aff_write_block`，供 worker 单写者调用 | 未验证 |
| `src/chiri/mod.rs` | 模块注册 | 注册 `diag_cost` / `diag_worker` | 未验证 |

> **D5 验收**（FIFO 满队列/顺序/双 policy/前台切换/诊断开关/配置代际/partial write/worker 失败/退出排空的确定性测试，原与新 aff 字节对拍，真机采样延迟观察）**未写、未跑**。
> **D1 采样/差分点、D2 失败切回同步、D3 生命周期与共享资源**在代码层面已落（见上表），但与 `mod.rs` 的接线完整性与运行期行为**未验证**。
> **D1 启用前置（本轮改：四重代际隔离，仍未验证）**：`bump_generation()` 原只在诊断会话开关（假→真）时递增；上一轮改为**包名 + 前台 PID 任一变化即递增，配置热重载也递增**；**本轮升级为 (包名 / 进程 starttime / 配置指纹) 参与、PID 变化即递增**——身份为 **(包名, pid, starttime, 配置指纹) 四重**，且**在采样时点冻结**（`SampledFrame`），**不在 build 之后读**（见 §8 ③）。`FrameTask` 沿用 `fg_pid`/`config_identity` 供身份对账（`@S` 帧格式不变）。**「已修」不等于「已验证」**：代际递增、四重身份的接线完整性与运行期行为**未验证**，D1 仍保持默认关闭、**不得声称可用**。

### 1.4 阶段 E：归因驱动的非 FAS 热点优化

- **未产生新的热点任务**：无经证据确认的非 FAS 热点被立项为编号任务，**无对应代码改动**。
- 依据计划 §8：未知热点不预先命名代码修改；热点实现属证据驱动的条件任务，待 A/B 归因给出候选后再在本计划追加编号任务。

### 1.5 阶段 F：统一验收（未执行）

- 对拍（aff 字节流 / `tick` 签名 / 落行）、Android 目标检查、真机自然观察、文档收口与回退**均未执行**。

## 2 验证状态表

| 项 | 状态 | 原因 |
|---|---|---|
| 全部新增/改动代码可编译 | **静态通过** | `YUMI_SKIP_EBPF=1 cargo check -p chiri --tests`：本轮全部改动零 error / 零 warning；全量编译被两个**既存**错误阻断（`screen_detect.rs:352` 的 Android 专有 `libc::__system_property_get`；`mod.rs:213` 的 `fas_package_ready` `and_then` 类型不匹配） |
| `diag_cost.rs` 聚焦单测（桶边界/分母/尾窗/重置/错误路径） | **已运行通过** | 独立 harness：`rustc --test --extern libc=...` → 16 passed / 0 failed（含真实 `CLOCK_THREAD_CPUTIME_ID` 读取与 ns 精确桶边界） |
| `diag_worker.rs` 聚焦单测（FIFO/序号/代际/停机/回压计数） | **已运行通过** | 独立 harness（`log` 与 `crate::logger` shim）→ 15 passed / 0 failed（含写入失败不推进 `last_written`、panic 排空回退、失败态阻止 submit） |
| C1/C2/C3/C4 对拍与边界测试 | **未验证** | 需全量编译（被两个既存错误阻断） |
| aff 完整字节流 / `tick` 签名与落行对拍（优化前后一致） | **未验证** | 未编译、无对拍程序运行 |
| @S 采样延迟与保真、worker 队满回压实测 | **未验证** | 无设备授权，未做真机观察 |
| 第二轮修订 ① worker 关闭排空（`aff_snapshot_drain`）/ ② panic 排空回退与部分写（`lost_frames`/`last_lost`） | **测试已运行通过 / 端到端未验证** | `diag_worker.rs` harness **15 passed**（第四轮后计数）含 `writer_false_marks_lost_and_does_not_advance`、`panic_marks_inflight_lost_and_recovers_drained`、`failed_after_panic_blocks_submit`；真实落盘/设备侧未验；② 仍属**部分修复**（正在处理的那帧可能部分写入） |
| 第二轮修订 ④ 直方图桶边界 ns 精确比较 / ⑤ Summary 窗口归属（`pending_summary`） | **④ 测试已运行通过 / ⑤ 未运行** | `diag_cost.rs` harness 16 passed 含 `bucket_compares_upper_bound_at_ns_precision` 与 `absorb_*`；⑤ 的窗口对账需全量编译与运行期数据，未做 |
| 第二轮修订 ⑥ worker 代际三重隔离（包名 / PID / 配置身份） | **未验证** | 接线完整性与运行期行为未验；见 §7 |
| 第三轮修订 ① 两处回退改用 `aff_snapshot_drain`（无门控 + 采样时点 ts） | **未运行 / 未验证** | 仅本次代码审查确认改动，无对应 harness 运行；见 §8 ① |
| 第三轮修订 ② 在飞帧 `FrameId` 入账 + 排队帧按 drain 返回值分 `recovered_frames`/`lost_frames` | **未运行 / 未验证** | 本轮新增测试与接线**均未运行**；见 §8 ② |
| 第三轮修订 ③ 四重身份隔离（包名 / PID / starttime / 配置指纹）+ 采样时点冻结 | **未验证** | 接线完整性与运行期行为未验；配置指纹实现见另一代理汇报，本轮未复核；见 §8 ③ |
| selfcost 摘要零 IO（未测量时）/ 轮转 / 保留计数 | **未验证** | 未运行 |
| 电池 TTL 感知延迟边界（持续调用下 TTL + 下一调用等待） | **未验证** | 未真机确认 |

## 3 与计划 §2 约束的符合性自查

| §2 约束 | 本轮实现是否遵守 | 说明 |
|---|---|---|
| 不降采样质量（不调大 `aff_secs`、不关 devimp、不暂停 clamp、不摘遥测） | **是（设计层）** | 未改 `aff_secs`、前台全量、长尾降采样、`full=1`、存活集合、TID 复用语义；selfcost 属**新增**观测 |
| 不新增采集开关对照实验 | **是** | 未引入任何 A/B 开关；selfcost 由测量期调用方在诊断会话内产生 |
| 所有测量调用进入汇总，不移出慢帧 | **是（设计层）** | 每次 `record` 一律入账；`SLOW_NS` 只增 `slow_count` |
| `≥50ms` 仅用于节流告警 | **是** | `SLOW_NS` 仅影响告警计数，不入成本账、不作入账门槛 |
| CPU 区间不重叠 | **是（设计层）** | `Build`/`Write`/`Summary` 段端点相接不重叠；外层总区间不作可加项 |
| 负残差不截 0 | **是（设计层）** | CPU 读取失败记 `None`（不计分母），**不兜底 0**；读端 `cpu_valid_count=0 → unavailable` |
| 墙钟不从进程 CPU 中相减 | **是（设计层）** | `wall_ns`/`cpu_ns` 两账独立，不做 `/proc/self/stat` 减法 |
| 不改现有 CSV 列序 / `full=1` / 存活集合 / TID 复用 | **是** | 未触碰；selfcost 为独立文件 |
| 不修改 FAS 行为、控制周期、温控/频率/亲和参数 | **是** | 未改 |

> 自查结论均为**设计层符合性**，**不是运行期证据**——因未编译、未测试，全部仍需在编译/测试/设备阶段复核。

## 4 未实施 / 受阻项

| 项 | 状态 | 原因 |
|---|---|---|
| **C4 感知延迟批准** | **未取得（条件项）** | 计划 §6 要求先取得「允许压频判定感知延迟」的明确批准；未批准则本项不实施。本轮仅按要求实现 2s TTL 并标注延迟边界，**未取得批准** |
| **C3 拓扑发现延迟边界** | 未单独批准 | 计划 §6：静态拓扑稳定依据与 60s 延迟发现边界未获批准时本项保持受阻 |
| **D4 整体采样迁移** | **未做（条件项）** | 要求 D1～D3 通过后再评估，且必须先固定原始基线、单独评审保真；当前保留 D1 同步采样 + 异步渲染/写入方案 |
| **worker 启用** | **默认关闭** | `DiagWorker` 不自动启动，等待 D5 验收与接线完整验证 |
| **D1 诊断代际隔离** | **已修 / 未验证（启用前置）** | 上一轮由「仅前台包名」改为**包名 / PID / 配置身份三重隔离**；**本轮升级为 (包名 / PID / starttime / 配置指纹) 四重隔离、在采样时点冻结**（见 §8 ③）；接线完整性与运行期行为**未验证**，D1 仍**默认关闭**、不得声称可用 |
| **本地编译 / 测试授权** | 未恢复 | Android target 未安装、无设备授权；host `cargo check` 因两个既存错误无法全量编译（`screen_detect.rs` 的 `libc::__system_property_get`；`src/chiri/mod.rs:213` 的 `fas_package_ready` `and_then` 类型不匹配） |
| **阶段 E 热点任务** | 未产生 | 无经证据确认的非 FAS 热点；不预先命名代码修改 |
| **阶段 F 统一验收 / 回退** | 未执行 | 未取得编译/测试/设备条件 |

## 5 明确的「不可声称」清单

- **不承诺**任何 CPU 降幅、功耗降幅、占用百分比收益。
- **不声称**编译通过、测试通过、对拍通过。
- **不声称**收益已兑现、优化已交付、采样质量已验证（不降质仅为**设计层**主张）。
- **不声称** worker 已在生产启用（**默认关闭**），不声称异步迁移已验收；D1 代际隔离本轮升级为 **(包名 / PID / starttime / 配置指纹) 四重隔离、在采样时点冻结**，但**接线完整性与运行期行为未验证**，仍不声称可用。
- **不声称 worker 故障零丢帧**：panic 时在飞帧无法回滚（计入 `lost_frames`、标 partial 风险），排队帧按 drain 返回值分流——写出成功记 `recovered_frames`、失败才记 `lost_frames`；仍须按「诊断故障 + 可能部分写入」记录（见 §7、§8 ②）。
- **不声称两处回退走的是「同步路径」**：已接收帧的唯一正确出口是 `aff_snapshot_drain`（无门控、采样时点 ts），`aff_snapshot` 只用于默认同步路径（写出时刻 ts、受门控）——二者不可混称（见 §8 ①）。
- **不声称两条路径的帧 `ts` 相同**：默认同步路径帧头 `ts` = 写出时刻，异步 worker 路径 `ts` = 采样时点，是**有意的时间语义迁移**，仅在 `DIAG_WORKER_ENABLED` 启用后生效（见 §7）。
- **不声称** C1 为「零分配快路径」——它只减少**需构造 token 的线程数**，单次构造与落盘 `into_owned` 仍分配。
- **不声称** C3/C4 为「零行为变化」——C3 有 ≤60s 发现延迟、C4 改变压频判定行为，均需前置批准与行为延迟验收。
- **不把「计划完整」读作「优化完成」**（计划 §10）：本文件仅记录改动与状态，未给完成结论。
- 阶段 A 已有的量级提示（P1≈26 / P3≈29 / P2≈86 ms/s）**仍为跨场景推导**，不因本轮实现而变成测量基线或验收目标。

## 6 回退指引

回退只撤销本专项改动，**保留其它用户改动，不强制 git 回退、不自动提交**。按计划 §10 分项撤销：

| 范围 | 撤销对象 | 备注 |
|---|---|---|
| **C 各子项** | `src/logger.rs` 的 `aff_token`→`Cow<'_, str>` 与 `main_tick` 原始数值签名；`src/chiri/mod.rs` 的 `aff_token` 惰性构造、`cpu_freq_snapshot` policy 缓存；`src/chiri/power_base.rs` 的 `BATT_STATUS_TTL`/`battery_state` | 逐子项独立撤销；C1/C2/C3/C4 互不依赖 |
| **D 采样/渲染边界** | `src/chiri/diag_worker.rs`（整个新建模块）；`src/logger.rs` 的 `aff_block_write`、`aff_snapshot_drain` 与 `recovered_frames` 查询接口；`src/chiri/mod.rs` 的 `diag_worker` 注册与交接（含两处回退调用点） | worker 本就默认关闭；撤销后回到同步渲染/写入，采样口径不变。**注意**：`aff_snapshot_drain` 是「已接收帧的唯一正确出口」（无门控、采样时点 ts），撤销它即让两处回退退回原 `aff_snapshot`——那是**受门控且用写时 ts 的已知缺陷**（见 §8 ①），不要当作等价替换 |
| **B 测量（如需整体撤）** | `src/chiri/diag_cost.rs`（整个新建模块）；`src/logger.rs` 的 `selfcost_write_line`/`selfcost_close` 与 `diag_prune` 前缀扩展；`src/chiri/mod.rs` 的 @S 分块计时与 selfcost 输出 | 撤销后不再产生 `selfcost_*.log`；**原采样口径不受影响** |
| **E 热点** | 本轮**无** E 热点改动，无需回退 | — |
| **文档** | `agentsdocs/02-convention.md`（selfcost 条）、`agentsdocs/05-maint.md`（新记录口径）、`agentsdocs/07-todos.md`（专项进展） | 仅追加/修订段落，按需改回 |

**回退要点**：① 撤销 `diag_prune` 前缀扩展时注意保留 `main_`/`aff_` 过滤；② 撤销 `aff_token` 借回路时要同步处理三处调用点；③ worker 与 `diag_cost` 均为**新建独立模块**，删除即回退，不牵动其它代码；④ 不因回退改动 FAS 及其它用户改动。

## 7 第二轮审查缺陷修复（上轮，全部「已修 / 未验证」）

> 第二轮代码审查确认 6 条缺陷，上轮按下表修复。**所有条目仅记「已修」，不是「已验证」**：可编译性、字节对拍、运行期行为一律未取得证据（同上「验证状态」）。`diag_cost.rs`（16 passed）与 `diag_worker.rs`（第四轮后 15 passed）已用独立 harness 运行通过；`logger.rs` 新增测试与 `mod.rs` 接线**未运行**。

| # | 缺陷（旧实现） | 修复（现实现） | 状态 |
|---|---|---|---|
| ① | worker 关闭排空丢帧：`aff_snapshot_with_ts` 仍查 `diag_active()`，诊断关闭后才 shutdown worker，队列里已采集的帧被门控丢弃；且 worker 无条件推进 `last_written`，把未写出的帧标记为已写出 | 新增 `logger::aff_snapshot_drain(rows, full, ts) -> bool`（**不受诊断开关门控**，只要求 devimp 目录已存在，返回是否真的写出成功）；worker 只在写入成功时推进 `last_written`，失败累加 `lost_frames` 并记 `last_lost` | **已修 / 未验证** |
| ② | worker panic 丢帧：`catch_unwind` 只记一条 error，队列残留与后续提交无处置 | panic 后置 `failed` 标志（此后 `submit` 一律 `Err(task)` 交还所有权），并借 `&rx` 在 panic 返回后**排空队列中已入队**的帧、逐条回退到同步路径；`lost_frames`/`last_lost` 可查。**边界**：panic 发生时正在处理的那一帧可能**已部分写出**、无法回滚 | **部分修复 / 未验证** |
| ③ | 帧 ts 时间语义：上一轮把采样时点 ts 也应用到了默认同步路径（build 之前取 ts），改变既有配对口径 | **同步路径回到原语义（写出时刻 ts，字节不变）**；**异步 worker 路径用采样时点 ts**。两者不同是**有意的时间语义迁移**，**只在 `DIAG_WORKER_ENABLED` 启用后生效** | **已修 / 未验证** |
| ④ | selfcost 直方图桶边界：先把 `wall_ns` 向下取整成 ms 再比上界，`1.999ms→≤1ms 桶`、`100.999ms→≤100ms 桶`，分位数桶范围**系统性低估**；旧测试还固化了该错误行为 | `bucket_index` 改为**按纳秒精确比较** `wall_ns <= upper_ms * 1_000_000`（上界含），不做 ms 向下取整 | **已修 / 未验证** |
| ⑤ | Summary 成本归属窗口：摘要写入发生在旧窗口结算期间，却在 `reset()` 后入账 → 下一窗口报出的 Summary 实际发生在该窗口起点之前，按窗口对账错位 | Summary 成本连同**其实际发生窗口**一起留存（`pending_summary`），下一次 flush 用**该窗口边界**报出；会话结束的尾窗按其自身边界补写 | **已修 / 未验证** |
| ⑥ | worker 代际未覆盖完整身份：只比较前台包名 | 代际判定改为**包名 + 前台 PID 任一变化**即递增，**配置热重载**也递增；`FrameTask` 增补 `fg_pid` 与 `config_identity` 用于身份对账（`@S` 帧格式不变） | **已修 / 未验证** |

### 7.1 两个「门」的口径（本轮明确）

- **「允许接收新采样」**（诊断开关门控 `diag_active()`）：同步路径 `aff_snapshot` / `aff_snapshot_with_ts` 经此门；关闭时不接收新采样。
- **「允许排空已接收任务」**（排空路径 `aff_snapshot_drain`）：**不受诊断开关约束**，只要求 `devimp/` 目录已存在（关闭态不留痕、不建目录）；用于把「诊断开启时合法采到、关闭后才轮到写出」的队列帧写出，不因关门丢弃。
- 结论：**「允许接收新采样」与「允许排空已接收任务」是两个不同的门**；排空路径不受诊断开关约束。

### 7.2 时间语义迁移（显式格式口径变更）

- 默认（`DIAG_WORKER_ENABLED=false`）：帧头 `@S ts=` = **写出时刻**（`filename_ts`），字节与既有实现一致。
- 启用（`true`）：帧头 `@S ts=` = **采样时点**（`aff_frame_ts`）；延迟写出时 ts 记采样时刻而非写出时刻。
- **不得声称两条路径 ts 相同**，也不得声称「默认同步输出时间语义不变」之外的东西——本迁移是**有意**的，且**仅在 worker 启用后生效**，启用即改变 `@S` 与 status.csv 的时序配对口径。

### 7.3 worker 故障记录口径

- **不得声称「worker 故障零丢帧」**：panic 时队列内已入队帧可排空回退到同步路径，但**正在处理的帧可能部分写出**，无法回滚。
- worker 故障须按「**诊断故障 + 可能部分写入**」记录；`lost_frames`/`last_lost`（排空失败）与 `backpressure_count` 一并报告。

## 8 第三轮审查缺陷修复（本轮，全部「已修 / 未验证」）

> 第三轮代码审查确认 3 条缺陷，本轮按下表修复。**所有条目仅记「已修」，不是「已验证」**；可编译性、字节对拍、运行期行为一律未取得证据。**本轮新增/修改的测试与 `mod.rs` 接线一律「未运行」**（真实运行证据仍只有 `diag_cost.rs` 16 passed、`diag_worker.rs` 15 passed、阶段 A Python 30 passed）。

| # | 缺陷（旧实现） | 修复（现实现） | 状态 |
|---|---|---|---|
| ① | panic 回退丢帧且改时间戳：worker 排空与 `mod.rs` 提交失败回退都调 `logger::aff_snapshot(rows, full)`——它**受 `diag_active()` 门控**（诊断关闭时丢帧）且用**写出时刻** ts（破坏采样时点语义） | 两处回退统一改用 `logger::aff_snapshot_drain(rows, full, ts)`：**不受门控、用采样时点 ts、返回是否写出成功** | **已修 / 未验证** |
| ② | panic 在飞帧未计入缺帧账、且分不清「回退成功」与「真丢」：panic 时正在处理的帧随栈展开销毁，只留泛化 error、无 `FrameId`、不更新 `lost_frames`；排队帧无论回退成功与否都计为丢失 | 新增**在飞帧 `FrameId`** 记录；panic 分支先取在飞帧 `FrameId` 计入 `lost_frames` + `last_lost` 并标注 **partial 风险**；排队帧按 drain 返回值分流——成功记 `recovered_frames`（**新增查询接口**），失败才记 `lost_frames` | **已修 / 未验证** |
| ③ | 身份隔离不完整：只比较 `(包名, pid)`，识别不了**同包同 PID 复用**；配置身份只含 8 个 meta 字段，热重载可能不改变代际；`FrameTask` 的 pid/配置身份在 **build 之后**读取，可能与采样时点不一致 | 身份升级为 **(包名, pid, 进程 starttime)** 三元组（starttime 读 `/proc/<pid>/stat` 第 22 字段）；配置身份扩为**全量配置指纹**（覆盖全量配置的稳定指纹，具体实现见 `diag_config_identity` doc）；身份与 ts 一起在**采样时点冻结**（`SampledFrame`） | **已修 / 未验证** |

### 8.1 回退出口口径（本轮明确）

- **已接收帧的唯一正确出口是 `aff_snapshot_drain`**：无 `diag_active()` 门控、用**采样时点 ts**、返回是否写出成功。worker panic 排空与 `mod.rs` 提交失败回退**都走它**。
- `aff_snapshot`（及 `aff_snapshot_with_ts`）**只用于默认同步路径**：写出时刻 ts、受 `diag_active()` 门控。
- **不要把 `aff_snapshot` 与 `aff_snapshot_drain` 混称「同步路径」**——前者是「接收新采样」门之后的默认出口，后者是「已接收帧」的排空出口；两者门控与 ts 语义都不同（承接 §7.1 的「两个门」）。

### 8.2 缺帧账判定边界（本轮明确）

- `lost_frames` = **真丢 / 无法回滚**：① panic 时正在处理的**在飞帧**（无来源回滚、按 partial 风险记录），② 排队帧排空时 drain **返回 false**（写出失败）。
- `recovered_frames`（**本轮新增查询接口**）= 排队帧排空时 drain **返回 true**（写出成功回退）。
- 二者**互斥**：同一排队帧只会进 `recovered_frames` 或 `lost_frames` 之一（按 drain 返回值分流）。
- **仍不得声称「worker 故障零丢帧」**：在飞帧无法回滚，须按「诊断故障 + 可能部分写入」记录（承接 §7.3）。

### 8.3 身份与时间戳在采样时点冻结（本轮明确）

- 身份为 **(包名, pid, starttime, 配置指纹)** 四重：pid 之外增 **进程 starttime**（读 `/proc/<pid>/stat` 第 22 字段）以识别**同包同 PID 复用**；配置身份由 8 个 meta 字段扩为**全量配置指纹**（覆盖全量配置的稳定指纹，实现见 `diag_config_identity` doc，本轮未复核其内部实现）。
- 身份与 ts **一起在采样时点冻结**（`SampledFrame`），**不在 build 之后读**——避免 build 之后进程/配置已变导致身份与采样时点不一致。
- D1 启用前置随之更新为**四重隔离**（见 §1.3 / §4）；**仍为「已修 / 未验证」，默认关闭、不得声称可用**。

## 9 第四轮审查缺陷修复（本轮，全部「已修 / 未验证」）

> 第四轮代码审查确认 3 条缺陷，本轮修复。**所有条目仅记「已修」，不是「已验证」**。**本轮新增/修改的测试与 `mod.rs` 接线一律「未运行」**；可写的真实运行证据只有 `diag_cost.rs` 独立 harness **16 passed**、`diag_worker.rs` 独立 harness **15 passed**、阶段 A Python `test_selfcost.py` **30 passed**。

| # | 缺陷（旧实现） | 修复（现实现） | 状态 |
|---|---|---|---|
| ① | **配置指纹确定性碰撞**：`diag_config_identity` 把 pretty-Debug 输出**整体排序**，丢掉字段所属层级 → 交换两个 profile 的参数值，只要行的多重集相同，指纹**完全不变**，实际调度配置变化却不递增代际（是**确定性**碰撞，不是「极低概率」） | 新增纯函数 `canonical_debug`：按缩进还原层级树，**只对同一父节点下的直接子项排序**（消除 `HashMap` 迭代序），父节点自身行与层级关系保持不变；`diag_config_identity = canonical_debug(&format!("{cfg:#?}"))` | **已修 / 未验证** |
| ② | **采样身份与 generation 未一起冻结**：前台身份在循环中判定（用于 bump），采样时又重读 PID/starttime，最后 `dispatch` 才读 `generation` → 两次读取间切换前台会出现**「新 PID 配旧 generation」**；配置热重载分开更新「指纹 / 配置 / generation」，build 期间重载会产生**「旧配置指纹配新 generation」** | `SampledFrame` 扩为**一次取齐的一致快照**（`ts` / `fg_pid` / `fg_starttime` / `config_identity` / `generation` / `sequence`）：同一 tick 内**先**「身份变化 → bump 代际」，**后**构造快照并同段读取 `generation`/`config_identity`；`dispatch` **不再**调用 `w.generation()`/`w.next_sequence()`，直接用冻结值组 `FrameId` | **已修 / 未验证** |
| ③ | **panic 排空二次 panic + 写入乱序**：(a) 排空 `write(&task)` 位于 `catch_unwind` **之外**，写入器再次 panic 会销毁剩余队列且**未逐帧记账**；(b) `failed=true` 后调度线程新帧**立即**同步回退，而 worker 仍在排空旧帧，文件锁只保证**单帧原子** → 新差分帧可能先于旧帧落盘，**破坏离线按序重建** | (a) 排空改为**逐帧** `catch_unwind`：`Ok(true)→recovered`、`Ok(false)/Err(_)→lost+last_lost`，二次 panic **只损失当帧记账、不中断排空**；(b) 新增 `drain_finished` 标志，worker 排空完成后置真；`submit` 在 `failed` 态**先等待排空完成再返回 `Err(task)`**（有界等待，超时记 error 并仍需返回 `Err`），使「旧帧先落盘、新帧后同步写」成立 | **已修 / 未验证** |

### 9.1 配置指纹的层级保留（本轮明确）

- **旧实现的缺陷是结构性的**：整体排序后，指纹只保留「行的多重集」，**不保留字段属于哪个父节点**。因此「把 A profile 的 `x` 与 B profile 的 `y` 互换」可能得出**完全相同**的指纹 —— 这是**确定性碰撞**，不能描述为「极低概率」。
- 现实现**保留层级**：只排序**同级兄弟**，父子关系与每个节点的自身行不变；收尾花括号等同文本叶子节点无可观测交换，不引入新碰撞面。
- 仍然**不含版本信息**（`Config` 无版本字段、仅 derive `Debug`）：若日后 `Config` 补 `Serialize`，应改用 `serde_yaml::to_string` 的稳定序列化（TODO）。

### 9.2 采样快照的解释边界（本轮明确）

- 快照保证的是「**采样时点那一刻**身份 / 代际 / 配置指纹彼此一致」；采样到 `dispatch` 之间若前台再次切换，整帧仍可能略滞后于**真实**当前前台 —— 这是设计取舍，不是缺陷。
- 「身份段先于采样段」这一顺序由代码位置保证，**只能静态审查**，没有运行时断言。
- `sequence` 在采样时点分配（与采样顺序一致）；被丢弃的帧会留下**序号空洞**，离线按 `generation + sequence` 判序。

### 9.3 panic 排空的口径（本轮明确）

- **二次 panic 不再造成「无声丢失」**：每一帧独立 `catch_unwind`，无论 `Ok(false)` 还是 `Err(_)` 都计入 `lost_frames` + `last_lost` 并记 error。
- **「先旧后新」由 `submit` 等待排空完成保证**，**不是**由文件锁保证（锁只保证单帧原子写入）。等待**有界**：超时后仍返回 `Err(task)`，但必须记 error 声明「顺序可能被破坏」。
- 仍**不得声称 worker 故障零丢帧**：在飞帧无法回滚。
