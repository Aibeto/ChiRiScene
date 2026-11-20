---
id: "37bd8d91-1f1f-4a1f-a1f2-8e83dbb1f165"
title: "ChiRi 长期契约与教训（自 Cursor 记忆蒸馏，2026-11-20）"
description: "铁律与协作、配置与数据契约、调度语义现状、亲和放置与 EEM、负载源降级、内核取证、通知与脚本、能量与收益口径、工具与环境、流程红线；含全部关键数字与作废口径标注。"
status: "active"
created_at: "2026-11-20T15:49:57.147Z"
updated_at: "2026-11-20T15:49:57.147Z"
content_hash: "8c13b481e91968e2e1474de55db3507ac15c306f2485769ef8588f2e75230d3a"
source_paths:
  - "agentsdocs/05-maint.md"
  - "agentsdocs/03-chiri.md"
  - ".archive/.cursor/memory/MEMORY.md"
session_ids:
  - "session-c3195d85-1409-4552-a706-f4834a248c60"
memory_body_ids:
  []
---

# ChiRi 长期契约与教训（记忆蒸馏，2026-11-20）

> 来源：`.cursor/memory/MEMORY.md`（349 行 / 62,363 B）+ 6 篇当日日志（2026-10-26 / 10-30 / 10-31 / 11-01 / 11-07 / 11-20）。
> 去向：Mnemon Documents（标题拟「ChiRi 长期契约蒸馏」）。原件已镜像到 `.archive/.cursor/memory/`，本文件是导入源。
> 原则：只留**仍然生效的契约、数字与教训**；已归档进 `agentsdocs/01~07` 的叙述不再重复；作废口径直接标注。

## 1. 铁律与协作

- **文件不出项目**：任何 AI 产出（计划/报告/探针）必须落在项目内；Cursor 计划先落 `.cursor/plans/<日期>-<名称>.plan.md` 再删外部原件。
- **计划/报告分目录**：计划类 → `.cursor/plans/`（权威 = `agentsdocs/07-todos.md:37`），报告类 → `.cursor/docs/`；`mdocs/` 只放项目原有文档。
- **不越权提交**：未经用户明确授权禁止 `git commit` / `push`（2026-10-07 铁律）。
- **不做 A/B 对照**：判据 = 逻辑必然 + 模型折算 + 功能回归观察；同版本同场景观察可以，并行/交错对照一律不做。
- **不降采样质量**：凡让数据变少/变粗的改动一律不做（行为延迟不算）。
- **不用 AI 自选口径否决用户目标**：评估负面时保留条目附意见，决定权交回用户。
- **收口三重核查**：`list_agents` + `job_list` + `ps -eo pid,etimes,args`；派子代理必须写明终端调用上限 / scratch 路径 / 互斥写范围（教训：子代理覆盖过主线程文档与脚本）。
- **汇报结构**：问题 → 原因（含源码路径）→ 后果 → 影响（量化）→ 快速判定方法（哪个 dv*.py 看哪段）+ 改哪一行；缺一不可，只给数字视为不合格。

## 2. 配置与数据契约

- **LTO 下发（2026-11-07）**：`cargo xtask build --lto full|thin|off|auto` 或 `CHIRI_LTO`；本地 `scripts/build-arch/build-arch.sh` 为固定档、全模块共用、失败不回退；CI 用 `auto`（逐模块 full→thin→off）。用户口径 `full` 在 Cargo/rustc 侧必须写 `fat`。实测同一提交 chiri off 7,117,024 B vs full 6,059,000 B；eBPF off 3,824 B vs full 3,504 B。
- **息屏静默档（2026-10-31）**：亮屏 telemetry 1s / mode file 5s / `EVENT_POLL_MS=1s`；息屏 `TELEMETRY_LOG_INTERVAL_SCREEN_OFF=5s` / `MODE_FILE_REWRITE_INTERVAL_SCREEN_OFF=30s` / `EVENT_POLL_MS_SCREEN_OFF=2s`（thermal 2s 不动）。目的：减少息屏唤醒打断 freeze（重跑 `dpm_suspend_start` 撞 vWdog ≈6.18 s 窗口）。息屏兜底 2s < `CLG_STALE_MAX=5s` ⇒ 断投发现时间不变。
- **归档格式（zstd，2026-10-12 起；旧 lz4 口径作废）**：产物 `logd/<ts>.tar.zst` 与 `logd/devimp_<ts>.tar.zst`——`pack.sh archive` 只打无压缩 tar，Rust 内置 zstd crate 流式压缩（级别 2、6 线程），落盘失败才回落 `.tar`；WebUI `export` 自 2026-10-13 起只打纯 `.tar`。**分析侧禁按扩展名判断**：`dvextract.py` 按 magic 路由（zstd 三级兜底 / LZ4 走 `dvlz4.py` / 其余 raw tar），入口 `dvrun.py`。
- **logd 预算清理**：按归档批次原子删（`<ts>.tar` 与 `devimp_<ts>.tar` 同进退、最新批次永不删）；`LOGD_MAX_BYTES` / `DEVIMP_DIR_MAX_BYTES` 256MB、TARGET 200MB。
- **soc.yaml 硬件基线**：`module/config/{soc}/soc.yaml`（topology/capacity/freq/idle/dpc）编译期嵌入，是 sysfs 兜底与校准基准、**不是调优输入**；三件套（meta/feature/soc）缺一 build.rs panic；`frontier_policy` 段由 `scripts/frontier_policy.py` 受控块生成，块内禁手改。39 个机型中 39 有该段（仅 `sm8475` 缺）。
- **meta 新字段五处同步**：`MetaYamlFile` / `ExternalMetaOverrides` / `chiri::config::Meta`(+apply) / meta.yaml 模板 / WebUI `contract/meta.ts`。
- **status.csv 28 列**：末尾 `daemon_rss_kb`（VmRSS，失败 0），只作跨会话趋势；WebUI 解析兼容 27/28。
- **main_ 45 列**：末尾 `activity`（`pkg/.MainActivity` 归一；非当前包写 `-`）；`ActivitySampler` 同包 3s 节流、换包 1s 内立即，只在配置了 Activity 级规则或 `meta.dev_record` 时查询。`aff_` 文件头只写标题 + 4 行可解析 schema（@A/@S/p/t）。

## 3. 调度语义现状

- **温控三处同关才算关（2026-10-16，勿当回归）**：`feature.yaml` `Thermal.enabled=false`、同段 `tuned_resp_enabled=false`、FAS `core_temp_threshold=0.0`；FAS 活跃时 `src/chiri/mod.rs:3651` 整体跳过 ChiRi 热保护。内核 thermal 独立（~95°C 起控 + LMh/DCVS），关掉不新增过热风险。
- **热压制恒钳 `Thermal.clamp_heavy`（默认 true）**：cap 窗口内恒钳写频目标、忽略 `free_above` 豁免；只钳写频不回写 `current_perf`。
- **热阶梯四级**：`1.0 → soft_perf_cap → mid_perf_cap → hard_perf_cap`，8550 取 41/43/45°C ↔ 0.85/0.60/0.40；`normalize` 强制 soft<mid<hard、hard_cap≤mid_cap≤soft_cap，越界钳制 + warn（`thermal-config-clamped`），硬限兜底判据 `hard < soft + 1.0`（不是 ≤，否则 (0,1) 差值配置会让 `f32::clamp` panic → watchdog 重启循环）。每级上下边都带回滞。
- **热态 tuned 响应**：`tuned_resp_enabled`（默认 true）+ `tuned_thermal_floor`（默认 0.55，≥ hard_perf_cap）；有效上限 = `min(档位天花板, max(cap, floor))`。
- **P1 取消二段式息屏已落地（c48a06c，2026-10-12）**：doze 段 4 行专用覆盖已删（`src/chiri/mod.rs` 原 :3725/:3728/:3729/:3730 及 :3321-3329、:4222-4233 同口径），息屏后 CLG 即 reduce 同参；五机型 `scene_mode_delay_secs` 300→3000（代码缺省同步 3000）。回退 = 恢复 doze 覆盖 + YAML 改回 300。净收益 +0.36 / +1.59 / +3.08 Wh。
- **CLG 高位升频门槛（2026-10-13）**：`high_zone_gate` 0.85 / `high_zone_count` 0（关）/ `high_zone_util` 0.90 / `high_zone_hold_ticks` 20；只压决策意图（decision=`hz_hold`），不封顶不回写 `current_perf`；8550 三段启用 count=2。
- **CLG 方向稳定（P1-2）**：上边补齐与下边同源死区 gate（no-op up 记 hold、不清 `down_wait`）；up-hold 期间 `down_wait` 有意不清零（代价 = 可提前 1 tick 降频，换来噪声不抹掉降频确认进度）。
- **`perf_lock` 性能优先档（2026-11-07，首个使用者终末地）**：落点唯一 `src/scheduler/fas/policy_mgmt.rs::apply_freqs`（在 util 软封顶之后、`thermal_cap` 之前）；`load_only = !locked && uses_load_control()`；`controller::adjust_target_for_util` 与 `frame_pipeline::apply_steady_decay` 在锁频档开头 return（防内部状态漂移）；`#[serde(default)] perf_lock: bool` 缺省必须 false；配 `fps_gears: [60.0]` 让齿轮机升降两段不执行；热限频仍生效。
- **模式全序**：FAS 白名单 > 特调（`activity-first` 让位例外）> activity 级 app_modes > 包名级 app_modes > global_mode。`special_yields_to_activity = flag && dynamic_enabled && has_rule`（三条件缺一不可）。
- **息屏保活锁 `wake_guard`（2026-11-01）**：息屏向 `/sys/power/wake_lock` 写 `chiri_screen_off`，让挂起尝试吃 -EBUSY ⇒ 不进 s2idle ⇒ 绕开 vWdog ≈6.18 s 撞线；代价 = 息屏不进 s2idle。`src/chiri/wake_guard.rs`（acquire/release/sync），调用点三处（screen 事件、DOWN 停摆、config_dirty 热重载），统一表达式 `enabled && !halted && !is_screen_on`；开关 `meta.yaml wake_guard_enabled`，缺键按机型（仅 `sm8750` 默认 true）。

## 4. 亲和、放置与 EEM

- **写失败是一等状态（2026-10-26）**：`pin_fail_wait()` = 8s × 2^(n-1) 封顶 60s；进程级 `narrow_pid` 备忘（同 pid 同簇 15s，只对 EINVAL 记账，前台/boost 变化清表）；`placement::Engine::abort()` 退回 `applied` 锚点，退避 4s × 2^(n-1) 封顶 30s。
- **记账取回读掩码**（`Engine::note_applied`）：cgroup cpuset 与不可用核会让内核静默摘核，写返回 0 ≠ 全生效；`@A act=place` 的 `value` 追加 `eff=<lo>-<hi>` 供对账。
- **取证基线**：sm8550/PHB110 14 批 A02-14 —— `place` 写失败 80%、ok 里 54% 又被裁剪 → 真正生效 ≈9.5%；`pin fg_pin` 失败 90.5%；失败呈爆发（单秒 431 条 = 同 pid 417 个线程钉同一颗核）；sm8750 同代码几乎零失败。
- **复核（2026-10-30，29 批）**：fg_pin e22 63,856 → 60；place 62,531 → 6,353；合计 19.6×。但 place ok 的 34,937 行里 81% `settled=false` / 64% `clipped=true` ⇒ 部分 EINVAL 转成静默裁剪，故新增 `note_stripped`（写 ok 但回读整簇被摘 → 同样记 15s 窄域备忘）。被摘核 94% 在核 0-2，但小核仍承担 11-12% 驻留、93 个 pid 拿到过 ⇒ 间歇可用，不是 cpuset 收窄。
- **`narrow_pid` 按 (pid, 簇) 分键**：`HashMap<(i32, CoreGroup), Instant>`，超 512 条按到期清理；单槽会被异簇覆盖。
- **EEM `k` = 忙核数**（`[eem_k_busy]`）：`k = count(util > EEM_BUSY_UTIL 0.02).clamp(1, 本簇核数)`；**不要再用 `ceil(Σ clamp(util))`**（均匀轻载会被当重载永久封顶）。`eem_target_freq` 仍过 `demand_with_core_floor`（`max(Σ, busiest×k)`）。
- **EEM 规划池 = 可行域交集**（`eem_allowed_cpus`）：每轮读 `GROUP_TOP_APP` / `GROUP_FOREGROUND` 的 `cpuset.cpus` 求交再与在线位图求交，簇内无允许核即整簇剔除；读 cpuset 带 `EEM_ALLOWED_TTL=1s` 缓存（写路径 160ms 一拍）；写前再求交（`dst_mask = a.mask ∩ allowed`），空集回滚不发 syscall（`act=place result=skip reason=dst_empty`）；`clipped` 以实际写下的掩码为基准。
- **封顶释放是滞回**：超 `EEM_CAP_RELEASE_TICKS`(10 tick = 1.6s，不是 10s) 挂起（`eem_cap_suspended`），只有甜点档供给重新够用才解挂。
- **`best_placement`**：按调用方给的核数比价（单线程 1、簇级 k），要求候选 `supply ≥ demand` 且在线核数 ≥ cores；两簇都供不上时取供给更大者（并列留先到者）。
- **升频速率闸判据前置**：`if !is_significant_jump && up_wait < up_rate_limit_ticks` 才拦；显著跳变直接 `up_wait=0; current_perf=target_perf`。
- **换前台即清记忆**：EMA/steady/up_wait/high_zone_wait/fast_down/eem_* 在包名变化时重置（`[fg_reset]`），但不动 `current_perf`/`current_freq`；空包名不算切换。
- **1107 基线（下轮对比用）**：up_wait 30.6% / up_wait→up 中位 478ms / place clipped 38.3% / 被摘核 86.1% 在核 0-2 / 小核被摘率 79-94% vs 大核 2-14%；daemon 自耗 playback 档 p50 7.9% 单核。

## 5. 负载源与降级

- **eBPF 主探针非全机型可用**：`BPF_MAP_TYPE_RINGBUF` 需内核 ≥5.8；加载失败必须切 `start_proc_stat_fallback`（逐核 /proc/stat 差分，节拍与 eBPF 一致），否则 daemon 活着但永不发 SystemLoadUpdate，CLG/tuned 看门狗反复 release（sm8250/4.19 实测 68 分钟 release 26 次）。
- **降级两条硬约束**：`total_delta == 0` 沿用上一拍；连续失败 >50 拍（≈8s）返回 Err 交回看门狗——宁可不接管，也不发全 0 把频率压到底；不伪造 BpfStats。
- **通知按持续时长而非翻转沿**：FAS 开环 500ms 判据正确，但 417 次开环中位 1s / p90 7s，逐沿通知 = 207 条骚扰 ⇒ `OPEN_LOOP_NOTIFY_AFTER=10s`，进入只记 INFO；换会话与去激活都要清 `open_loop_since/notified`。
- **零售 GKI 缺 `sched_migration_cost_ns` / `sched_nr_migrate`**（CONFIG_SCHED_DEBUG=n，只挂 debugfs）⇒ FAS 的 migration_cost 与 Sched 段迁移微调在零售机空转，不当异常、不每 boot 重试刷 WARN。
- **`activate()` 是设备依赖路径**：先查编译期白名单再读真机 sysfs；`fas_manager` 现有 3 条单测（:588/:683/:694）因此恒红。

## 6. 内核取证（minidump）

- **容器形态**：`SYSTEM_LAST_KMSG@….dat.gz` 解出是 tar（PAX），成员 `olc_get_log.txt` / `device.info` / `minidump.bin`(256 MiB) / `prop.txt`，**不是裸 ELF** ⇒ 用全文件 grep + 结构体签名定位，别解析段表。
- **printk ring 自毁**：s2idle 路径调试打印 ~19 MB/s，2 MB ring 每 ~106 ms 冲刷 ⇒ 只留崩溃前 ~100–140 ms 记录；「首条打印时间戳 − last pet = 6.2 s」是 ring 深度产物，不是挂起耗时（2026-10-31 曾误判，已订正）。
- **撞线判据**：只看 `bark Now − last pet at` 与 `vWdog-time since last pet` 的 tick 数——三次实测 Δ 6.348–6.390 s / 均 206 tick ⇒ 30.82–30.90 ms/tick（32.45 Hz = 32.768 kHz/1024）⇒ 客侧 `bark 20000 ms` 不生效，实际窗口 ≈200 tick ≈6.16–6.18 s；栈指纹 `qcom_wdt_bark_handler` ← GIC ← `cpuidle_enter_s2idle+0x98/0xb0` ← `do_idle`。
- **suspend cycles 时基不可靠**（ns/cycle 在不同 dump 差 6 倍）⇒ 只用 sched_clock 差 + vWdog tick 反推。
- **`prop.txt` / `device.info` 是重启后快照**：只能用于计数（`persist.sys.oplus.total_abnormalreboot_count`），不能判崩溃瞬间亮/息屏或电量。

## 7. 通知与脚本

- **三处调用**：FAS 开环翻转沿（tag `ChiRi-FAS`，前 120 条静默 + 1h 同文去重）、启动完成（`src/main.rs`，tag `ChiRi-Status`，标题字面量 `ChiRi已启动`，无拦截器）、日志超限（`src/logger.rs::note_write`，tag `ChiRi-Logs`，**必须与启动通知分开**否则几秒内被顶掉）。
- **root 投递被丢弃的根因**：`NotificationShellCmd.onCommand` 在 uid==ROOT 时把包名设为 `ROOT_PKG=root` → `fixNotification` 抛 `NameNotFoundException` → `return false` 丢弃；换 uid 2000（`com.android.shell`）才落地。**退出码 0 毫无意义**（`onCommand` 丢掉 `doNotify` 返回值）。MagiskSU 无 `-l`，`su -lp 2000` 仍是 root ⇒ **`su 2000 /system/bin/cmd …`（argv 直传）排第一**；判定只认 `-v` 回查的 `posted:`。
- **脚本双身份**：daemon 走 `include_str!` 内嵌正文（每次释放临时目录、chmod 0700、跑完删，**每次必写**防同名劫持），WebUI 走磁盘副本（启动无条件覆盖 + inotify **监听目录**还原）；仓库里不能删这两个 `.sh`（编译期读取）。
- **日志超限门控**：达 128MB 先做 10 分钟冷却判定（必须排在 `watchdog_pid()` 之前），再按 `current_mode.chr` 判——只有明确的非 FAS 模式才放行，`fas` 或读不到模式都推迟（`None` 也拦）；放行后先发通知再判看门狗，无看门狗时补一条 `ChiRi-Watchdog` 人工接管告警。
- **启动通知按 DOWN 分叉**：`down::is_down() || down::read_down(&root)`（只信全局原子量会漏判竞态）。
- **远程提醒**：每天 08:00–24:00 每 2 小时拉一次（槽位 8/10/12/14/16/18/20/22），GitHub raw → gitee 兜底、失败不重试；versionCode 只认编译期内嵌 `module.prop`，恒等比较不去前导零；tag `ChiRi-Remote`。

## 8. 能量与收益口径

- **硬地板先扣**：屏幕 6.35 Wh + 息屏地板外围 4.38~8.14 Wh = 10.73~14.49 Wh（16%~22%）；调度侧现实可动 1.6~5.0 Wh（2.5%~7.6%，中枢 3.3 Wh / 5.0%）。报活跃外围必须声明用的 B 锚点（B1 0.608 W / B2 0.466 W / B3 0.327 W）。
- **CPU 动态模型只占整机 31.3%**（logd_1011 定稿，66.5 Wh 放电样本）；残差 68.7% 是 GPU/外围/模型误差 ⇒ 任何 CPU 调频方案收益上限受此约束；`resid_w` 负值 4.24% ⇒ 只做相对排序，不做绝对功耗预测。
- **P1 净收益三档**：保守 +0.36 / 中枢 +1.59 / 乐观 +3.08 Wh（= P1 代价 −1.17/−0.59/−0.09 + P2 + P4）；P2 对外 1.38~2.77 Wh（已含热态 4% 折扣）。
- **两条裁决**：① scenemode 相对 doze 的 DiD = +0.0461 W（方向与省电相反）⇒ 息屏类收益只剩 P1 的 0.02~0.07 Wh；② t6（playback/fas/akmode）与 tuned headroom 是同一杠杆。
- **热档不是省电手段**：放电期非 100 档仅 4.03%，cap=60 段均 5.33 W **高于** cap=100 段 2.32 W。
- **决策占比四套口径不可互换**（tick 行 `screen_on` 恒为 `-`）：全 mode 合并 tick（n=1,899,928）up 40.8 / down 9.0 / down_wait 9.2；default 单独 tick（n=1,329,613）51.6 / 7.6 / 3.6；全 mode 按秒（n=1,147,979）48.4 / 10.4 / 10.0；default 按秒（n=714,358）66.9 / 8.0 / 2.2。「亮屏」二字不得挂在 tick 数字上；`fas` 无 tick 行。
- **模型不能当整机预测器**：低估 1.0–1.8 W（8750 段反而高估 1.34×）；8750/8650 簇级瓦数是单核线性外推（求和 35.31 W vs 实测整机 7.03 W）。
- **日志口径**：main tick 行 `cur_freq_khz` 是决策值/上限（实测在 snap 的 `cpu_cur_khz`）；行数 ≠ 时间（tick 按 (policy,cluster) 去重 + 2s 心跳），墙钟统计要按相邻 Δts 加权。

## 9. 工具与环境

- **devimp 工具链**：`dvrun.py` 一条命令跑 extract/analyze/main/aff/status；单阶段 `--only`；探针一律写 `devimpbin/_tmp/`（禁止项目根）。
- **devimpbin 清理（2026-11-20）**：28G → 4.5G，释放 23.5G；保留最新一包 + 含 FINDINGS/REPORT 的批次 + `web/` + `_hf-cache/` + `rustup-home/` + `_cargo_home/` + 工具脚本；现存 22 个批次目录 = 43,861,339 行。
- **累计推理量口径**：`mdocs/socList.md` 值 = 账本 `scripts/devimp/soc_batches.tsv`（只增不减）+ `dvcount.py`；按「main 字节 + aff 字节」去重；SoC 取 `main_*.log` 第 3 行 `soc=`，回退 `board=`。基准 461,009,540（sm8250 116,128 / sm8550 368,633,805 / sm8650 47,820,937 / sm8750 16,665,188 / zumapro 27,773,482）。
- **本机验证**：`YUMI_SKIP_EBPF=1 cargo check -p chiri` 可用（≈36s）；五套离线回归 `energy_model` / `affinity_cpuset` / `app_mode_activity` / `clg_tuning` / `diag_slice`，`rustc --edition=2024 --test <f> -o devimpbin/_xlog/<bin>`（二进制必须落项目内）。
- **MemSearch**：只认 `$HOME/.memsearch/config.toml`；provider 必须 `onnx` + `gpahal/bge-m3-onnx-int8`（本机无 OPENAI_API_KEY）；`HF_ENDPOINT=https://hf-mirror.com`；模型副本 `devimpbin/_hf-cache/`；修复脚本必须在真实终端跑。
- **DSH 工具桥故障绕法（2026-11-18~20 实测）**：auto review 会返回散文/DSML 导致 `Unexpected token`，或直接挂死；`run_code` 有硬 120s 上限。绕法 = 单次 bash + 循环重试、或直接在 `run_code` 里用 node `fs` / `child_process` 操作文件与 git。

## 10. 流程与红线

- 全文见 `agentsdocs/05-maint.md` 的 `[workflow] 调度优化标准流程与注意点（2026-11-20 固化）`：七步流程 / 九条注意点 / 三条决策指导。
- 核心红线：失败态是一等状态；优先修结构性重犯（每 2s 重犯一次，收益 O(重犯次数)）；预算打满说明是限流器不是 bug；先量谁在迁（akmode 10,395/s vs ChiRi 0.2/s）。
