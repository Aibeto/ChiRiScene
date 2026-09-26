---
name: 0926 包结论落地改进
overview: 把 0926-162821 包的三条结论落成改动：P1-4 core_ctl enable 感知（省无效写与告警）、clamp_change msmp 读取减法（省 2s 周期空读），软限幅豁免带暂不动代码、先派代理量化数据再定是否重构。
todos:
  - id: p14-enable-aware
    content: core_ctl.rs：ClusterCtl 快照增加 enable 读取，shrink_prime_via_max_cpus 对 enable=0 簇直接走 offline 兜底
    status: pending
  - id: clamp-msmp-slim
    content: mod.rs clamp_evidence_snapshot：msmp 连续读空 8 次后跳过读取，每 64 次重试一轮
    status: pending
  - id: cap85-quantify
    content: 派整理代理做 cap85 时段决策值与豁免带逐秒对照，产出 cap85-band.md，回填 T8
    status: pending
  - id: verify-and-backfill
    content: cargo check 自证（Checking chiri + 0 warning），回填 device-todo.md
    status: pending
isProject: false
---

# 基于 0926-162821 包结论的改进计划

问卷未作答，按默认口径执行：低风险两项直接改；软限幅豁免带不动代码，先量化。

## 改动一：P1-4 core_ctl enable 感知

真机数据：`corectl=policy0:1/3/0;policy3:3/4/1;policy7:0/1/0` —— 仅 big 簇 enable=1，**prime 簇 enable=0**。当前 `shrink_prime_via_max_cpus` 会先对 prime 簇写一次无效的 `max_cpus=0`（enable=0 时内核不受理），再靠「读回非 0 → warn 不重试」降级逐核 offline。

改动（[src/chiri/core_ctl.rs](src/chiri/core_ctl.rs)）：

- `ClusterCtl` 增加 `enable: Option<bool>` 字段，`enumerate` 快照时顺带读 `core_ctl/enable`（读失败记 `None`）。
- `shrink_prime_via_max_cpus` 入口检查：prime 簇 `enable == Some(false)` → 不写 max_cpus，直接返回 `false` 走既有逐核 offline 兜底（warn 一次，复用 `max_cpus_warned` 去重）。
- `enable == None`（读不到）→ 保持现行为，符合「缺失即降级」原则。
- 不做 enable 周期重读：厂商可能动态改，快照仅作省写优化，纠偏路径（core_ctl.rs 约 732 行 reassert）原样保留兜底。

## 改动二：clamp_change msmp 读取减法

`clamp_change` 为【临时】采集，但 msmp 两节点本机恒 `-`，每 2s 白读 2 个 sysfs 文件。改动（[src/chiri/mod.rs](src/chiri/mod.rs) `clamp_evidence_snapshot`）：

- 增加进程级计数：msmp 两节点连续 8 次（≈16s）读空后停止读文件，快照直接记 `-`；
- 每 64 次（≈128s）重试一轮，节点恢复可读则自动回归正常采集（不与内核强绑定）；
- 【临时】注释与 `diag_active()` 门控保持不变；horae 的 `Path::exists` 开销低，不改。

## 改动三：软限幅豁免带——只做数据量化，不改代码

「cap85 决策满档」已确认是 `cpu_load_governor.rs:544-558` 的刻意设计（`current_perf >= free_above` 豁免，防决策卡死，内核温控兜底）。重构涉及调度语义，等数据支持：

- 派一个数据整理代理（只整理不分析）：cap85 时段（15:49–16:26）逐秒对照 tick 决策值与 snap 实测，统计豁免带内/外簇占比、满档簇的实际负载，产出 `devimpbin/0926-162821/cap85-band.md`；
- 结论回填 `device-todo.md` T8，是否重构豁免带由你基于数据决定。

## 收尾

- `cargo check -p chiri --target aarch64-linux-android`，确认输出含 `Checking chiri`、0 warning；
- device-todo.md 增补本轮改动说明（core_ctl enable 感知为持久改动，非【临时】）；
- 无 scratch 残留。
