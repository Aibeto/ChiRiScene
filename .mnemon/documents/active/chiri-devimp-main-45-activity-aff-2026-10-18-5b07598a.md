---
id: "5b07598a-186e-4685-889e-4f03b31184c4"
title: "ChiRi devimp main_ 45 列 + activity 列（2026-10-18，含全局采样修正）"
description: "devimp main_ 新增 activity 列（45 列）与 aff_ 文件头精简；2026-10-18 修正：Activity 采样全局化（不限 FAS）、普通应用同样记录并参与 Activity 级规则（同包 activity 变化重算模式）；含节流/开关/旧值保留语义与真机待验状态。"
status: "active"
created_at: "2026-10-17T19:49:58.334Z"
updated_at: "2026-10-17T21:19:09.960Z"
content_hash: "ed45e49162196d1341371a31c746665f6c47b72940abe94f7e9a9e6e71c9e7e3"
source_paths:
  - "src/logger.rs"
  - "src/chiri/mod.rs"
  - "src/monitor/window_visibility.rs"
  - "src/monitor/app_detect.rs"
  - "src/monitor/config.rs"
  - "module/config/i18n/zh.ftl"
  - "module/config/i18n/en.ftl"
  - "module/rules.yaml"
  - "agentsdocs/02-convention.md"
  - ".cursor/commands/devimp-log-analysis.md"
session_ids:
  - "session-d1125492-b5c3-4174-a286-22fea661883c"
memory_body_ids:
  []
---

# ChiRi devimp main_ 45 列 + activity 列（2026-10-18，含全局采样修正）

## 背景与修正

用户需求：Activity 级性能规则需要数据训练，但 devimp 日志没有记录 Activity；同时 aff_ 文件头有已确认要删的说明性注释。

第一版实现后用户实测 `activity` 恒为 `-`，并指出设计错误：**不能只给 FAS 游戏采样**——游戏通常只有一个 Activity，真正需要 Activity 级规则的是微信/B 站这类页面内多 Activity 的应用。

根因（代码核实）：`LAST_ACTIVITY` 只在 `query_visibility()` 成功后写入，而窗口探针请求只出现在 `reconcile_fas_focus` 的**慢路径**；`fast_same`（游戏正常前台、身份匹配）分支直接 `return`，不调用 `probe.request`。因此正常对局里探针一次也不查 dumpsys ⇒ activity 恒空 ⇒ 日志恒 `-`，Activity 级规则也一直没生效。

修正：**Activity 采样与 FAS 解耦、全局化**（采样器 + 规则重算 + 文档/记忆同步）。

## 契约

- `main_` CSV：44 → **45 列**，末尾追加 `activity`（末列，0 基索引 44）；既有 44 列索引不变，旧脚本按前 44 列仍可读。
- 值 = 前台 Activity 查表键短形 `pkg/.MainActivity`（`activity_key` 归一：`.MainActivity` 与全限定同包名收同一键；`$` 内部类不剥；外部包 component 归一保留全串，但列值按当前前台包前缀校验，非当前包写 `-`）。
- `-` = **从未采到**，或当前焦点属于其他包（换包瞬间/系统叠加层；落行前 logger 仍按 package 前缀复核丢弃）。屏幕灭、采样失败、同包空焦点会**保留最后一次采样值（可能陈旧）**。
- 非 ChiRi 的 `diag_writer` 路径不采样，`activity` 恒 `-`；DOWN 停摆期间 1s 遥测与 app_detect 照常运行，采样继续（退出停摆恢复依赖 `last_determined_mode`）。
- 日志写入前按 package 前缀复核（`set_diag_activity` / `MainRow::new` / `set_pkg`），换包旧 Activity 不会串到新包行。

## 采样设计（修正后，全局）

- `ActivitySampler`（`src/monitor/window_visibility.rs`）：独立线程，只跑 `dumpsys -t 1 window` 取 `mCurrentFocus`（不取 `window windows` 段，成本约为完整可见性查询的一半）。
- 节流：同包 3s（`ACTIVITY_SAMPLE_INTERVAL`）；换包后 1s 内立即采样（`ACTIVITY_SAMPLE_MIN_GAP`），重复请求合并；节流基准是「上次查询的包」（失败也算，避免失败/空焦点时退化成每秒重试），失败 warn 按同节奏重试。
- 触发条件（调度 1s 块，`src/chiri/mod.rs`）：屏幕亮 + 有效前台，且 **配置了 Activity 级规则**（`dynamic_enabled` && `activity_aware` && `app_modes` 含 `pkg/.Activity` 键）**或**开启 `meta.dev_record`。两者都关时不产生 dumpsys 自耗。
- 采样写入 `LAST_ACTIVITY`；devimp 的 1s 行经 `logger::set_diag_activity(last_activity_key())` 同步。
- 采样器含 `catch_unwind` + `stopped` 收尾；锁毒化时取回内部值（记忆同步不把 poison 当致命）。

## Activity 级规则（修正后）

- `app_detect` 每轮读 `LAST_ACTIVITY`，经 `activity_for_package` 过滤：只接受属于当前前台包的键（`base_package` 比较，`com.x:push` 与 `com.x` 同应用）；换包旧值/系统叠加层键被丢弃，避免新包命中旧包的 activity 规则。
- **同包内 activity 变化**（非空且不同）也触发 `determine_mode` 重算；activity 不参与 identity/generation，不触发 FAS 重接管，ModeChange 用当前 generation（消费者按 package/pid/generation 校验后接受）。
- **空观测不覆盖同包旧值**（`next_activity`）：屏幕灭/采样失败/叠加层抢焦点时规则保持最近一次命中；换包、`force_refresh` 或采到新 activity 才重算；未命中仍回退包名级。

## 改动位置

- `src/monitor/window_visibility.rs`：`ActivitySampler` + 轻量焦点查询 + `allowed_query_at` 节流纯函数 + 注释/测试。
- `src/monitor/app_detect.rs`：`activity_rules_configured` 标志、`activity_for_package` 前缀过滤、`activity_observation_changed`、同包 activity 变化重算模式、注释/测试。
- `src/monitor/config.rs`：`activity_aware` 注释更新（全局采样语义）。
- `src/chiri/mod.rs`：创建 `ActivitySampler`；1s 块按配置/诊断开关请求采样。
- `src/logger.rs`：`DIAG_ACTIVITY`/`activity_cell` 注释更新（采样来源全局、节流/旧值语义）。
- i18n：`activity-sample-failed` / `activity-sampler-stopped` / `activity-sampler-worker-failed` / `app-detect-activity-changed`（中英同键）。
- 文档：`agentsdocs/02-convention.md`、`.cursor/commands/devimp-log-analysis.md`、`module/rules.yaml`（app_modes 的 Activity 键说明）。

## 训练/读法

- 过滤：`activity != "-"` 且前缀 == `package + "/"`；采样粒度 3s，同一 activity 的多行是重复采样，训练时按需去重。
- 屏灭/采样失败时列里保留旧值（可能陈旧），仍按前缀过滤；非 FAS 应用的 activity 数据现已可得（`meta.dev_record` 开启即可）。
- Activity 级规则本身只要配置了 activity 键即会采样，不依赖 dev_record。

## 验证与遗留

- host `YUMI_SKIP_EBPF=1 cargo test --bin chiri` → **233 passed / 3 failed**（3 条仍是既有 `chiri::fas_manager::tests::*`，与本轮无关）；`cargo fmt --all --check`、`git diff --check` 干净。
- 独立只读审查：无阻断问题；已修复「换包/失败后退化为 1s 重试」「文档把屏幕灭/失败写成 `-`」与锁毒化路径等问题。
- Android target 编译未在本机执行：无 NDK（zstd-sys 缺 `aarch64-linux-android-clang`），既有环境限制。
- 真机待验：需要用户构建部署后确认（1）devimp `main_` 表头 45 列、非 FAS 应用也有 activity；（2）配置 `pkg/.Activity` 规则后页面切换能触发模式变化；（3）daemon.log 无 `[WindowProbe] Activity 采样失败` 刷屏。用户可提供 adb 联调。
