## [todos] 未竟事项台账（2026-09-26 汇总）

> 历史计划/日志归档后的**唯一 TODO 权威**。开工前先查本文件；完成一项删一项。2026-09-17 审查提出的旧项标注「先核实」——动手前先确认现行代码是否已修。
>
> **口径变更（2026-09-28）**：R3「**永不再新增 A/B 测试**」生效（详见 `04-hard-lessons.md` 的 `[hard]` 首条）。下列以 A/B 为手段的条目**不再按 A/B 执行**，改为「离线重放 + 同版本观察」：[C3] 构建剖面、真机 A/B 整体、T7 uclamp.max=85 / T8 clamp_heavy 恒钳、telemetry gpu_busy 采集成本——逐条待裁决去留，其中涉及"采集开关对照"的直接作废。

### 性能与探针（perf-report backlog）

- **本轮进展（2026-10-09，代码已落、验证待定；不销旧专项）**：
  - `normal_busy`/`normal_press` 真实 `EINVAL` 按 starttime + 目标 CPU 列表退避 4/8/16/32s 封顶，成功/目标/身份变化清除，释放与 reload 失效。TODO: 同版本真机观察失败重试与恢复；两处真实 reload 入口清缓存，不在 2s apply 清。
  - 仅 restricted `cpu.uclamp.max` 真实 `ENOENT` 负缓存 600s，目标值与 max 恢复不破缓存；background 60s 守卫不变，到期可绕过同值 guard，reload 清。TODO: 构造性自检与自然观察到期、节点恢复及 reload 行为，不做开关实验。
  - `diag_build_cost.rs` 新增 `selfcost-build/1 parent=build` 四类子块 + residual，失败调用也 count，父账不变、父子不可相加；60s/关闭尾窗复用输出，会话独占，窗口 reset 留 errors、session reset 全清。TODO: 核对有效分母、异常非假零及观察者成本，保持 worker 默认关、不降采样、不摘 K3。独立审查完成，无剩余阻断发现；编译/运行验证仍受阻，待主线补验证结果与同版本真机观察，静态审查不等于测试通过；未测得省电收益。
  - **验证限制（主线回传）**：Android `cargo check` 已尝试，仅输出 `Compiling chiri`、无 `Checking chiri`，因 `build.rs` eBPF 的 `-Z` 仅 nightly 允许而当前 stable 失败；纯模块 `rustc --test` 因项目内 TMPDIR 只读失败，测试未执行。主线已将两真实模块加入 `scripts/tests/rust_regression.rs` 持久入口，尚未重跑；`ReadLints totalFiles=0` 不构成有效验证。`mod.rs` 单测 `aff_cost_record` 缺第 7 参已补 `None` 并经同一独立审查员复核有效；独立审查完成，无剩余阻断发现，编译/运行验证仍受阻。
  - **用户提供的 1009 汇总（非本轮复算）**：build CPU 3221s / daemon CPU 8889s（36.2%），94,922 次调用 / 3842s 墙钟，真实慢帧 19.2%；仅作归因背景，不当作本轮改动收益。

- [K3] 摘除纯遥测探针需 meta 开关：wakeups/migrations 被 WebUI 消费（status.csv 第 19/20 列 wakeups/migrations + devimp snap + telemetry-summary），属数据面变更——需显式开关 + 关时写 `-` + WebUI null 展示同步。
- [C3] 构建剖面（opt-level z/s/3 + thin LTO，比体积与 8550 实测功耗）——原 A/B 手段作废，待裁决：改同版本观察或直接作废。
- [E3] logdr events buffer（liblog 协议 150-250 行 + SELinux 可达性验证）；[E4] pid_watcher 并入 app_detect（时延 500ms→1.5s 取舍待拍板）。
  - **[E1] 内容比对短路已定案不做（2026-10-01）**：`app_detect.rs` 显式注明「刻意不做内容未变则复用」——冷启动期间 pid 先入组、exec 后 cmdline 才可读，缓存会漏检这类前台切换。
- 自测量基线：**status.csv 末尾两列 `daemon_utime_ms`/`daemon_stime_ms` 已落地（2026-10-01，1s 采样读 `/proc/self/stat`）**；`cargo bench` 三纯函数仍未做（需先定哪三个 + 新建 benches/ 基础设施）。
  - **首包实测（`devimpbin/1005-021253`，A01-06 / 8550 / 24.7 h）**：daemon 自身 **5.36% 单核（亮屏）/ 4.63%（息屏）/ 6.96%（screen_prop=4）**，五段合计 3381 s CPU 时间。大头是 `@S` 帧的前台线程逐帧 stat 下钻（帧头 `nfg` 峰值 1036）。
  - **采样节奏配置已落地（2026-10-05）**：meta `devimp_aff_secs`（缺省 1，clamp 1..=60）+ 息屏固定倍率 `AFF_SNAP_OFFSCREEN_FACTOR`=5。**TODO: 同版本自然观察与构造性自检**——保持现有诊断开关与 `aff_secs`，核对 selfcost 的计数、有效分母与窗口边界；不主动关闭诊断、不调档做开关对照，不承诺占用或功耗降幅。
  - **功耗分解两列已落地（2026-10-05）**：status.csv 末尾追加 `cpu_dyn_w` / `resid_w`（`energy_cost::cpu_dynamic_power_w`），离线分解走 `scripts/devimp/dvenergy.py`。**TODO: 标定复核**——用 8550 下个包看 `[4]` 回归的截距 a（外围地板，亮屏期望 1.5~2.5 W）与 R²；a 明显偏离或 R² < 0.3 说明能效表与真机量纲对不上，需回 `mdocs/8550/sm8550-freq-power.md` §3 复核电压来源。
  - **「核秒」口径开销评估（2026-10-01，只评估未实现）**：核秒**不能**从 aff 的 `t` 行积分——`t` 行是槽级差分 +
    冷长尾降采样 + 每 30 帧刷新帧（`full=1`）放大行数，缺失行 = 未变，只能证明采样状态占比、不是核秒。
    最省的来源是 `cpu_monitor` 每 tick **已在算**的 `CoreState.busy_diff`（ns）：加一个累计量只需**每核每 tick
    一次整数加 + 一次存储**（160 ms × 8 核 ≈ 50 次加/秒，**无新 syscall、无新文件读**）。持久化二选一：
    ① status.csv 末尾**追加一列累计核秒**（≈ +1 MB/天，推荐，遵守「列只在末尾追加」惯例）；② 在 `@S` 帧加逐核行
    （≈ 150 B/s ≈ 13 MB/天，会部分抵消差分省字设计）。**结论：开销可忽略，真要落地走 ① 追加列**。
- **【新增 2026-10-07，立项】daemon 自耗「基线归因」专项**（分析依据 `.codebuddy/docs/2026-10-07-daemon-perf-四项查证与优化方案.md` §8/§10.1/§12，第一层工具 `scripts/devimp/dvselfcost.py`，三个子命令 `feas` / `diag` / `pair`）。
  **目标**：把「daemon 自耗总量 减 @S 帧成本」剩下的那部分（P1≈26 / P3≈29 / P2≈86 ms/s）查成明细：哪些路径执行多少次、每次成本多少、哪些仍未归因。**交付是明细表，不是承诺降到某个占用**。
  **为什么现在做**：30~40ms/帧 是息屏进程 CPU 差分的配对估计，不是调度线程墙钟计时；亮屏侧未获验证，跳帧对照的负差不能单独证明选择偏差或原因。剩余量依赖跨场景减法，只作待验证提示，不用于优化收益承诺。
  **执行边界（2026-10-07 修订）**：先校正离线工具并筛候选，不以设备补采作为启动前提。严格分块归因与线程化迁移前需直接测量 @S 的墙钟与线程 CPU；所有调用纳入汇总，≥50ms 只用于节流告警，不能作为入账门槛。不为本专项调大 `aff_secs`、主动关闭 devimp 或暂停现有采集。只有显式状态证据的自然关闭窗口才可作为诊断关闭样本；main 日志覆盖不能证明开关状态，没有证据就报告未知。源码测量、运行分析、编译测试和设备采集分别待授权，本轮仅修订计划。
  **分两层**：第一层纯离线（用现有日志 + 自耗差分，按同版本/配置/屏幕态/调度模式/连续有效区间分组，**排除跨缺口与切换边界的差分**，不拿包均值做机型归因；注意落行次数≠执行次数，CLG/tuned tick 行有签名去重）。第二层只对少数候选做直接归因（汇总计数 + 分块线程 CPU 时间与墙钟耗时，**汇总输出、不逐事件刷日志、不减少原有采集**）。
  **已知限制**：亮屏跳帧配对不能直接作因果成本；跨场景减法不作严格分解；`/proc/self/stat` 不作调度线程专属账（它是整个进程的 CPU，也不覆盖 eBPF 系统事件路径成本）。旧脚本报告的 P3 52.2%、P2 20.9%、P1 5.7% 是未覆盖秒数的初算，需校正跨午夜、文件/会话边界、覆盖分母及 screen_on/screen_prop 分层后再引用，不直接称丢行率。缺口原因及缺失是否随机均未知。
  **顺序**：本专项**先于** §1.3-B② 快照诊断线程化（后者实施前必须先固定原始基线，否则迁移后归因口径会混）。本专项全程遵守「不降采样质量」红线（2026-10-07 裁决），第二层属**新增**采集而非裁减。
  **实施计划**：唯一有效计划为 `.cursor/plans/2026-10-07-daemon-selfcost-implementation-plan.md`（合并版）；原 `.codebuddy/plans/2026-10-07-基线归因-实施计划.md` 已标记废弃，仅保留历史，原计划的降采样对照及阈值计时要求不执行。Cursor 修订版已合并迁出 docs，不保留重复有效计划。
  - **进展（2026-10-07，阶段 A/B/C/D(部分)/E）——未销账**：
    - **阶段 A 已完成**（离线工具修订 + 测试 + 结果报告）：`scripts/devimp/dvselfcost.py`、`scripts/devimp/test_selfcost.py`（30 项通过）、`.trae/docs/2026-10-07-daemon-selfcost-inputs.json`、`.trae/docs/2026-10-07-daemon-selfcost-results.md`。总量口径可复算（旧 53.6/119.0/44.0 ↔ 新 strict 53.7/118.9/43.9 ms/s）；全仓**无诊断关闭对照窗口**（`confirmed_off` 恒 0），剩余量「是否非诊断」仍不可判定。
    - **阶段 B/C/D(部分)/E 代码已落，全部「未验证」**：`src/chiri/diag_cost.rs`（新建：线程局部成本累计器 `Block::{Build,Write,Summary}`、`thread_cpu_ns`（`CLOCK_THREAD_CPUTIME_ID`，失败 `None` 不兜底）、固定墙钟直方图 9 桶、`SLOW_NS=50ms` 仅告警、`summary_line` 固定字段）、`src/chiri/diag_worker.rs`（新建：单线程 + 有界 FIFO 容量 2、`FrameId{generation,sequence}`、队满阻塞回压、停机排空幂等、**默认不启用**）、`src/logger.rs`（`aff_token`→`Cow<'_, str>`——**借用只在「不落盘的省行路径」上省分配**，落盘 `build_aff_snapshot` 仍 `into_owned` 分配、单次构造仍分配，收益仅限「减少需构造 token 的线程数」；`main_tick` 收原始数值 `Option<f32>`/`Option<u32>` 仍按格式化字符串比较保序；`selfcost_write_line`/`selfcost_close`；`diag_prune` 前缀扩展 `main_`/`aff_`/`selfcost_`；`aff_block_write`）、`src/chiri/mod.rs`（注册两模块、`aff_token` 三处惰性构造、`cpu_freq_snapshot` policy 发现缓存——**新增/删除 policy 最多延迟 60s 才发现、非零行为变化、需前置批准**、@S build/write 分块计时每 60s/会话末出 selfcost 摘要）、`src/chiri/power_base.rs`（电池状态 2s TTL，只缓存有效值）。
    - **阶段 E 未产生新的热点任务**（非 FAS 热点无代码级优化落地，保持待证据立项，见 §8 规则）；**阶段 F 未执行**（统一验收、对拍、真机观察、回退未做）。
    - **未验证项（待编译/单测/对拍/设备观察）**：上述全部代码的可编译性、聚焦单测、aff 字节流与 `tick` 签名/落行对拍、@S 采样延迟与保真、worker 默认关不生效、selfcost 摘要零 IO 与轮转/保留计数。
    - **受阻项与原因**：本地编译测试授权未恢复（Android target 未安装、无设备授权；host `cargo check` 因**两个既存错误**无法全量编译——`screen_detect.rs` 的 Android 专有 `libc::__system_property_get`、`src/chiri/mod.rs:213` 已暂存的 FAS 改动 `fas_package_ready` 的 `and_then` 类型不匹配，本轮**未通过任何编译或测试**，保持「未验证」）；C4 电池感知延迟批准未取得（本项按要求仅实现 TTL、标注延迟边界；**改变压频判定行为，需单独授权 + 行为延迟验收，未批准则不实施**）；C3 拓扑发现延迟边界未单独批准（**新增/删除 policy 最多延迟 60s 才发现，非零行为变化，平台拓扑引导期定型为前置**）；**D1 worker 启用前置（本轮改：四重代际隔离，未验证）**（代际身份由「仅前台包名」逐轮升级至 **(包名 / PID / 进程 starttime / 配置指纹) 四重**——starttime 读 `/proc/<pid>/stat` 第 22 字段识别同包同 PID 复用，配置身份为覆盖全量配置的稳定指纹（见 `diag_config_identity` doc）；身份与 ts **在采样时点冻结**（`SampledFrame`）、**不在 build 之后读**；`FrameTask` 沿用 `fg_pid`/`config_identity`、`@S` 帧格式不变；接线完整性与运行期行为**未验证**，D1 仍保持默认关闭、不得声称可用）；D4 整体采样迁移未做（要求先固定原始基线并单独评审保真，D1 同步采样 + 异步渲染/写入方案保留）；worker 默认关闭。
    - **不可声称**：不承诺 CPU/功耗降幅、不称编译通过、不称收益兑现。详见 `.trae/docs/2026-10-07-selfcost-phase-bcf-results.md`。
    - **第二轮审查修订（2026-10-07，全部「已修 / 未验证」，新增测试未运行）**：① worker 关闭排空丢帧 → 新增 `logger::aff_snapshot_drain`（**不受诊断开关门控**、只要求 `devimp/` 目录已存在、返回是否真写出成功），worker 仅在写出成功时推进 `last_written`，失败累加 `lost_frames`/`last_lost`；② worker panic → 置 `failed` 标志（此后 `submit` 一律 `Err(task)` 交还所有权）+ 借 `&rx` 排空已入队帧回退同步路径，**但正在处理的那帧可能已部分写出、无法回滚**（**不得声称零丢帧**）；③ ts 语义 → 同步路径回到**写出时刻 ts**（字节不变）、异步 worker 路径用**采样时点 ts**（**显式时间语义迁移，仅 `DIAG_WORKER_ENABLED` 启用后生效**，不得声称两路 ts 相同）；④ 直方图桶边界 → `wall_ns <= 上界*1_000_000` **ns 精确比较、上界含**、不做 ms 向下取整（旧实现系统性低估已修）；⑤ Summary 归属窗口 → Summary 成本带其**实际发生窗口**（`pending_summary`）留存、下次 flush 用该窗口边界报出（`window_*` 是实际发生区间，非报告时当前窗口）；⑥ 代际身份 → 三重隔离（见上条 D1；本轮再升为四重）。**未运行**：本轮全部新增测试（可写真实运行证据仅 `diag_cost.rs` 16 passed、`diag_worker.rs` 15 passed、阶段 A Python 30 passed）。读法口径见 05「selfcost 读法补充」。
    - **第三轮审查修订（2026-10-07，全部「已修 / 未验证」，本轮新增/修改测试与 `mod.rs` 接线未运行）**：① **回退出口**——worker panic 排空与 `mod.rs` 提交失败回退此前都调 `logger::aff_snapshot(rows, full)`（**受 `diag_active()` 门控**、用**写出时刻** ts），现统一改用 `aff_snapshot_drain(rows, full, ts)`（**无门控、采样时点 ts、返回是否写出成功**）；**已接收帧的唯一正确出口是 `aff_snapshot_drain`**，`aff_snapshot` 只用于默认同步路径——**两者不可混称「同步路径」**。② **缺帧账**——新增**在飞帧 `FrameId`** 记录：panic 分支先取在飞帧 `FrameId` 计入 `lost_frames` + `last_lost` 并标 **partial 风险**；排队帧按 drain 返回值分流——成功记 `recovered_frames`（**新增查询接口**）、失败才记 `lost_frames`（`lost_frames` = 真丢/无法回滚，`recovered_frames` = 排空写出成功，二者**互斥**；**仍不得声称零丢帧**）。③ **身份隔离升为四重**——**(包名 / PID / starttime / 配置指纹)**，身份与 ts 在**采样时点冻结**（`SampledFrame`）、**不在 build 之后读**（见上条 D1）。**未运行**：本轮新增/修改测试与 `mod.rs` 接线；真实运行证据仍只有 `diag_cost.rs` 16 passed、`diag_worker.rs` 15 passed、阶段 A Python 30 passed。
- 真机 A/B 整体未做（perf 16 项优化 + K1/K2 eBPF 同场景 status.csv 逐核 util 对比）；K1/K2 CI 构建确认（本机 stub 编译盲区，「CI 通过才算落地」）。
- **已定案不做（勿重评）**：U3 cpu_monitor 侧、A5b、panic=abort、换 hasher/parking_lot、热路径值缓存、事件化否决四项（cgroup.events poll / PSI / thermal uevent / timerfd 全量合并）、换语言与双进程拆分。
- **【P0，2026-10-03 logd_1003 四包实证】8650 tick 去重 key 冲突（`devimpbin/1003-213457`，8650/PJX110/A01-04）**：`8650/soc.yaml:9-12` 把两组 A720 并进 `big`，但它们是 policy2 与 policy5 两个 cpufreq policy；`logger.rs:789` 的 `MAIN_TICK_STATE` 以**簇名**为 key → 两 policy 共用表项互相刷新签名 → 去重完全失效。实测 tick 密度 per 簇每秒：default big 11.96 vs little 5.84；**playback big 50.03（= 决策周期 40ms 的 25/s × 2）vs little 6.67**；big 占全包 tick 行 79%（284.7 万 / 360.6 万）。对照 Pixel(同版本) 与 8550(20000) 三簇均衡。附带伤害：日志体积约翻倍 → 更快撞 128MB/日志预算 → 该包 28h 内 9 次归档重启（每次把热状态重置回 cap=100）。
  - **已修（2026-10-04）**：`MAIN_TICK_STATE` 改为两级 `policy_id → cluster 名 → (签名, 时刻)`（热路径仍零新分配，内层用 `&str` 查）；`main_tick` 新增 `policy_id: i32` 首参，CLG 侧传 `cluster.policy_id`、tuned 侧 `ClusterState` 新增 `policy_id` 字段（构造处取 `pid`），`tuned.rs` 的 `main_rows` 元组同步带上它。**待下包复核**：8650 上 tick 行密度是否降到与 little/prime 同档（playback 预期 50 → 6~10 行/簇/秒），以及包体与归档重启次数是否回落。**旧包判读**：2026-10-04 之前的包仍带这个 2× 因子，跨机型比 tick 行数/日志体积前必须除掉。
- **【记录 2026-10-04】tuned up 分支丢弃 `gated_write` 返回值的开销评估**：进 up 分支的前提是 `target_max > current_max + hyst_freq`，写成功后 `current_max` 前移到 `target_max`，同一 target 不再满足该条件 → **不存在「每 tick 重复写同一 target」的浪费**（2026-10-03 报告里这句话是错的，撤回）。真正会连续重试的只有两种：写失败（`last_failed`）与被 dwell 翻摆延迟，两者都在 `gated_write` 里先判定、延迟路径在 syscall 之前就 return false。加 `if` 的成本 ≈ ns 级且 up 侧**没有任何状态迁移依赖返回值**（down 侧才有：`down_since` 写成功才清），故按「if 更小则改」执行下来等于加一个空操作分支 —— **结论：不改**，保持丢弃返回值。
- **【定案 2026-10-04】压制判定的温度源只取电池**：`mod.rs` 的 cap 分支已删掉 CPU 阶梯（2026-10-03 前实现为两路各自判档后取较小值），`config.rs` 的 `[hyst_invariant]` 随之只对电池阶梯取 room；CPU 温度继续落到 snap / status.csv / `thermal_change` 事件文本，性质是**遥测**，`feature.yaml` 的 `cpu_*_temp_c` 对 cap 无效。由此 `devimpbin/1003-214440`（Pixel/zumapro）`cpu_temp` 列全文 `-`、`thermal_cap_pct` 73161 行全 100 属**预期结果**——电池最高 40℃ 未到 zumapro 软限 41℃（2026-10-03 曾把它列为 P0「CPU 温度源缺失导致热保护失效」，按本口径撤回）。遗留两条低优先：**①** `scripts/devimp/dvcommon.py` 的 `avg()/pct()` 空样本返回 0.0，报告会渲染成「某列恒 0.0」，回原始列确认是不是 `-`；**②** `clg-thermal-no-sensor` 仍是 `debug!`，CPU zone 缺失在运行期不可见，现在只影响遥测列完整度。**T2（thermal zone 全清单 + cpu_temp 口径）随之降级**：CPU zone 只决定遥测列有没有值。
- **【P1，2026-10-03 实证】CLG 升降频数量悬殊（棘轮），三 SoC 同状**：up 类 : down 类 = 25:1（8650 `com.tencent.mm`）、22:1（8650 `default -`）、6:1（8550 incallui/minimap）、5:1（Pixel dragon.read）；`deb_up` 最大连续 522。根因在源码侧（`cpu_load_governor.rs`）：① 簇内取 max(util) `:205-211,:410`；② 升侧渐近逼近使 `current_perf` 恒低于 target → 每 tick 都进 up 分支并清零 `down_wait`（`:522,:537`）；③ down 后 `current_perf` 塞回 ≤ target 立刻重建 up（`:569`）→ 「降一档回半档」无驻留带；④ up 分支不清零自己的 `up_wait`（`:530-545`）→ deb_up 失去确认进度语义；⑤ 触摸 floor 免确认即时改写 `current_perf`（`:584-591,:653-655`），8650 上 policy2/5 双 big 同时被抬；⑥ 唯一回落机制 `steady_decay` 触摸期停用、`util ≥ 0.88` 即清零、上限仅 0.20。**「降后 N tick 禁止再抬」已否决（2026-10-04 用户裁决：连续加压场景必须能立刻再抬）**，不再往这个方向做；up 分支清零 `up_wait` 一并搁置。注意阈值本身并不对称（default 档 `down_rate_limit_ticks=2` < `up_rate_limit_ticks=3`），不要往「调阈值」方向改。
- **【暂缓，2026-10-04 用户裁决】FPS 探针 `perf_event_open` 失败 217 次**（214440 124 / 220345 85 / 213457 16）：同 app 多进程交替占坑 → 直方图实际只覆盖当前挂住的进程。候选修法（退避 + 失败计数落 status.csv）**先不动，只留记录**；判读时记住 `[7]` 段长尾率可能被漏采抹平，别读成「该时段无卡顿」。
- **【已复核 2026-10-05，结案】8650 tick 去重 key 修复生效**（上一项的「待下包复核」，样本 `devimpbin/1004-233049`，8650 / PJX110 / **A01-04 + A01-06 混装** / 20.5 h / 13 批次）。分簇 tick 密度（行/簇/秒，bili playback）：A01-04 批次 big **29.35 / 40.92 / 40.0**（1003-194056 / 213109 / 233042，little 3.7~5.2）→ A01-06 批次 big **2.98 / 2.75**（1004-153222 / 223902，little 0.8）。修复前基线（1003-213457）big 50.03。**降幅约一个数量级，确认生效**。残余 big:little ≈ 3.6:1 属结构预期（8650 的 big 含 policy2 与 policy5 两个 policy，各计一次），不再动。**判读提醒**：本包前 7 批仍是 A01-04（10-04 10:06:34 起才是 A01-06），跨版本段的 tick 行数与包体不可直接比。
- **【新增，同上包】`batt_power_w` 尖峰读数**：放电段出现量级不可能的 max（lolm 25.48 W / mm 33.25 / mark.via 28.82），与 1005-021253 记录的「2 行 31.48 / 25.93 W」同源。**已落地（2026-10-05）**：`dvpower.py` 新增 `PW_SANITY_MAX_W = 20.0`，超限行计入「异常读数」单独计数、不进均值/p50/p95/max/Wh，并在 `[3]` 后打出剔除行数与最大值；摘要 dict 增 `pw_bad`/`pw_bad_max`。本包实测剔除 5 行（最大 33.25 W），lolm 的 max 由 25.48 → 16.61 W、微信 33.25 → 16.01 W，均值与 Wh 不变。

### 内核取证（详见 06-kernel.md）

- **T1** DT 反编译（capacity 出处 / OPP 电压 / trip / cooling-map / idle 厂商改动全实证）——最大单点缺口；**T2** thermal zone 全清单 + cpu_temp 两路定案；**T3** cpuidle state 全量抓取；**T4** 真热压时段 dcvsh/lmh 采样（常规负载已排除）；**T5** msm_performance / Horae 活跃性（部分定案：msmp 不可读、horae=1）；**T6** ftrace（trace_sched_task_util / trace_dcvsh_freq）；**T7** uclamp.max=85 A/B → P1-5 tg 层钳制立项与否；**T8** clamp_heavy 恒钳 A/B（cap85 2.78 vs 3.18W 基线）；**T9** input boost 实际挂载点（**2026-10-01 已定案：非字符串写错，是节点本体不存在**——见 `06-kernel.md` 真机实测档案）；**T10** 热压场景 FAS verify 行为。
- 树外 oplus_cpu 专项（frame_boost / eas_opt / OMRG / cpufreq_health 的 uclamp/亲和写入路径）。
- 次级未确认：OMRG/kalama 启用与否、msm_lmh_dcvs/lmh.c 是否 probe、厂商 defconfig cdev 清单、`sched_lpm_disallowed_time` 定义文件、WALT busy-hold 对应 waltgov 行号、cpuset 收紧被 WALT 恢复宽掩码的实测。

### 调度 / 机制验证

- **热档中档解除点修复（`hysteresis_mid_c`，2026-09-28 落地）——已实测生效（2026-10-01 结案）**：
  原中档与软档共用 `hysteresis_c=3` → 中档解除点 `43−3=40°C` **低于软限跳闸 41°C**，中档一咬住即跳过
  软档、须一路冷过 40°C 才恢复。8550 取 `hysteresis_mid_c: 1.5` → 解除点 `41.5°C`（> 软限 41）。
  **本包实证**（`devimpbin/1001-190148` `power.txt [5][6]`）：cap=60 温度带 **min 41℃**（0928 为 40，
  含 2556 s 落 40~41 桶）、驻留 **1251 s**（0928 5152 s），解除后直接交软档 cap=85（不再跨档跳过）。
  正文与 normalize 不变量见 `03-chiri.md` 四级判定条。
- ~~**息屏离核策略改为「下线 little」的真机验证（2026-09-28 落地，最高优先）**~~ → **2026-10-01 已结案（结论：不成立，已按修法3 改为不下线）**。结论来自 `logd_1001-045147`（A08-08/8550/PHB110/A16，9 批次、息屏 7.4 h）：**26 次进入 scenemode，20 次（77%）在 11~414 s 内被 `scenemode 持续顶满性能上限（little util 100%）→ 退回 reduce + 300 s 冷却` 撤销**，有效驻留 ≈3.6 h、另有 ≈1.7 h 处于禁入冷却（≈20×300 s）→ 下线 little 并没消除抖动，只是把瓶颈从「3 个小核」搬到了唯一在线的引导核 CPU0（判据 ①② 均命中，② 的「预期仍会触发」被证实，但对象是 little 引导核而非 big）。
  处置：`CoreCtl.scenemode_offline` 该字段此前是**死字段**（`mod.rs` 未接线，下线由 `enabled` 无条件驱动），已接线为真门控并在 8550 置 **false**（不下线任何核、保留全核，仅靠 `perf_ceil` 0.12 + uclamp 压制）。**待下包复核**：① 进入 scenemode 后是否仍出 saturation 撤销（目标：显著减少）；② 息屏段 `batt_power_w` 走向（全核在线的漏电 vs 免冷却的净收益）；③ 亮屏唤醒/电源键响应与 `corectl` 恢复是否干净（此时 core_ctl 不再介入 scenemode，`max_cpus`/`min_cpus` 应保持快照值不动）。
  其余子项（⑥⑦⑧⑨⑩）随「不下线」直接失效，无需再验；**软回退** = 8550/feature.yaml 的 `CoreCtl.scenemode_offline` 改回 true。
- **息屏期 little 被顶满 100%（2026-09-28 包实锤）**：**2026-10-01 更新**——本包给出定量结论（见上条结案），根因确认为「下线 little → CPU0 单核承压」；`logd_0928-170828` 里 7 次撤销属同一机制。以下原始取证线索保留备用：
  由 `ranges.prime` 改为 `ranges.little`（引导核 CPU0 无法热拔出，8550 实际下线 CPU1-2），core_ctl `max_cpus`
  写 **1**（`scenemode_keep_cpus()`，**不是 0**——0 是 walt_halt_cpus 整簇停摆语义，对含引导核的簇写 0 会把
  CPU0 一起停摆），big / prime 改常驻低频，调度服务独占核由「编号最大 little」改为「编号最大 big」。
  判据（同版本长时观察，不做对照）：① `status.csv` 息屏段 `batt_power_w` 与 `dvpower [5][6]` 的档位/温度带
  ——**大核最低频能效低于小核，息屏底噪可能不降反升**；② 是否仍出 `scheduler-scene-mode-saturation`
  （判据取 little∪big 每核 util 最大值，换常驻核不影响它生效，**预期仍会触发**，只是对象从 little 变 big）；
  ③ `corectl-reassert-offline` / `corectl-vendor-override` 是否刷屏（= core_ctl 把核拉回、在拉锯）；
  ④ 亮屏唤醒/电源键响应延迟、`corectl-scenemode-off` 恢复是否干净（含 max_cpus 快照回写）；
  ⑤ 独占大核后业务少 1 颗 big 的影响（亮屏恢复时应释放）。**软备份**：把 `scenemode_targets()` 换回
  `ranges.prime`（core_ctl 路径会自动回到 halt 语义）。
  ⑥（独立审查后补的护栏，需真机确认其是否触发）真机确认 little 簇快照 `min_cpus` 是否 >1——>1 时实现会先把它
  压到 `keep`(=1)、退出再按快照写回；日志出现 `corectl-reserved-core-halted` 说明该内核把保留档也当停摆档
  （引导核掉线），必须放弃 core_ctl 路径只走逐核 offline。
  ⑦（元审查后补）`min_cpus` 的压低/写回是**独立记账**（`min_cpus_lowered`），不受 `max_cpus` 是否写回成功影响；
  真机看退出后 `min_cpus` 是否回到快照（`cat .../core_ctl/min_cpus`）——若长期停在 1，说明写回一直被内核拒
  （`max_cpus` 尚未解除或节点被锁），日志会持续 `corectl-write-failed`（debug 级）。
  ⑧（元审查后如实记录，非缺陷）进程在 scenemode 中途被杀时，被独占大核的 **cpuset 排除会残留在磁盘**上，
  而该排除是 ChiRi 用快照恢复的、快照随进程丢失 → 能否自愈**依赖系统 CpusetManager 重写组 cpus**（代码注释
  既有观察）与「之后进入 boost 时覆盖写回」，ChiRi 自身不保证；真机可 `cat /dev/cpuset/top-app/cpus` 复核
  编号最大的大核是否在列。
  ⑨（第二轮审查发现并已修的 **P0**）目标簇定位原按 `first_cpu == scenemode_targets().first()` 比对——目标已
  retain 掉引导核 CPU0，其 `first()` 是 CPU1，而 little 簇首核恒为 0 → **恒不命中**，core_ctl 路径整体静默失效
  （max_cpus 钳制 / 引导核护栏 / min_cpus 压低与两处记账全是死代码）。现已改按「首核号 + 簇内核数」区间定位。
  真机验证：进 scenemode 后 `cpu0/core_ctl/max_cpus` 应为 **1**（little 簇，`min_cpus` 同为 1），退出后两值都
  回到快照，daemon.log 有对应 @A 帧（`max_cpus=1 ok`）；若仍无 corectl 打点 → 定位或节点仍不可用。
  ⑩（第二轮审查记录的边界，**刻意不做自动修复**）`min_cpus` 残留无自愈路径：崩溃残留的
  `min_cpus == max_cpus == keep` 会让 `discover()` 的残留判据（`max < min`）恒假，而 `force_online_all()`
  只修 max_cpus / online——**不能**盲写 `cluster_size`（厂商把 little 的 min_cpus 配成小于簇规模是正常省电，
  盲写会破坏它）。触发需「快照 min_cpus > keep」+「崩溃」同时成立；彻底解决需持久化「scenemode 进行中」标记。
- **[已落地 2026-09-28] 分析侧「建账」三视图**（只读日志，不改调度行为）：`dvpower.py` 的 `[5]` cap 档迁移时间线 /
  `[6]` 档位温度带 / `[7]` 按 mode 能量汇总（gpu/psi 能量加权）；`dvmain.py` 的 `[7]` 播放态帧间隔直方图
  （活跃窗 `n>=15` + 长尾率 >52ms/>100ms）。**先有账再动参数**：本项目已禁 A/B，没有这三张表就只能盲调。
- **播放态 fps 旁路的存续条件（2026-09-28 评估）**：该旁路唯一产物是离线直方图，立项理由「热压制是否伤播放」
  已在 `logd_0928-170828` 上答完（无差异）→ **只在要复核 `tuned_profiles.yaml` 的 playback 参数
  （hysteresis/down_hold）或 `tuned_thermal_floor` 时才需要它**；`:31`、`:34` 两条关闭后按仓库惯例用
  `// [PAUSED]` 注释掉旁路入口（勿删代码）。已知瑕疵：同 app 多进程交替占坑（实测同秒 `9913→18144→9913`
  反复 detach/attach + `PID 切换失败: perf_event_open failed`）→ 直方图只覆盖当前挂载的进程；候选修法是
  同包多探针（`states`/`links` 已是 HashMap，但 `current_pid` 是单值、`switch_pid` 必经 detach），**需真机验证再动**。
- **播放态直方图实际只覆盖到 200ms（2026-09-28 第二轮审查发现）**：`fps_monitor.rs` 的 `MAX_FRAME_NS=200ms`
  把 >200ms 的帧间隔**整帧丢弃**（连 `n` 都不计）→ 档号 51+ 恒空，长尾率的分子分母都不含 >200ms 的严重卡顿，
  「无长尾」只能读作「无 52~200ms 级卡顿」（命令文档与 `dvmain [7]` 输出已注明口径）。若要覆盖到档表上限
  1020ms，需给直方图**单列**一个上限，**不能**直接抬 `MAX_FRAME_NS`——它同时喂 FAS 的 `frametimes`，把极端
  间隔算进窗口会拉低 avg_fps、引发过度降档。
- **息屏期 little 被顶满 100%（2026-09-28 包实锤，能量价值最高的待查项）**：`logd_0928-170828` 的 batch1
  息屏段（05:59–09:39、09:51–12:25 等）里，daemon.log **7 次**报
  `scenemode 持续顶满性能上限（little util 100%），退回 reduce 并进入 300s 冷却`，且每次都在息屏窗口内
  （前后最近的界标是「息屏已超过阈值，切换到 scenemode」与「亮屏触发事件」）→ **息屏省电在反复自我撤销**。
  配套数字：息屏+放电行 8345（占全部放电行 40%）均值 **0.42 W**（分桶 0.1~0.6 W 连续分布）、迁移 ≈2186/s、
  `psi_cpu 23.8`；这些秒数的 `package` 是息屏前残留的 `me.weishu.kernelsu`（95% screen*on=0），
  故 `[1]` 表里 kernelsu 的 0.59 W 是「息屏 0.42 W 段 + 少量亮屏段」的混合值，**不要当亮屏空转读**。
  **下一步判据（全部可用本包现成数据）**：① 按息屏时间窗过滤 `aff*\_.log`，看 little（core 0-2）上的
`u=`与 comm 是谁；②`main\_\_.log`息屏段 tick 行的`max*util`/`cur_perf`看负载形态；③ status.csv 息屏段的`gpu_busy`判别 CPU 侧还是 GPU 侧。**候选嫌疑**：dev_record 采集本身（息屏 2.3 h 写了 19 MB main + aff 每秒
全线程采样 + 每帧 eBPF，属观测者效应）/ 第三方后台（little 上高频出现`V8_DefaultWorke`、`Chrome_ProcessL`、
`CookieMonsterCl`、`TracingMuxer`、`perfetto_hprof*` 等 Chromium 系线程名）/ ChiRi 自身巡检。
  **若指向采集开销 → 结论是「诊断期不可测待机基线」（写判读口径，不改调度）；若指向第三方 → 才是压制策略问题。**

- 亮屏 gap①：电源键亮屏无 touch 事件 → 20% 稳态 3 tick 降到地板；候选 = 亮屏复用 `on_touch()` 做一次性 400ms ceiling 窗口（待用户批准）。
- 亮屏 gap②：亮屏恢复「任一权威节点报亮即恢复」不对称判定（0926 换属性轮询后需在新机制下重评；息屏保留多数票语义）。
- FAS 无帧降级（退出 FAS 交回 CLG 或固定档保底）——会改调度行为，先定取向。
- FAS 白名单真机验证：speedmobile / lolm / 国服 sgame 首刷看 devimp fps 是否贴 target 档（玩家开 90 帧则 target_fps 调 [60,90,120]）。
- playback 参数复核（tuned*profiles.yaml 内 TODO）：hysteresis 0.06 / down_hold 150、`steady_decay*\*` 实际效果（headroom 0.85 与三簇 per_cluster 天花板已于 2026-09-28 按天花板全拆删除）。
- **播放态掉帧验证（P0-3 收尾）【已答 2026-09-28】**：`logd_0928-170828` 已按「占优簇反推基线 + 超基线长尾档」
  判读完成——中/硬档窗口（272 / 246 / 793 个活跃窗）长尾率 **0.0~0.4%**、未压制基线 **0.0%**，**基线没动**。
  **遗留缺口**：本包只有常规码率视频（占优簇 12~16ms 档），无 4K/高码率样本；**在线判据不做**（无差异 = 不值得在线化）。
- **播放态「边充边放」温度曲线【本包无样本，待采集】**：`charge ∈ {charging, full}` ∩ `mode=playback` = **0 行**
  （本包充电时段没有在播视频），下次采集需特意覆盖该场景。**嫌疑与判据**：`tuned.rs` 的
  `profile_ceil.min(cap.max(floor))` 在硬档（cap 0.40）下由 `tuned_thermal_floor`（**0.55**，见
  `src/chiri/config.rs::d_tuned_thermal_floor` + 各 `module/config/<device>/feature.yaml`）把大核抬到 0.55
  ——tuned 接管的播放态在 ≥45°C 时**可比 CLG 跑得更热**（播放态不走 CLG，此时 CLG 已压到 0.40），
  这是为「视频解码需要稳定档位」有意保留的（见 `tuned.rs [thermal_ceil]` 与 03-chiri.md「热态 tuned 响应」）。
  若边充边放时整机/壳温**持续爬升不止**，`0.55` 地板即首要嫌疑（次选：充电态额外降 cap）。
  判据 = `tuned` tick 行 `cur_max` + status.csv `batt_power_w`（按 package 均值）+ 电池温度列。
- `background_uclamp_max_pct` 现值 **25**（8550；2026-09-28 由 35 下调，A1 降权）待真机复核后台同步 / 消息推送时延；回退 = 改回 35。
- **PowerBase 档位体系与触摸升档的真机验证（2026-09-28 实现，8550 = 标准 #13 / 触摸 #20 / 上限 #25）**：① 触摸时打 `powerbase-touch-tier`（debug 级）且频率瞬抬到对应桶落点；② 连续触摸逐级升档（#13→#20→#25），已在 #25 时只续窗口、不调频；③ 窗口（`touch_break_ms`，8550 未覆盖该键 → 取代码默认 400ms）过后回落 #13；④ 非 PF SoC / 桶越界时的回退路径与不启用逐位一致。8998 已移出 CHIRI_SOC_HINTS（恢复支持需补 [powerbase] 显式段与 hints 片段，现兜底 3.0W）。
- **PowerBase 无周期调用点（既有缺口，档位体系把它放大）**：`on_load_update` 只在**负载事件分支**被调用（`mod.rs:2696`），内部转调 PowerBase 后它的返回值 `Option<Duration>`（5s 防篡改重写的剩余时间）被丢弃、未纳入 `wait`——与 `fast_lock.tick()`（`mod.rs:2243`，返回值参与动态超时）不同形。后果：eBPF 负载源一旦停摆，`on_load_update` 不再执行 → **触摸升档窗口不回落**、防篡改重写也不发生，直到 `CLG_STALE_MAX` 看门狗（`mod.rs:2245` 起）释放 cpu_governor——它内部连带释放 PowerBase（旧记的「不含 power_base」已随后端改造失效），代价是整机退回系统调频，而不是「一直锁在 #25」。修法二选一（待用户定，未动）：接收返回值纳入 `wait`（但主循环只在 load 事件分支调用，还需另开超时路径），或给 PowerBase 单开周期调用点。
- ~~feature.yaml 无 SoC→根 的字段级合并~~ **2026-10-01 已落地**：`embedded_feature_str()` 改为「根 `feature.yaml` 为基底 + `{soc}/feature.yaml` 按字段深合并覆盖」（`common.rs::merge_yaml`，map 递归、标量/序列整体替换）。**注意**：全量合并下 SoC 未写的**整段**也会从根继承——8550/8998 无 `vector` 段，现已继承根的 vector（极速档）配置，**待真机复核 8550 极速行为**。
- 线程写观测：sched_setaffinity / cpuset / uclamp 写失败无观测（event 行 kind=thread_write 未实施；相关 i18n 键已删、要用需从 git 历史找回）。
- **DOWN 实测从未做过**：写 down.chr → down-boot-halted + 60s 心跳（注意实际周期 300s）+ devimp mode=down + current_mode.chr 停在 down，对比停摆前后 governor/max_freq/cpu_boost/cpuidle/IO/sched/cpuset/在线核数。
- telemetry gpu_busy 每秒读的采集成本 A/B（GPU busy 开关对照，最便宜的「功耗为何变高」验证项）。**【2026-09-28 已评估】**：采集开销的写盘主体是 devimp（20–28 kB/s，aff 帧占 88%），logd 侧 0.13 kB/s，`gpu_busy` 属 logd 每秒读、量级远小于 devimp；**A/B 已作废**，改为长时观察（同一版本内看该列读取与 `batt_power_w` 的相关性）。已知的反例警示：GPU **快照**（读 Adreno 节点）实测 playback 1.76→2.59 W（+47%，已默认关），即"读节点唤醒设备"类探针能到 W 级，故不能凭"读一次很便宜"推断所有探针都便宜。
- V4 媒体场景 GPU 频率上限：唯一能碰 GPU 功耗大头的杠杆（云音乐 gpu_busy 33.9% / 8745 74.1%、与功耗 r=0.851），掉帧一票否决，须真机先测。
- fps_monitor 挂载失败活跃期 500ms warn 节流（建议同 PID 只告警一次、PID 变化或 FAS 重激活才重试）；fps_probe 100ms poll 事件化（eventfd/自管道 + mio，`set(false)` 已 notify）。
- devimp 真机验证：main/aff 拆分后 @A/@S 实录 + 失败注入 e{errno} + 归档双文件。
- 「先读回 scaling_max_freq 再决定写」未实施（需真机确认内核同值写是否真触发重新锁频）；FastLock/PowerBase 5s 盲写改读校验、`cpu_freq_snapshot` 复用句柄（前置：cpufreq 节点支持重复读——FastReader 依赖 seek(0)，不支持 llseek 的节点读失败）。
- clamp_change 内直接加 `smax ≤ cap 档位` 校验未做（现用 governor 侧 `clamp_apply` 跃迁事件作行为级证据替代）。
- **帕累托前沿（CLG-PF）真机验证（2026-09-28 落地，仅 8550）**：① 确认接管日志出现 `clg-pf-enabled`（指纹 `92bcedf210e6aed1`、桶数 46），稳态期打 `clg-pf-tick-log`；② 若出现 `clg-pf-fallback`（缺容量 / 桶越界 / 该簇无目标）说明表与真机口径有漂移，先查 capacity 与桶边界再谈落点；③ 落点是「只下调」，观察**同场景稳态频率是否下移而掉帧不增**（R3 口径：只做长时观察，不做开关对照）；④ 触摸窗口内 PF 已与 decay 同口径关闭（`pf_target = None`）——若在触摸/滑动场景看到落点被压回低频桶，先确认 `touch_active` 状态而不是怀疑表数据。
  - **①②已结案（2026-10-01，`devimpbin/1001-190148` A08-11/8550 两批次）**：两批次接管时均打
    `[CLG-PF] 帕累托前沿查表已启用 | 指纹=92bcedf210e6aed1 桶数=46`（与 soc.yaml 逐字一致）；
    **全程无 `clg-pf-fallback`**（`capacity-missing` / `bucket-out-of-range` / `cluster-target-missing` /
    `unknown-cluster` 均未出现）→ 0928 的「头号待办 ②：capacity-missing 全程回退」在本包**已消失、PF 全程生效**。
    历史（0928）：启用后数分钟即 `[CLG-PF] 前沿表不可用（capacity-missing），回退比例路径（只此一次）`，两段均复现
    → CLG-PF 全程未生效（落点 = 旧比例路径）；诊断已增强为带核编号（`capacity-missing(cpu=N)`）。已排除
    「soc.yaml capacity 段缺失」（FDP 用同一个 `soc_capacity_for_group` 且无 `nodata` Skip）。
    **③④ 现可观察**（PF 已真正生效）：落点只下调、同场景稳态频率是否下移而掉帧不增（R3 口径：只做长时观察）。
- **`modes` / `anticipate`（三模式取点与预判交叠带）已生成但运行期未消费**：当前只落地了「需求桶 → 三簇目标频点」。要在 CLG 里消费三模式差异与交叠带，需先把「流畅度下界（`reserve_pct`）」与「预判基准提前量（`lead_frac`）」从占位值（reduce 0 / default 0.10 / boost 0.25；`lead_frac` 0.30）定稿——重跑脚本即可覆盖（`--reserve` / `--lead-frac`），不需要改代码。
- **FDP（前沿主导的放置内核）已落地（2026-09-28），待真机验证**：`src/chiri/energy_cost.rs` 用**双边净收益**给后台/批处理 promote 选目的簇——「等算力下把这份算力放到边际 mW/Mdmips 更低的地方」；数据不手抄，直接读 `soc_frontier_policy().per_cluster.<簇>.{freq_khz, power_w}`（由 `scripts/frontier_policy.py` 从文档 §3 生成）。缺省 `fdp_enabled: false`（8550 已开 true）；`bg_promote_exclude_prime` 仍把关 prime 资格（静态表不含漏电/idle-exit，且判不出 P-b 的 >2600 Mdmips 门槛）。**降级口径（2026-09-28 用户定）**：`fdp_enabled` 为真但静态数据缺失（无 `capacity` / 无 `per_cluster` 功耗表）时，经 `energy_cost::fdp_available()` 判定后**回退原 A3 放置路径**（不把后台 promote 整段停掉）；FDP 自身的配额/净收益/选核否决仍不回退。**待验证**：① 日志 `action=fdp` 的 `net` 与真机 `batt_power_w` 走向是否同号（R3 口径：只做长时观察，不做开关对照）；② 自算例（little u=0.5 / big u=0.3 / prime u=0，d=168）在真机上是否落在同一档（模型 vs 实测 `scaling_cur_freq`）；③ 若出现「该升不升」（后台批处理变慢）先把 `fdp_cost_hysteresis_w` 调到 0.01。
  - **真机实证（A08-04/8550 两段会话，2026-09-28）**：`action=fdp` 帧 91 条**全部 Skip**（`hysteresis` 90 + `dstfreq` 1），**零 Move 帧、`pin bg_busy` 零出现**——该场景下 FDP 未批准任何迁移（这些时刻旧 A3 路径会 promote，属可观测的行为变化）。保守来源 = 成本包络口径（见下条「不同底」）与 `fdp_cost_hysteresis_w` 阈值；**阈值先不动**，按 ③ 收集「后台批处理是否变慢」的证据再定。另：`fdp_ready` 判据已与准入同源（防「数据缺失时 FDP 与 big 饱和保护同失」，2026-09-28 修）。
  - **真机基线（2026-10-01，`devimpbin/1001-190148` A08-11/8550 两批次）**：`action=fdp` 共 **270 帧**，
    Skip **268**（`hysteresis` 157 + `dstfreq` 111）、Move **2**（`little->big f_src=1785`，`move_group fdp ok`）
    → **动作率基线 2/270 = 0.74%**（相对 0928 的 0/91 已有首个 Move）。**口径注意**：本包统计是**旧脚本产物**
    （`aff.txt`），`dstfreq` 111 里含「同一候选被多重拦截只报一个」的老口径高估（见 T1a 修，`energy_cost.rs`
    出参已改为 blocked 位组合 `quota+dstfreq` 等，下包起可分离）；`net` 与 `batt_power_w` 同号性仍待 ①。
- **8650（PJX110 / 一加 Ace 3 Pro / 8 Gen 3）首版适配待真机复核（2026-10-02 落地）**：`config/8650/` 三件套已建、`CHIRI_SOC_HINTS` 已加 `8650`，参数沿用 8550 同代际口径，**本机零实测**。复核清单：① 识别是否命中（真机 `ro.soc.model` / `ro.board.platform` / `/sys/devices/soc0/machine` 至少一处须含 `8650`；若只暴露板级名 pineapple，则片段与目录名要随之改）；② 拓扑 little 0..2 / big 2..7 / prime 7..8（2+3+2+1 四 policy，两组 A720 同归 big）与真机一致；③ 标 TODO 的项：`top_app_uclamp_max_pct`、`background_uclamp_max_pct`、`CoreCtl.scenemode_offline`、`hysteresis_mid_c`、default 的 `per_cluster.little`（8550 抬到 0.95 抑制贴顶）、触摸 600ms/2 档、`powerbase.target_power_w`；④ `soc.yaml` 的完整频表缺口（capacity 已用真机值补齐：379 / 923 / 1024）——参考内核 `android_kernel_oneplus_sm8650` 内无 vendor DTS（`dts/qcom` 止于 sm8450，不可套用），频表由 EPSS 硬件 LUT 运行时注册，只能取真机 `scaling_available_frequencies` / `cpu_capacity`；⑤ 该机 `debug.tracing.screen_state` 缺失只影响 `screen_prop` 列（写 `-`）与属性轮询路径，**息屏判定本身正常**（uevent 路径照常翻转：status.csv 有 `screen_on=0` 行、daemon 有成对的息屏/亮屏事件）。**scenemode 是否入场仍待观察**——本包最长一次息屏不足 `scene_mode_delay_secs=300`。
- **8650 首包实证（`devimpbin/1002-143231`，A01-02 / 30000）**：① 识别已命中（`# soc=8650`、"检测到特定处理器，已启用 Chiri 专用调度器"），CLG 接管 4 policy（P0 [0,1] / P2 [2,3,4] / P5 [5,6] / P7 [7]），调速器选型 uag 四 policy 一致；② **FAS 帧源不稳**：lolm 会话里「已挂载 uprobe 到 PID」在同一秒被刷十几次（26102 / 7745 / 20329 / 3105 / 18862 …多 PID 交替占坑），夹杂 6 次 `PID 切换失败: perf_event_open failed`，平均帧在 21.3~109.9 之间跳 → 档位 120↔60 每隔几秒反复切（07:48:49–07:49:55 十余次）。与 8550 已修的「降/升档重叠区」不是同一处：这里根因在帧源（多进程占坑 + perf_event_open 偶发失败），帧源不稳时判据改得再准也会抖。待办与 F3/F4 同源（`monitor/fps_monitor.rs`：PID 选择与失败退避），**先别在 8650 上调档位参数**；③ 环境性告警（非缺陷）：`/sys/module/cpu_boost/parameters/input_boost_enabled` 与 `perfmgr` 节点不存在、`core_ctl/enable=0`（不接管）、msmp/horae clamp 佐证节点不可用。
- **zumapro（Pixel 9 Pro / Tensor G4）首版适配待真机复核（2026-10-02 落地）**：`config/zumapro/` 三件套已建、`CHIRI_SOC_HINTS` 已加 `zumapro`（`ro.board.platform=zumapro` 已在 devimp 头里确认，识别应无悬念）。复核清单：① 拓扑 little 0..4 / big 4..7 / prime 7..8（4+3+1 三 policy）与真机一致；② 调速器选型会取 `sched_pixel`（Google 自家），若它与 CLG 的上限写入打架就删掉该表项回退 schedutil；③ `Sched` 段已置 `enabled: false`——真机 `ls /proc/sys/kernel/sched_*` 已确认只有 12 个节点（与 8650 同形态），无 `sched_migration_cost_ns` / `sched_nr_migrate`，无需再验；④ `soc.yaml` 的 capacity 与完整频表缺口：DT 只有 `capacity-dmips-mhz`（193/927/1024，与 sysfs `cpu_capacity` 不同量纲，勿直接填），DT 无 CPU OPP（频表来自 cpufreq_domain 驱动/固件）；⑤ 该机 `debug.tracing.screen_state` 缺失只影响 `screen_prop` 列（写 `-`），息屏判定走 uevent 路径仍可翻转（同 8650 的订正口径）；GPU 候选节点全无 → gpu 列恒 `-`。
- **F3 / F4 待真机**：F3 = fps 探针 attach 累计失败数（已补打点 `fps-monitor-attach-stats`）需真机看真实失败率；F4 = FAS 延迟退出时长（`deactivate_delay_secs`）**本轮刻意未改数值**——缩短会提高「短暂离开（弹窗/画中画/后台配音）被误判为真退出」的概率并引发 activate/deactivate 抖动，应先测「离开→回来」时长分布。
- **T11 三模式取点待真机复核（8550）**：`reduce.steady_decay_target_ratio = 0.80` 是刻意低于 default 的 0.90（天花板拆除后 reduce 与 default 其余参数已很接近，靠取点保住省电档差异）；`boost` 的 `steady_decay_enabled: false` 是「对功耗宽松档不主动下探」的选择。判据 = 省电档滑动手感 + 同场景稳态频率；省电档过钝就把该行删掉回到缺省 0.90。**8475/8998/根兜底的 boost 档同批也已显式 `steady_decay_enabled: false`**（同判据：`up_threshold` 0.55~0.65 时 T 落进常态负载区、削掉响应优势），这三份的 reduce/default 仍走缺省开启，待真机复核。
- **【P1，2026-10-05 实证】FAS 游离在热阶梯之外**（`devimpbin/1004-233049`，8650 / lolm）：CLG 侧电池 41/43/45℃ → cap 0.85/0.60/0.40 三级（`module/config/8650/feature.yaml`），FAS 侧只有电池 **45℃ 单点**护栏（`module/config/normal/fas/*.yaml` 的 `core_temp_threshold: 45.0`，命中才把 perf 压到 `core_temp_throttle_perf`），两者不互通。实测 lolm FAS 放电 21216 s（5.9 h）：batt 41.4~45.7℃、均值 42.5℃，其中 **11375 s（53.6%）落在 CLG 中档（≥43℃）区间、2864 s ≥44℃、仅 966 s（4.6%）真正触发 45℃ 护栏**；同期 CLG 的 `thermal_cap_pct` 已给到 60，FAS 一行不理。代价是电池长时间贴在中限带（本包 cap=60 档 14193 s，其中 lolm 12897 s）。**TODO:** FAS 写频入口把统一 `thermal_cap` 与 `effective_perf` 取 min，或把 45℃ 单点改成 41/43/45 三档 perf 阶梯；判据 = 同版本长时观察下 batt 峰值与中档驻留秒数（不做 A/B）。**热侧并不伤帧**：cap=60 档 lolm fps avg 112.8 / p50 123 反高于 cap=100 档的 106.45，低帧集中在切场景/加载（fps<60 共 868 行 = 4.1%）。
- **【P2，同上】FAS 120 档 perf 过供**：lolm 目标 120fps，fps 分桶 `<70` 18.6% / `100~115` 3.8% / `115~125` 43.0% / **`>=125` 33.9%**（p50 123 / p95 128）→ 超目标 ≥5fps 的时间占三分之一。由 snap 反解稳态 perf_index ≈ 0.59（little 1498 / big 1930 / prime 1979 MHz vs hw_max 2265 / 3148 / 3302）。**TODO:** 复核 120 档升档落点 P=0.70 是否可下探（掉帧由 PID 抬回）；**前置条件**：8650 的 fps 探针多进程占坑尚未修（见 8650 首包实证条），帧源不稳时先别动档位参数。
- **【P2，同上】FAS 延迟退出期 mode 污染 + 频率/调速器未即时回落**：`mod.rs:2692` 在 `request_delayed_exit()` 后 `continue`，mode 列仍是 `fas`，退出期（默认 15 s）内任何切到前台的包都被记成 fas 并吃着 FAS 频率：本包 com.tencent.mm 23 行 8.12 W、launcher 10 行 7.53 W、tim 49 行 6.43 W、酷安/京东/bili 共 115 行（合计约 0.6 Wh，量小但污染离线归因）。**TODO:** **只动诊断列**——把 `pending_mode_after_fas` 暴露给 devimp 写入端（新增 `mode_eff` 列，或 mode 列直接写 pending 值）。**不要在退出期提前降 perf / 恢复调速器**：`mod.rs:2690` 注释已明确「mode_clone 不动，否则出现『文件写 default、FAS 还持着频率』中间态」，且延迟退出的设计目的就是 15 s 内切回白名单应用时由 activate 无缝续期，提前松手会引入 activate/deactivate 抖动（与 F4「延迟时长先别改数值」同源）。
- **【P1，同上】异常退出后 governor 残留 performance**：23:12:10 `Governor` 已把 4 个 policy 由 performance 恢复为 uag，但 23:14:40 新一轮启动仍报 4 条「检测到残留 performance 调速器（上次可能异常退出）」→ 中间两分钟有人又写成 performance 且没释放。候选根因：FAS 引擎 `policy_controller::reset()` 按 `orig_governor` 写回，而该快照若取自 GovernorGuard 已切 performance 之后（`fas_manager.rs` 已有「GovernorGuard 必须先于 load_policies 快照」的顺序约束）就会把 performance 写回去。**TODO:** `reset()` 里 `orig_governor == "performance"` 时改回 uag/schedutil 并 warn。附带结论：23:14~23:30 的四次重启**不是** daemon 自归档（无 128MB 行、无 panic、间隔 8m27s/6m52s/43s），23:30:49 即产出抓包 → 判为外部触发。
- **【P2，同上】daemon.log 在进程存活期提前停写**：x_1004-232307 那轮进程活到 23:23:07，daemon.log 末行却停在 23:14:53（8m14s 无日志），而 status.csv 写到 23:22:16 → 死亡现场被抹掉，「达到门限才重启」这条唯一可见路径随之失效。**已加失败可见性（2026-10-05，根因仍未定）**：`append_open` 的 `.ok()` 与写失败分支原本全程静默（只丢弃句柄），故「没有事件」与「写不进去」事后无从区分。现加 `APPEND_FAIL_STREAK` + `report_append_failure()`：写成功归零，失败累计，首次与之后每 100 次各报一条 warn（新 i18n key `logger-append-failed`）；`APPEND_REPORTING` 防 warn 自身重入递归，报告一律在锁外发。**下一步**：下个包看是否出现该 warn 及其连续次数——有则按次数定位是「目录被删/重建失败」还是「句柄失效」，无则说明原现象只是该时段没有 INFO 事件（本包无法区分这两种）。
- **【记录，同上包】`status.csv` 无 `cpu_dyn_w`/`resid_w` 属预期不是回归**：本包采集于 10-03~10-04（A01-04 / A01-06），两列 2026-10-05 才落地 → `dvenergy.py` 只报提示不产表。标定复核仍按上面「功耗分解两列已落地」那条，等 A01-07+ 的包。
- **FDP 源簇让出收益「不同底」（2026-09-28 审查发现，已在代码留 TODO）**：`src/chiri/energy_cost.rs` 的源簇一对值 = 现状取**实测**、之后取**模型**，而目的簇侧已用 `f_dst_now`/`f_dst_after` 的 min/max 包络把两侧锚到同一底；源簇侧缺模型现状值 → 当 little 实测高于模型（QoS / 热 / 别的线程抬频）时 `min` 仍会**放大让出收益**，可能推动不该有的迁移。修法 = 对称取 `f_src_now = min(实测, 模型)`；动手前先用真机 `action=fdp` 日志的 `net` 与 `batt_power_w` 走向对照（R3 口径：只做长时观察）。

### WebUI / 工具

- 视觉走查人工项（本机无 agent-browser/无头浏览器）：0926 screen_off_value / screen_prop 交互、analyze/ 图表页。
- 导出轮询改阻塞 exec 未做（需真机验证长命令回调）；mock-shell readMany 整体 failed 的已知缺口保持。
- state.svelte.ts 零单测；export 轮询 4 分钟不可取消。
- 归档新格式真机验证：logd/ 产出 `.tar.lz4`（至今仅合成夹具）；WebUI 导出 `.tar.gz` 由内置 `chiri gzip` 产出（退出码 0）。
- aff Busy 释放修复待验：需一份凉机、放电、非 boost、压力有升有落的包——压力解除后 `pin=1/home=-1` 计数应回落到零。

### 配置 / 文档 / 杂项

- updateInformation/changelog.md 空文件，是否填 Alpha06-12→14 汇总待确认（HotUpdateInfo.md 亦空）。
- 完整安装会清掉 down.chr / rhine.chr（用户停摆/实验室状态丢失，不在「保留配置」范围）；实验室还原不原子（meta 写失败半还原可重试）；实验室与 sync_meta_snapshot 都写 meta.yaml 理论交错未加锁。
- （2026-09-17 提出，**先核实**）down.rs 非 UTF-8 读失败当缺失 → 模板覆盖会冲掉用户手改；BOM(U+FEFF) 两侧 trim 行为不一致；monitor `dynamic_enabled=false` 直返 global_mode 绕过 fas/akmode 门控；看门狗释放后 vector/特调无周期重建。
- panic 收尾缺 governor/gpu/lab 清理（靠重建幂等收敛，P2 可接受）；kgsl-3d0 与 devfreq 扫描可能收双节点（重复写无害）。
- meta 两个写者共用固定 tmp 名（撕裂需两线程同时写）；rhine on_startup→DirWatcher 毫秒级窗口写入丢失；meta 被改非法后锁定期不自动 reassert（等下次改文件/重启）。
- OPlus 电池：2S/2P 判定缺一次性标准节点读值（已加 telemetry-raw-snapshot 兜底）；bcc_parms 下标 12/13 疑似重复电流字段待核对；2S 机型自检：`dumpsys battery` voltage 3841mV 级=单节域（平均对）、7680mV 级=2S 直串（改回求和）。
- 文档修订（用户定暂缓，顺手勿改）：README FAS 措辞与断链（README:28 链 `updateWithoutRestart.md` 不存在；`updateWith.md` 已移 `.archive/mdocs/`，恢复链接或删除）；mdocs/socList.md 滞后（只列 8550，实际 8550/8475/8998 三份）；`src/chiri/mod.rs:184`「多实例」注释过时（实际单实例）；affinity.rs 头注释 0.2 vs `PINNED_WEIGHT` 0.4；soc.yaml capacity 注释「厂商板级 DTS 覆盖」建议改弱为「来源待证」。（原列的「feature.yaml『整簇断电』注释 vs 实际只下线 prime」已随 2026-09-28 的离核策略改动一并消解——注释与行为现在都是「下线 little」。）
    - **第四轮审查修订（2026-10-07，全部「已修 / 未验证」，`mod.rs` 接线与新增测试未运行）**：① **配置指纹确定性碰撞**——旧 `diag_config_identity` 把 pretty-Debug **整体排序**、丢掉层级，交换两个 profile 的参数值即得**相同**指纹（**确定性碰撞**），配置变了却不递增代际；现改为 `canonical_debug`：按缩进建层级树、**只排同级子项**，层级保留。② **身份/代际未一起冻结**——`SampledFrame` 在**采样入口一次性取齐**（`ts`/`fg_pid`/`fg_starttime`/`config_identity`/`generation`/`sequence`），同 tick 先 bump 后取快照，`dispatch` **不再取代际**。③ **panic 排空二次 panic 与写入乱序**——排空逐帧 `catch_unwind`（二次 panic 仅损失当帧记账）；`submit` 在 `failed` 态**等待 `drain_finished` 再 `Err`**，保证**先旧后新**（**不由文件锁保证**）；等待有界、超时记 error。**未运行**：`mod.rs` 接线与 `logger.rs` 新增测试；真实运行证据为 `diag_cost.rs` 16 passed、`diag_worker.rs` **15 passed**、阶段 A Python 30 passed。
