# zumapro 帕累托前沿适配设计

日期：2026-10-19
范围：建 zumapro 能效模型文档、泛化 `frontier_policy.py`、落 `frontier_policy` 块、启用 CLG-PF / FDP。

## 0. 为什么 zumapro 和 8550 不是一回事

8550 那张表是推出来的：`Cx·f·V²`，系数借用 AYN Odin 2（QCS8550）EAS 补丁的电压，逐档功耗全是计算值，没有一台真机量过。zumapro 不一样，它有实测数据，而且是两个独立来源。

第一个来源是 `freqbench/results/zumapro/main/results.json`。这份数据有个容易看漏的地方：它没有 `housekeeping` 键，而 kdrag0n 的 `bench.py:471` 无条件会写这个键（msm8998、sm8150 都有）。再看 `idle.power_mean`，prime 恒为 0.0 mW，little 19.8 到 38.7 mW。整机差分不可能出现 prime 空闲恒为 0（msm8998 的 idle 是 1289 到 1329 mW）。所以这批数据是用 tensor 分支的 PMIC ODPM 能量计跑的，`versions.txt` 里的 `sultan@sultan-box` 与 tensor 仓独有的 `lga`（Tensor G5）是同一人。它被提交进了 kdrag0n 仓，但产出方法是 tensor 的。

这点很重要：ODPM 逐轨净能量与 ChiRi 的 `Cx·f·V²` 同口径，都是 CPU 动态功耗，不含屏幕、DDR、GPU。8550 那张表想达到的就是这个口径，只是它是算出来的。

第二个来源来自联网查到的内核源码。kerneltoast 的 `android_kernel_google_zumapro` 仓库（分支 `16.0.0-sultan`，已归档）里有 `google-devices/zumapro/dts/zumapro-a1-foplp.dts`，第 432 行的 `pixel_em` profiles 是 Google 自报的逐档功耗表。

## 1. 联网核对的结论：文档里有三处记错

### 1.1 DT 容量记的是 Tensor G3 的数

`agentsdocs/06-kernel.md:36` 记 `capacity-dmips-mhz` 为 193 / 927 / 1024。这三个数一字不差地出现在 `zuma-cpu.dtsi:55,91,127`，那是 Tensor G3，不是 zumapro。

zumapro 的真值在 `zumapro-cpu.dtsi:52,88,115`：

| 簇 | 文档记的（实为 G3） | zumapro 真值 |
| --- | --- | --- |
| little | 193 | 449 |
| big | 927 | 850 |
| prime | 1024 | 1024 |

按文档自己写的折算口径（`dmips × f_max` 归一到 1024）：

| 路径 | little | big | prime |
| --- | --- | --- | --- |
| zumapro DT 折算 | 282 | 712 | 1024 |
| freqbench 实测 | 283 | 714 | 1024 |

差 0.4%。两条独立路径闭合，选型不再有悬念。文档那组数折出来是 121 / 776 / 1024，little 差 2.3 倍。

顺带 dpc 也记错了，文档记 70 / 513 / 757，DT 实际是 176 / 325 / 748。

### 1.2 频表有源码级铁证，但 prime 下限对不上

`zumapro-cpu.dtsi:260-269` 的 `dsu-bci-constraint-table` 逐行列出 CL0 十档：

```
1950000 1849000 1696000 1548000 1425000 1328000 1197000 1098000 955000 820000
```

与 freqbench cpu1 的 10 档逐字节一致。big 17 档、prime 18 档也逐簇一致。

分歧在 prime：`zumapro-cpu.dtsi:292` 写 `min-freq = <500000>`，而 freqbench 与 Google EM 都从 700000 起。500 MHz 这一档固件没有暴露出来。文档记的「prime 700–3105」是对的，DT 声明的 500 MHz 不能当可用档位。

### 1.3 Google 自报的功耗表与实测差一个常数倍率

`pixel_em` 三列经 `pixel_em.c:656` 的 `sscanf` 顺序确认是 `freq | capacity | power`，单位由 `include/linux/energy_model.h:88` 的 `EM_MAX_POWER = 64000000`（微瓦）钉死。1950 MHz 那行第三列 `220000` 是 220 mW，freqbench 实测 204 mW。

比值（EM 除以实测）：

| 簇 | 中位 | 区间 |
| --- | --- | --- |
| little | 0.93 | 0.76 到 1.08 |
| big | 1.86 | 1.63 到 2.00 |
| prime | 1.85 | 1.72 到 1.91 |

big 和 prime 恒定在 1.85 附近，说明形状几乎一致，只差一个常数倍率（Google 的表可能含静态分量，或者工作点定义不同）。little 有正负 15% 漂移，低频 EM 偏低、高频偏高，正好落在漏电最重的那一簇。

EM 的 capacity 列在 f_max 档是 188 / 731 / 1024，与实测 283 / 714 / 1024 相比 little 差 33%，big 和 prime 一致。所以 EM 的容量列不能直接当 `cpu_capacity`。

## 2. 已定的五个口径

| 项 | 决定 | 理由 |
| --- | --- | --- |
| 适配深度 | 全链路 | 只有落块才被运行期消费 |
| 功耗口径 | 漏电校正：`n×active − (n−1)×idle` | 与 `Cx·f·V²` 同口径。小簇 4 核 820 MHz 从 225 mW 降到 166 mW，big 约 3%，prime 无差 |
| 容量 | freqbench 实测折算 283 / 714 / 1024 | DT 折算与实测差 0.4%，三重验证 |
| 算力单位 | CMdmips（CoreMark 折算） | 走实测，不引入 `capacity × f/f_max` 假设。与 8550 的 Mdmips 不可横向比 |
| 功耗来源 | freqbench 实测为主，Google EM 作旁证 | 实测独立第三方量的；EM 高估约一倍但形状一致 |

小数点后说明一下算力单位为什么安全：运行期 `demand_units()`（`cpu_load_governor.rs:773`）是 `Σ(util × capacity)`，桶边界 `build_buckets()`（`frontier_policy.py:216`）用 `combo_units()`。下凸包按 CMdmips 排，桶边界按真机容量单位排，两者每簇比例系数不同，排序有可能不一致。实测过：zumapro 38 个前沿点的 `combo_units` 严格递增，零逆序对（1001.0 到 4298.0），桶不会错位。

## 3. §3 簇级表

数据来源 `freqbench/results/zumapro/main/results.json`，漏电校正后逐簇如下。big 簇 1795 MHz 是实测坏点，保留原值并标注，下凸包枚举验证过它零次入选。

#### little 簇（A520 ×4）

| 频率 kHz | 簇功耗 W | 簇算力 CMdmips |
| -------- | -------: | ------------: |
| 820000 | 0.1661 | 15344.3 |
| 955000 | 0.2019 | 17900.8 |
| 1098000 | 0.2415 | 20540.4 |
| 1197000 | 0.2674 | 22433.3 |
| 1328000 | 0.3152 | 24919.0 |
| 1425000 | 0.3543 | 26727.2 |
| 1548000 | 0.4077 | 29022.0 |
| 1696000 | 0.4784 | 31770.0 |
| 1849000 | 0.6031 | 34761.4 |
| 1950000 | 0.7015 | 36550.3 |

#### big 簇（A720 ×3）

| 频率 kHz | 簇功耗 W | 簇算力 CMdmips |
| -------- | -------: | ------------: |
| 357000 | 0.1020 | 9487.6 |
| 578000 | 0.1663 | 15394.4 |
| 648000 | 0.1875 | 17245.3 |
| 787000 | 0.2305 | 20939.5 |
| 910000 | 0.2746 | 24198.4 |
| 1065000 | 0.3378 | 28369.2 |
| 1221000 | 0.4187 | 32505.1 |
| 1328000 | 0.4795 | 35363.5 |
| 1418000 | 0.5255 | 37767.5 |
| 1549000 | 0.6136 | 41240.9 |
| 1795000 | 1.3543 | 47806.2 |
| 1945000 | 1.1286 | 51776.2 |
| 2130000 | 1.4057 | 56730.4 |
| 2245000 | 1.5997 | 59818.6 |
| 2367000 | 1.8580 | 63151.2 |
| 2450000 | 2.0478 | 65264.7 |
| 2600000 | 2.5816 | 69281.4 |

#### prime 簇（X4 ×1）

| 频率 kHz | 簇功耗 W | 簇算力 CMdmips |
| -------- | -------: | ------------: |
| 700000 | 0.1588 | 7476.7 |
| 1164000 | 0.2846 | 12403.6 |
| 1396000 | 0.3820 | 14883.9 |
| 1557000 | 0.4680 | 16613.1 |
| 1745000 | 0.5757 | 18621.4 |
| 1885000 | 0.6733 | 20087.7 |
| 1999000 | 0.7529 | 21263.0 |
| 2147000 | 0.8893 | 22889.4 |
| 2294000 | 1.0526 | 24451.9 |
| 2363000 | 1.1507 | 25168.8 |
| 2499000 | 1.3707 | 26641.8 |
| 2687000 | 1.7218 | 28658.8 |
| 2802000 | 1.9930 | 29861.1 |
| 2914000 | 2.3921 | 31041.4 |
| 2943000 | 2.4681 | 31356.2 |
| 2970000 | 2.5046 | 31690.7 |
| 3015000 | 2.6556 | 32154.3 |
| 3105000 | 3.0277 | 33092.5 |

## 4. 前沿复算结果

3060 个组合（10 × 17 × 18）枚举出 38 点下凸包。算力 32308.6 到 138924.2 CMdmips，功耗 0.4269 到 6.3108 W。首段边际能效 91863.1 CMdmips/W，顶段 2521.4 CMdmips/W。

形状合理：起点三簇全在最低档（820 / 357 / 700 MHz），终点三簇全满频（1950 / 2600 / 3105 MHz）。prime 在需求 82666.5 CMdmips 之前一直压在 700 MHz，之后才逐级抬升，这与 8550 的 P-b 结论（单核算力需求超过阈值才启用 prime）一致。

各簇效率峰值（ULPMark-CM）：

| 簇 | 峰值 | f_max 处 | 倍率 |
| --- | --- | --- | --- |
| little | 820 MHz @ 68.0 I/mJ | 1950 MHz @ 44.7 | 1.52 倍 |
| big | 578 MHz @ 90.9 I/mJ | 2600 MHz @ 26.8 | 3.39 倍 |
| prime | 700 MHz @ 47.1 I/mJ | 3105 MHz @ 10.9 | 4.31 倍 |

## 5. 生成器泛化

`scripts/frontier_policy.py` 现有 504 行，写死了 8550。要改的地方：

| 位置 | 现状 | 改法 |
| --- | --- | --- |
| 37-40 行 | `DOC` / `SOC` / `OUT_NEW` 三个常量 | 新增 `--soc` 参数，按名字拼路径 |
| 172 行 | `CLUSTER_CORES = {little:3, big:4, prime:1}` | 从 soc.yaml 的 `[topology]` 区间长度算；zumapro 是 4 / 3 / 1 |
| 55-63 行 | `ANCHORS` 七个值写死 | 按 SoC 分组；zumapro 用第 4 节实测值 |
| 67 行 | `PRIME_TOP_FROM_KHZ = 2092800` | 按 SoC 分组；zumapro 是 3015000 |
| 454 行 | `counts != {little:16, big:20, prime:21}` | 改成与文档解析结果对账，不写死档数 |
| 429 行 | `--target` 文案写死 8550 | 文案随 `--soc` 变 |
| 277 行 | `fingerprint` | 不变（只哈希输入内容，天然多 SoC） |

锚点对账的容差 `ANCHOR_TOL = 0.03` 保留。zumapro 的锚点值取第 4 节实测结果：`compute_min` 32308.6、`compute_max` 138924.2、`power_min` 0.4269、`power_max` 6.3108、`hull_points` 38、`first_seg` 91863.1、`last_seg` 2521.4。

回归要求：泛化后重跑 8550，块必须逐字节不变（现有指纹 `92bcedf210e6aed1`）。

## 6. 落块与启用

`module/config/zumapro/soc.yaml` 现在 48 行，`[capacity]` 和 `[freq]` 都是空 TODO。要填：

```
[capacity] little 283 / big 714 / prime 1024
[freq]     little 10 档 / big 17 档 / prime 18 档（第 3 节表格第一列）
```

`[capacity]` 上方那段「勿把 DT 值直接填进来」的警告要改写：警告本身仍然成立（DT 的 `capacity-dmips-mhz` 与 sysfs `cpu_capacity` 不同量纲），但要补上真值来源，说明 449 / 850 / 1024 折算后是 282 / 712 / 1024，与实测 283 / 714 / 1024 一致；原文档记的 193 / 927 / 1024 是 Tensor G3 的数，已作废。

`[freq]` 的注释要写明 prime 起始档是 700000 而不是 DT 声明的 500000。

块落进 soc.yaml 后，`module/config/zumapro/feature.yaml:44-45` 的 FDP 开关可以打开，参数对齐 8550（`fdp_cost_hysteresis_w: 0.02`、`fdp_max_inserts_per_round: 2`、`fdp_dst_freq_cap_ratio: 0.85`）。

同步要改的文档：`agentsdocs/06-kernel.md:36` 的容量与 dpc、`agentsdocs/07-todos.md` 的 zumapro 条目。

## 7. 待办与风险

`--lead-frac`（占位 0.30）和 `--reserve`（reduce 0 / default 0.10 / boost 0.25）两个参数仍是 TODO，zumapro 沿用同样的占位值，不单独拍板。

`modes` 和 `anticipate` 段会生成但目前没有运行期消费方，与 8550 现状一致。

CLG-PF 的真机验证项（稳态降频不增加掉帧、触摸窗口内 PF 关闭）在 zumapro 上同样要做，且 zumapro 的调速器是 `sched_pixel` 而不是 waltgov，写入是否会被改写需要单独确认。

还剩一个没核实的点：Google EM 的 `cam1` 到 `cam3`、`game` 这几个 profile 的功耗比 `default` 高一到三成（比如 little 820 MHz 从 43000 变 56000 µW）。这说明 EM 表是随场景切换的，而 freqbench 只在 default 下测。

按 default 处理，理由是绝对功率不参与决策、参与的是相对能效。核过代码，这个理由要分两条说：

**CLG-PF：绝对功耗完全不参与。** `frontier_lookup()`（`cpu_load_governor.rs:797`）调 `table.bucket_for_demand(demand)`，落到 `common.rs:531` 的二分查找，只读 `lo_units`，再取该桶的三个 `*_khz`。整条链路不碰 `power_w`。桶里的 `target_power_w` 和 `margin_mdmips_per_w` 在 `src/` 下没有任何读取方（此前那个 `target_power_w` 读点属于已删除的功耗调频器模块自己的配置项，与前沿表无关）。所以 CLG-PF 的判据就是「等算力下的相对能效」。

**FDP：比较的是差值，但阈值带绝对量纲。** `net_benefit_w()`（`energy_cost.rs:229`）算 `(p_src_after - p_src_now) + (p_dst_after - p_dst_now)`，四项都是差值。但 `energy_cost.rs:427` 的门槛是 `net < 0.0 && net <= -hysteresis_w`，`hysteresis_w` 取 `fdp_cost_hysteresis_w: 0.02` 瓦，是绝对值。

结论仍然不变，理由换一个：各 profile 相对 default 近似同比缩放（cam1 的 1.3 倍在小簇各档相当一致），差值同比放大只会让门槛更容易过；而 0.02 W 本来就是拍的工程值，不是物理常数。所以 default 是同一条曲线上的保守取样点。
