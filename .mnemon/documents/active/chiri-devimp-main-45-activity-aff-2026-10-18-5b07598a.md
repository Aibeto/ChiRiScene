---
id: "5b07598a-186e-4685-889e-4f03b31184c4"
title: "ChiRi devimp main_ 45 列 + activity 列与 aff_ 文件头精简（2026-10-18）"
description: "devimp main_ CSV 新增第 45 列 activity（前台 Activity 查表键）并精简 aff_ 文件头为 schema + 元信息：契约、实现位置、分析脚本兼容、训练取数限制与验证状态。"
status: "active"
created_at: "2026-10-17T19:49:58.334Z"
updated_at: "2026-10-17T19:49:58.334Z"
content_hash: "f2bfc49a9ff6c5b9cb9525cb642cfbef8b104b2de5ff404dd005af4831a1538a"
source_paths:
  - "src/logger.rs"
  - "src/chiri/mod.rs"
  - "scripts/devimp-analyze.py"
  - "scripts/devimp/dvcommon.py"
  - "scripts/devimp/test_accounting.py"
  - "agentsdocs/02-convention.md"
  - ".cursor/commands/devimp-log-analysis.md"
  - ".cursor/memory/2026-10-18.md"
session_ids:
  - "session-d1125492-b5c3-4174-a286-22fea661883c"
memory_body_ids:
  []
---

# ChiRi devimp main_ 45 列 + activity 列与 aff_ 文件头精简（2026-10-18）

## 背景与结论

用户指出：Activity 级性能规则已支持（2026-10-16 提交 5384b27），但 devimp 日志没有记录 Activity 名称，无法离线按 Activity 取训练数据。核实属实：原 main_ 为 44 列、aff_ 也没有 activity。本轮把 `activity` 作为 **main_ 第 45 列（末尾追加）** 落盘，并按用户确认删掉 aff_ 文件头的说明性注释，只留 schema + 元信息。

## 契约

- main_ CSV：44 → **45 列**，末列 `activity`（0 基索引 44）。**既有 44 列索引不变**，旧脚本按前 44 列仍可读；`MAIN_HEADER`、`MainRow([String; 45])`、`DM_ACTIVITY=44` 三者同源。
- 值 = 前台 Activity 查表键**短形** `pkg/.MainActivity`（`activity_key` 归一：`.MainActivity` 与全限定同包名收同一键；`$` 内部类不剥）；未知写 `-`。
- 来源 = `monitor::window_visibility::last_activity_key()`（窗口探针最近焦点）。调度 1s 遥测块在 `set_diag_package` 之后经 `logger::set_diag_activity` 同步；tick/snap/event 三类行全由 `MainRow::new` 自动填充。
- 归属保证：`set_diag_activity` 入口的 `activity_cell` 按当前前台包前缀（`pkg/…`）校验，非当前包/换包陈旧值/外部包 component 写 `-`；`MainRow::new` 落行前按最终 package 复核，`set_pkg` 覆盖 package 后重校验 ⇒ 旧包 Activity 不会串到新包行。
- **覆盖限制**：窗口探针只在 FAS 路径运行 ⇒ activity 实际只在 FAS 会话期间刷新；非 FAS 前台通常 `-`，FAS 退出后同包内可能保留最后值（陈旧）。**离线训练样本必须过滤 `activity != "-"` 且前缀 == `package + "/"`**；要采非 FAS 的 Activity 数据需另扩窗口探针请求点（本轮未做）。
- **aff_ 文件头（2026-10-18 起）**：只写标题 + 4 行可解析 schema（@A/@S/p/t）+ 元信息（module/soc/android/ts-column）；槽级差分、刷新帧 `full=1`、`NaN` 哨兵、帧边界等说明不再写进文件头（语义保留在 agentsdocs/02 与 `scripts/devimp/dvaff.py`）。

## 改动位置

- `src/logger.rs`：MAIN_HEADER + `DM_ACTIVITY`/`MainRow` 45 列、`DIAG_ACTIVITY`/`set_diag_activity`/`activity_cell`、`MainRow::new`/`set_pkg` 复核、AFF_HEADER 精简；单测覆盖 45 列同源、activity_cell 反例、set_pkg 复核。
- `src/chiri/mod.rs`：调度 1s 遥测块调用 `set_diag_activity(last_activity_key())`（`diag_active()` 门控，唯一调用点；非 ChiRi 的 `diag_writer` 线程路径不调用，activity 恒 `-`）。
- Python 分析侧：`scripts/devimp-analyze.py`、`scripts/devimp/dvcommon.py` 新增 `C45`，`detect_schema` 判定顺序 40→48→45→44；`scripts/devimp/test_accounting.py` 新增 schema 测试（36 tests OK）。
- 文档：`agentsdocs/02-convention.md`（45 列契约/演化链/读法）、`.cursor/commands/devimp-log-analysis.md`（schema 表、列速查、过滤口径）。

## 验证状态（如实）

- host `YUMI_SKIP_EBPF=1 cargo test --bin chiri` → **223 passed / 3 failed**；3 条失败为既有 `chiri::fas_manager::tests::*`（与本轮无关）。host 编译需临时门控 `src/monitor/screen_detect.rs` 的 bionic `__system_property_get`，跑完已还原（该文件 0 改动）。
- `cargo fmt --all --check` 干净；`git diff --check` 干净。
- Android target `cargo check` 未通过：本机无 NDK（zstd-sys 找不到 `aarch64-linux-android-clang`），属既有环境限制，需 CI/真机验证。
- 真机未验证：新列首次生效需部署后在 `meta.dev_record` 开启下采日志；旧 44 列日志没有该列。

## 后续/注意

- 非 FAS 的 Activity 数据仍不可得；若要让 Activity 级规则覆盖非 FAS 场景，需扩展窗口探针请求点（成本：dumpsys 查询频率），属后续改动。
- 分析旧包时 `detect_schema` 会正确落 44 列分支；44/45 行不混用索引（I 映射只取前 44 列的同名索引，新列仅在末尾）。
