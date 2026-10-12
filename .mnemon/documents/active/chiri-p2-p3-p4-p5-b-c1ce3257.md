---
id: "c1ce3257-d1be-4442-ba78-34526636507c"
title: "ChiRi 未提交改动独立审查记录：P2/P3/P4/P5-b 修复与遗留判定"
description: "2026-10-12 对全部未提交改动（P2/P3 参数、P4 @S 帧构建分片、P5-b cpuset 校验）的独立算法审查：1 阻塞 + 2 高优修复、4 处最终修改清单、回归验证口径，以及慢帧补采 pid_comm 兜底等接受不改的低危判定。"
status: "active"
created_at: "2026-10-11T22:07:53.772Z"
updated_at: "2026-10-11T22:07:53.772Z"
content_hash: "350739cb2b5a563478c89b6f48734c866a39260bcf7813de6ddca41721ac8e45"
source_paths:
  - "scripts/tests/clg_tuning_regression.rs"
  - "scripts/tests/clg_replay.rs"
  - "src/chiri/affinity.rs"
  - "src/chiri/mod.rs"
  - "module/config/8550/feature.yaml"
session_ids:
  - "21ff571e-fa2c-4e2a-83ba-439e7f5556b9"
memory_body_ids:
  []
---

# ChiRi 未提交改动独立审查记录（2026-10-12）

对全部未提交改动的独立算法审查（P2/P3 频率参数、P4 daemon @S 帧构建分片、P5-b 组掩码 cpuset 校验、相关测试守护）。方案编号与定稿口径见文档 3290f544-df25-40da-8e41-c802a1089ea6。涉及 11 个文件，213 insertions / 72 deletions。

## 最终修改（4 处）

1. **[阻塞] 测试名与断言字面冻结** — `scripts/tests/clg_tuning_regression.rs`。测试 `saturation_selfcheck_must_not_be_weakened`（原 `saturation_must_reach_exact_hardware_maximum`）改为断言模板必须包含 `saturated_perf` 与 `assert_eq!(saturated_freq, hw_max`，并保留 `!contains("saturated_perf > 0.9")` 弱断言禁令；不再强制字面文本冻结未来调参。行为强项由 `replay_clg.py` 的 `selfcheck` 执行（`scripts/tests/clg_replay.rs:52-53`）。
2. **[高] `mask_write_skip` 时序语义** — `src/chiri/affinity.rs:1240` 附近。dedup 命中时跳过 `check_group_cpuset` 为有意行为（窗口内重读无收益），只加注释澄清，不改代码顺序。
3. **[高] P5-b 无 cpuset 树机器为 no-op 未文档化** — `affinity_cpuset.rs` empty-mask 分支补注释：`/dev/cpuset` 不存在（read 返回 Unknown）时守卫退化为 no-op，调用方保留原兜底。
4. **[高] 注释精度** — `module/config/8550/feature.yaml:249` 稳态下探目标 `0.79` 统一为 `0.792`（= up_threshold 0.88 × 0.90），与 :278 一致。

## 接受不改的判定（含低危）

- **慢帧补采 `pid_comm` 兜底**（`src/chiri/mod.rs` 行 1952 None 分支 / 2018 / 2031 附近）：cold_tids 路径仅在 `diag_slice::slow_anchor` 把 refresh false→true 的过渡帧走到；cold 项不走主循环采样，`th_rows`/`pid_comm` 不含其 comm，但 `st.base` 保留老采样 comm 供补位 p 行沿用——与差分帧「静默冷窗口沿用旧值、refresh 帧真实采样覆盖」的设计一致。残余非对称：refresh 过渡帧里「全部线程皆冷」的冷被管 pid，其补位 p 行 comm 用的是 60s 前旧值，且 line 2018 已把当下样本 comm 覆盖 `st.base.comm`。触发条件 = 非刷新→刷新过渡帧 + 至少一个全冷 tid 的冷被管 pid + comm 60 秒内变化过，极窄，记低危接受。
- **refresh 帧行数翻倍疑点撤回**：行 2031 的 anchor_rows 追加只在 `else` 分支，refresh 时 `th_rows` 已消费，无重复追加、`nfg` 计数不失配。
- P4 其余：慢帧锚点顺序、`first_pid` 兜底、`st.base` 接管语义核对无新风险。
- P5-b 其余：`cpus: &[]` 空数组不可达（perf_pool 永非空）、"-1" 误判、group 名空格、空 mask fallback 均按设计。
- P2/P3：频 p95 高于 recorded 是「理想 ratio sink」与实际带 dwell 写入的差异，非削顶失效证据。

## 验证口径（全过）

- Rust 回归：`rust_regression` 34/34、`affinity_cpuset` 14/14、`clg_tuning` 4/4、`diag_slice` 6/6。
- Python：`test_replay_clg` 5/5，`test_accounting`/`test_selfcost` 过。
- Android 交叉检查：`YUMI_SKIP_EBPF=1 cargo check -p chiri --target aarch64-linux-android` 0 error、9 warnings 与基线一致。
- `git diff --check` exit 0。

教训：回归守护测试若断言模板字面文本（如断言行原文），会冻结未来合法调参；应断言语义锚（符号名 + 关键比较式）而非整行原文。
