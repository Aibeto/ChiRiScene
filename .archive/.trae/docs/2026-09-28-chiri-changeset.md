# 2026-09-28 ChiRi 改动清单（8550 能效计划落地）

来源：`.codebuddy/plans/d924b09fabb343f991a86c153e57a5e5/plan.md`（v19）。
贯穿口径：除 SceneMode 外任何模式不设频率天花板；省电改由门槛、升频审批、稳态下探承担；
结论只能来自逻辑必然、模型折算、功能回归观察（本项目禁止 A/B 对照）。
编译门槛：`cargo check -p chiri --target aarch64-linux-android` 通过（零 rustc 警告）。

---

## 一、配置

| 文件                                    | 改动                                                                                                                                                                                                            | 依据                                                 |
| --------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| `feature.yaml`（兜底）/ `8475` / `8998` | `reduce`、`default` 的 `perf_ceil` 全部改为 `1.0`                                                                                                                                                               | 天花板全拆                                           |
| `8550/feature.yaml`                     | `reduce.perf_ceil 0.60 → 1.00`；`background_uclamp_max_pct 35 → 25`；新增 Affinity 开关与 `tuning` 段；新增 FDP 四项；`reduce` 与 `default` 加 `steady_decay_target_ratio`（0.80 / 0.90），`boost` 关掉稳态下探 | 天花板全拆、A1 降权、A7 常量可配、FDP 开关、模式取点 |
| `normal/tuned_profiles.yaml`            | akmode `perf_ceil 0.95 → 1.0`；playback `headroom 0.85 → 1.0`，三簇 `per_cluster` 天花板整段删除                                                                                                                | 同上                                                 |
| `normal/scenemode.yaml`                 | 不动（`perf_ceil 0.12` 是 R1 允许的唯一例外轴）                                                                                                                                                                 | —                                                    |
| `8550/soc.yaml`                         | 新增 `frontier_policy` 受控生成块（46 桶 × 三簇目标频点 + 逐档功耗）；文件头注释同步契约变更                                                                                                                    | 前沿结果向量表                                       |
| `i18n/{zh,en}.ftl`                      | 新增 8 个键（CLG-PF 四条、corectl 两条、fps attach 汇总、FDP 一条）                                                                                                                                             | 打点可读                                             |

`aarch32_clusters` **缺省留空 = 32 位线程落点不受限**：主流系统带转译层，32 位程序实际能跑全核，
预测式拦截会把它们误锁在小核。只有确认无转译的机型才填（如 `["little"]`），填了才恢复「32 位只落这些簇」
的写前拦截。位宽判定与 `elf32` 观测帧照常，内核真拒（EINVAL）时的 `pin_incapable` 兜底也照常。

---

## 二、代码

**新增 `src/chiri/energy_cost.rs`**——FDP 的成本模型。文件头有 `//!` 区块索引，内部按
`[source] [power] [supply] [cost] [probe] [decision] [tests]` 分块。核心是三个纯函数：
`cluster_power_w`（逐档插值）、`freq_for_policy_util`（schedutil 口径的频率映射）、
`net_benefit_w`（双边净收益）。表不手抄，读 `soc_frontier_policy()`。簇到 cpufreq policy 的映射只发现一次
（policy 目录开机后固定，热插拔不增删 policy），此后每轮只读 3 个 `scaling_cur_freq`，不再扫 policy0..16。

**`src/chiri/cpu_load_governor.rs`**——T11 稳态下探与 CLG-PF 落点。
下探是对写侧偏移的限幅积分控制，被控量是 util；只在 `flush()` 减去偏移，不回写 `current_perf`。
触摸窗口内不叠加下探（否则会把落点压到触摸地板之下）。PF 落点 `min(比例路径, 前沿目标)`，只下调，
仅稳态查表，缺数据回退比例路径，日志前缀按 SoC 打 `CLG-PF` 或 `CLG`。

**`src/chiri/tuned.rs`**——同款下探；新增 `up_confirm_ticks` 严格升频审批。
下探参与目标档计算，但不改 up/down 的判定基准。

**`src/chiri/affinity.rs`**——亲和线 A3/A6/A7/A8/A10 与 FDP 接线。
后台 promote 不进 prime；big 饱和保护推广到所有会 promote 的模式；32 位线程写掩码前拦截；
17 个放置常量可配（缺省等于原 const，三道时间闸钳制下界就是现值）；首见 32 位线程落 `elf32` 帧、
每 25 tick 汇总写失败；掩码写去重 + 同目标最短重写间隔；FDP 只在后台 promote 的目的簇选择上生效。

**`src/common.rs`**——`SocConfig.frontier_policy` 与 `Frontier*` 结构（全字段 `#[serde(default)]`），
`[frontier_lenient]` 保证该段损坏只令其自身为 `None`，不拖垮 topology/capacity/freq 兜底。

**`src/chiri/config.rs`**——新增字段与 `normalize` 钳制。

**缺陷修复**：`fas_manager.rs` 把 `governor.activate()` 前移到 `load_policies()` 之前并补齐早退路径
（退出后不再残留 `performance`），`perfmgr_enable` 按快照恢复；`core_ctl.rs` 在 `enable == Some(false)`
时不写 `min_cpus` 并按 `e0` 打点；`fps_monitor.rs` 累计 attach 失败数与最后原因。

---

## 三、工具与表

`scripts/frontier_policy.py`（离线生成器，不进设备、不进 `cargo build`）。本轮扩展：

- 每个需求桶直接带 `little_khz/big_khz/prime_khz` 与真机容量单位边界 `lo_units/hi_units`；
- `per_cluster.<簇>` 带 `capacity + freq_khz + power_w`，生成时断言「文档 §3 档位数 == soc.yaml 档位数」；
- 指纹移入块内，新增 `--disable`，写盘固定 LF，读入前先剥掉生成块再解析；
- 复算 7 项锚点全过（46 点前沿、起手 3652.8 Mdmips/W、prime 冲顶段 599.946），两次运行逐字节一致。

表的唯一权威是 `mdocs/8550/sm8550-freq-power.md` §3 → 生成器 → `8550/soc.yaml` → 运行期。
**任何一侧都不许手抄数值。** 非 8550 的 `soc.yaml` 没有该段，`soc_frontier_policy()` 返回 `None`，
CLG 与 FDP 都自动走原逻辑。

---

## 四、我拒绝、改动或补上的计划项

| 计划项                              | 处置             | 理由                                                                               |
| ----------------------------------- | ---------------- | ---------------------------------------------------------------------------------- |
| F7 清理 `CoreCtl.scenemode_offline` | 拒绝             | 该字段被 `deny` 解析链需要，删掉会让含该键的旧 yaml 解析失败；注释已写明「已停用」 |
| F4 缩短 `deactivate_delay_secs`     | 拒绝改数值       | 缩短会提高「短暂离开被误判为真退出」的概率并引发进出抖动，计划自己标为未验证候选   |
| F5 息屏模式不降级                   | 判定有意保持     | 息屏只改屏幕状态，FAS 接管与屏幕解耦、特调保 akmode 都是刻意的                     |
| A2/A9 统一 uclamp                   | 只复核不改       | 8998 为 0、8475/8550 为 85，各机型本就不同且无实测依据统一                         |
| T1–T6 直接删字段                    | 改为写明「取点」 | `reduce` 丢 `perf_ceil 0.60` 后与 `default` 几乎无差别，按 R7 用取点保住省电档差异 |
| FDP                                 | 按用户指示实现   | 我原先以「前置未满足 + 表自洽不等于真机正确」拒绝，用户明确要求做                  |
| `modes` / `anticipate` 消费         | 未做             | 依赖「流畅度下界」与「预判提前量」两个占位口径先定稿                               |

FDP 实现过程中我打回并让它修掉两处模型缺陷：一是原本用「簇总算力过原点反解」频率（假设需求均摊到全簇，
会把单核的 prime 判成免费）；二是 prime 的候选资格不能交给成本函数（静态表不含漏电与 idle-exit，
且判不出计划 P-b 的门槛），仍由 `bg_promote_exclude_prime` 把关。

---

## 五、上线后先看什么

拆天花板会让视频与游戏的稳态频率抬升、功耗回升，这是主动代价，T11 只能补回一部分
（`boost` 档因为 `perf_ceil` 拆前拆后都是 1.0，行为与拆除前一致）。先看视频场景同场景功耗与热档触发频率。

FDP 一侧看日志 `action=fdp` 里的 `net` 与真机 `batt_power_w` 是否同号；再用自算例
（little u=0.5 / big u=0.3 / prime u=0，d=168）比对模型落点与实测 `scaling_cur_freq`。

逐项回退动作（触发条件 + 字段级改法）在 `.trae/memory/2026-09-28.md` 的回退表；
口径与机制说明在 `agentsdocs/03-chiri.md`，内核事实在 `agentsdocs/06-kernel.md`。
