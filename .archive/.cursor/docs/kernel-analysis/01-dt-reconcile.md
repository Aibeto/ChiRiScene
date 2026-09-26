# 厂商真机 DT vs 上游 mainline dtsi 对账报告（SM8550 / OnePlus 11）

日期：2026-09-26
对象：`android_kernel_oneplus_sm8550`（分支 `oneplus/sm8550_b_16.0.0_oneplus_11`，HEAD `b9bdf47513b9`，"Synchronize code for OnePlus CPH2447/2449/2451/PHB110 16.0.5.702/701, 基于 QCOM TAG AU_LINUX_KERNEL.PLATFORM.2.0.R1.00.00.00.004.146"）
对照基准：`mdocs/sm8550.dtsi`（上游 mainline 7202 行本地副本）

---

## 结论（先行）

1. **真机设备树不在内核仓库，且从未在仓库历史中出现过。**
   - HEAD 顶层无 `qcom/` 目录；`git ls-tree HEAD qcom/`、`qcom/proprietary/`、`qcom/proprietary/devicetree/` 均为空。
   - `git log --all --oneline -- qcom/proprietary/devicetree` 提交计数 = **0**；连 `-- qcom` 顶层也是 **0**（全历史、全分支、全 tag 无任何提交触碰过该路径）。
   - `arch/arm64/boot/dts/vendor` 是 mode `120000` 的符号链接（blob `e4ccfdab`），目标 `../../../../../qcom/proprietary/devicetree`——即指向**仓库之外的 Android vendor 构建树**，OnePlus 未随源码放出。
   - 其余 21 个 OnePlus SM8550 分支（t_13.x / u_14.x / v_15.x / b_16.x 系列）抽查 `sm8550_t_13.1.0_oneplus11` 的 `qcom/` 同样为空，均为同一形态的裸内核。
2. **内核树内 `arch/arm64/boot/dts/qcom/` 共 167 个文件，止于 sm8350**（grep `sm8450|sm8550` 无命中），不含 Kalama 设备树。
3. **真机 FAS 日志的 `cpu_capacity = 280/855/1024` 在本仓库内未找到直接出处（未确认）。**
   内核侧机制是 `drivers/base/arch_topology.c:256` `capacity = raw_capacity[cpu] * freq_factor`，`raw_capacity` 完全来自 DT 的 `capacity-dmips-mhz`/`cpu-capacity`（`arch_topology.c:284-295`）。厂商 DT 缺失 → 真机值的 DT 来源无法在本仓库验证；仅能确定**它不等于 mainline 的 326/693/1024**，即厂商 DT 改写了该属性（或厂商 DT 定义了不同数值）。
4. **真机 thermal zone type `soc_max` 同样不在内核源码内**：`git grep soc_max HEAD -- drivers/thermal/ include/` 为空；驱动里 `soc_max` 仅命中 AMD vega10、电池 fg-alg/qpnp-qg、USB gadget 等无关文件。`soc_max` 是厂商 DT 的 thermal-zone type 命名，属热管理策略定制，mainline `thermal-zones`（`sm8550.dtsi:6138`）中不存在。
5. 其余可对账项：CPU OPP 频率集合真机与 mainline 一致（既有实测 scaling_available_frequencies），电压/增删档未确认；idle-state、cpufreq freq-domain、GPU OPP、LLCC、interconnect 全部**未确认**（同因：厂商 DT 不在仓库）。

**对账方法定性：本仓库只能对账「mainline 基准」，无法对账「真机 DT」本身。要拿到真机真值必须反编译设备 dtb。**

---

## 文件清单（探索证据）

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| HEAD 顶层 | `git ls-tree HEAD --name-only` | 无 `qcom/`；有 `arch/ drivers/ include/` 等 |
| dts 目录 | `git ls-tree HEAD arch/arm64/boot/dts/` | `vendor` 为符号链接（120000, blob `e4ccfdab`）；`qcom` 是普通 tree（0a9b0493） |
| 链接目标 | `git show e4ccfdab` | `../../../../../qcom/proprietary/devicetree`（仓库外） |
| vendor DT 历史 | `git log --all --oneline -- qcom/proprietary/devicetree` | **0 个提交** |
| `qcom/` 顶层历史 | `git log --all --oneline -- qcom` | **0 个提交** |
| 全分支抽查 | `git ls-tree remotes/origin/oneplus/sm8550_t_13.1.0_oneplus11 qcom/` | 空 |
| 树内 sm8550 检索 | `git ls-tree -r HEAD --name-only \| findstr sm8550 kalama salami` | 仅 configs / build.config / clk-icc-pinctrl 驱动 / dt-binding 头，**无任何 dts/dtsi** |
| qcom dts 上限 | `git ls-tree -r HEAD --name-only arch/arm64/boot/dts/qcom/`（167 个）+ findstr | 最高 sm8350-hdk/mtp/sm8350.dtsi |
| 内核侧容量机制 | `git grep raw_capacity HEAD -- drivers/base/arch_topology.c` | `arch_topology.c:256,262,293` raw_capacity 来自 DT |
| 内核侧 soc_max | `git grep soc_max HEAD -- drivers/thermal/ include/` | 空（thermal 驱动无此字符串） |
| cpufreq 驱动 | `git grep epss HEAD -- drivers/cpufreq/qcom-cpufreq-hw.c` | 仅通用 `qcom,cpufreq-epss` compatible（:523），无 sm8550/kalama 专用条目 |

---

## capacity 对账

- **mainline**：`capacity-dmips-mhz` 326（silver，cpu0-2）/ 693（gold，cpu3-6）/ 1024（prime，cpu7）；`dynamic-power-coefficient` 251/447/1057。引用：`mdocs/sm8550.dtsi:80-81`（cpu0）、`169-170`（cpu3）、`281-282`（cpu7）。
- **真机（FAS 日志实测）**：cpu_capacity = **280 / 855 / 1024**。
- **对账结论**：二者不符 → 厂商 DT 对 capacity 做了真机化改写。本仓库内核源码与 DT 中均无 280/855/1024 的定义处（未确认出处）；内核读法只有 DT 一条路径（`arch_topology.c` raw_capacity），排除驱动硬编码。
- 注意归一化口径：内核会把 capacity-dmips-mhz 归一化到最大核 1024（`arch_topology.c:256` 的乘法与归一），真机日志 280/855/1024 恰以 prime=1024 为基准，**与「DT 原始值即 280/855/1024」或「DT 原值成 280:855:1024 比例」两种可能均兼容**，需 dtb 反编译区分。

## OPP 对账

- **CPU**：mainline `cpu0_opp_table`（`sm8550.dtsi:458`）little 16 档 307200–2016000；`cpu3_opp_table`（:541 附近）20 档 499200–2803200；`cpu7_opp_table`（:648）21 档 595200–3187200。
  真机 `scaling_available_frequencies` 与该集合一致（既有实测），说明**频率档位无增删**；电压（opp-microvolt）与 turbo 档取舍**未确认**。
- **GPU**：mainline `gpu_opp_table`（`sm8550.dtsi:2889`）注释注明 A740+ speedbin 未完成、仅保留低频档。真机 gpu_opp_table 是否增删档**未确认**。

## thermal 对账

- **mainline**：`thermal-zones`（`sm8550.dtsi:6138`）含 cpuss0-3-thermal（:6157/:6175/:6193/:6211，tsens0 1–4）等，trip 统一 125000 mC / passive（如 :6162）。
- **真机**：thermal zone type 名含 `soc_max`。本仓库 thermal 驱动无该字符串（见结论 4），说明热区命名/类型/策略在厂商 DT 层定制（Qualcomm 热引擎 thermal-engine 的 zone 命名传统即由厂商 DT 定义）。
- zone 全名清单、trip 温度、cooling-map 均未确认 → 需 dtb 反编译。

## idle-states / cpufreq_hw 对账

- **mainline**：idle `silver 550/750/6700µs`（:338-346）、`gold 600/1300/8136`（:348-356）、`prime 500/1350/7480`（:358-366）；簇级 `cluster_sleep_0 750/2350/9144`（:370-376）、`cluster_sleep_1 2800/4400/10150`（:378-384）；`cpufreq_hw: cpufreq@17d91000` 三 freq-domain（:5875-5879，compatible `qcom,sm8550-cpufreq-epss`）。
- **真机**：驱动侧 `qcom-cpufreq-hw.c` 只有通用 epss compatible（无 SoC 定制条目），freq-domain 划分差异只可能在 DT；idle-state 数值是否被厂商改动**未确认**。ChiRi 现有观察（idle 数值与 mainline 一致的印象）可在设备侧用 `/sys/devices/system/cpu/cpu*/cpuidle/state*/{latency,residency}` 直接核验。

## 其他（简要）

- **LLCC / interconnect / PSCI**：内核含 `drivers/interconnect/qcom/kalama.c`、`include/dt-bindings/interconnect/qcom,kalama.h`、`drivers/clk/qcom/*-kalama.c` 等驱动与 binding 头，说明厂商内核按 Kalama 编译；但对应 DT 节点定义不在仓库，真机 LLCC slice 表、BCM/路由表、PSCI suspend 参数与 mainline 的差异**未确认**。
- 附带发现：同类厂商 proprietary devicetree 有公开先例（如 GitHub `xiaomi-8550-kernel/vendor_qcom_proprietary_bt-devicetree`），说明「vendor DT 独立仓库」是常见发布形态；OnePlus 未放出对应仓库。

---

## 对 ChiRi 的含义

1. **获取真机 DT 的唯一可靠路径：反编译设备 dtb。** 首选 `adb pull /sys/firmware/fdt`（root 设备），或从刷机包 boot/vendor_boot 镜像提取 dtb；然后 `dtc -I dtb -O dts -o kalama_op11.dts`。反编译产物放入 `mdocs/`（不进内核仓库）后，本报告的 capacity/OPP/thermal/idle 全部小节可升级为实证对账。
2. **ChiRi 不应把 mainline dtsi 当真机真值**：cpu_capacity 用真机实测 280/855/1024；若 Rust 代码里有任何 326/693/1024 或 251/447/1057 的硬编码/推导，应以实测为准修正（`schedutil` 能量模型、EAS 置信度都会被错误 capacity 拉偏）。
3. **thermal 读取逻辑需按真机 zone 命名适配**：zone type 含 `soc_max`，且真机 zone 清单与 mainline 不同，ChiRi 读温度的 zone 选择不能照搬 mainline 名单。
4. **频率档位可放心按 mainline 集合使用**（与真机 scaling_available_frequencies 一致），但**电压/功耗推算参数不可**。
5. 后续若拿到 dtb 反编译结果，优先核对四处：CPU 节点 capacity/dpc、thermal-zones type+trip+cooling-map、CPU/GPU OPP 表（含 microvolt）、idle-state latency/residency——即本报告标记「未确认」的全部条目。
