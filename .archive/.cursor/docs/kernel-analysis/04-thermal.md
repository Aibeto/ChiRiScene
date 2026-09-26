# 内核热框架与温度传感器路径分析（OnePlus 11 / SM8550，Linux 5.15 GKI + QCOM msm）

> 2026-09-26，只读分析 `android_kernel_oneplus_sm8550/`。所有行号以仓库内该内核树为准。

## 结论先行

1. **tsens 读数单位是毫摄氏度（m°C）**：SM8550 走 tsens v2.6 路径，寄存器原始值为 0.1°C（deci°C）带符号码，驱动 ×100 转成 m°C；`/sys/class/thermal/thermal_zone*/temp` 直接回显该 m°C 值。ChiRi 读到的 temp 除以 1000 才是 °C。
2. **`soc_max` 在整个内核树中不存在**（Grep 全树 0 命中），主线索：内核有 `virtual-sensor.c`（`qcom,vs-sensor`，VIRT_MAXIMUM 聚合多个传感器取 max）——真机 DTB（不在本树）里大概率用它定义了 `soc_max` 聚合温区。ChiRi 名单里的 `soc_max` 是**聚合值（多传感器取 max）**，其余 `cpuss0-3` / `cpu0-7-*-thermal` 是单传感器值。
3. **内核侧 governor**：GKI defconfig 只启用 `user_space` + `power_allocator`（`gki_defconfig:431-432`）；CPU zone 的 trip 都是 passive 且无 cooling-maps，真正的 CPU 压频由 Android userspace thermal-engine（thermal-genl 事件 + qti userspace cdev）完成，不是内核 step_wise。
4. **LMh**：硬件自治限频层（`msm_lmh_dcvs.c`），在 tsens→cpufreq 之间直接给 cluster 施加 freq cap，10ms 轮询硬件上限，不经任何 governor——ChiRi 的 `max_freq_khz` 决策会被它硬夹住。

---

## 1. 温度来源：tsens 读数语义

SM8550 = Kalama 平台，tsens IP 是 v2.6。两条驱动路径：

**实际生效路径（msm-tsens / tsens2xxx）**
- 匹配链：`msm-tsens.c:130-131` compatible `"qcom,tsens26xx"` → `data_tsens26xx`（`tsens2xxx.c:922`）→ `tsens2xxx_get_temp`（`tsens2xxx.c:140`）。
- 读寄存器：`tsens2xxx.c:299` `last_temp = code & TSENS_TM_SN_LAST_TEMP_MASK`（12 bit，mask `0xfff`，符号位 `0x800`，`tsens2xxx.c:49-53`）。
- 转换：`tsens2xxx.c:79` `*temp = last_temp * TSENS_TM_SCALE_DECI_MILLIDEG`，其中 `TSENS_TM_SCALE_DECI_MILLIDEG = 100`（`tsens2xxx.c:54`）——**寄存器是 deci°C，输出 m°C**，含符号扩展。
- sysfs 输出：`thermal_sysfs.c:46` 直接 `sprintf("%d", temperature)`，即 `/sys/class/thermal/thermal_zone*/temp` 单位 = **m°C（0.001°C）**。

**主线兼容路径（tsens.c，供对照）**
- `tsens.c:141-171` `tsens_hw_to_mC`：同样 deci°C ×100 → m°C（`sign_extend32(temp, resolution) * 100`）。

**ChiRi 口径**：`/sys/class/thermal/thermal_zone*/temp` 读出的数是 m°C；若 ChiRi 内部以 0.1°C 存，除以 100；以 °C 存则除以 1000。

## 2. 真机 DT 的 thermal zone 清单与 `soc_max` 出处

- **本内核树没有 SM8550 板级 dts**：`arch/arm64/boot/dts/qcom/` 只有到 sm8350 的主线 dts（Glob 无 sm8550*），OnePlus 设备 dts 不在此树。
- **`soc_max` 全树 0 命中**（drivers + arch 均无）。它的来源只有两种可能：
  1. 真机 DTB（vendor 仓库）里的 **virtual sensor 聚合温区**：`drivers/thermal/qcom/virtual-sensor.c:151-152` 定义 `"qcom,vs-sensor"`，`virtual-sensor.c:42-72` 按 `qcom,logic = VIRT_MAXIMUM` 对若干命名传感器（`sensor-names`）取 **max** 后作为新温区注册——`soc_max` 这类名字典型由此生成。**（未确认具体聚合了哪几个传感器，需真机 `/sys/class/thermal/thermal_zone*/type` + 对应 DTB 验证）**
  2. userspace thermal-engine 的配置文件（JSON）定义的同名逻辑温区——但它不会出现在 `/sys/class/thermal`，既然 ChiRi 真机能看到 `soc_max` zone，virtual-sensor 路径可能性更高。
- **主线 sm8550.dtsi 的 zone 清单**（对照本仓库 `mdocs/sm8550.dtsi`，它是主线风格提取件）：`aoss0/1/2`、`cpuss0/1/2/3`（`6139-6228`，trip：thermal-engine-config 125°C passive + reset-mon 115°C）、`cpu3..7-top/bottom/middle`（`6229-6492`，90/95/110°C）、`cpu0/1/2-thermal`（`6511-6582`，90/95/110°C，110 是 critical）、`cdsp0-3`、`video`、`mem`、`modem0-3`、`camera0/1`、`gpuss-0..7`（GPU zone 绑 `<&gpu NO_LIMIT>` cooling，`6929-7193`）。
- **对 ChiRi 名单的验证**：
  - `soc_max`：真机存在（ChiRi 实测），聚合温区，**保留且应为首选**——它是整机最热点的保守代表。
  - `mtktscpu`（MTK）、`cpu-1-`、`cpu-0-0-usr`（旧 QCOM/其他设备命名）：与本机无关的兼容条目，保留无妨。
  - **可考虑补**：主线命名 `cpuss0`–`cpuss3`（cluster 级单传感器）与 `cpu0`–`cpu7-*-thermal`——若真机 DTB 直用主线命名，这些是逐核真实温度；`soc_max` 缺失时的回退源。

## 3. 谁在压频：governor 与 cooling device

**governor（`drivers/thermal/`）**
- 编译进内核的只有（`arch/arm64/configs/gki_defconfig:426-436`）：`THERMAL=y`、`THERMAL_GOV_USER_SPACE=y`、`THERMAL_GOV_POWER_ALLOCATOR=y`、`CPU_THERMAL=y`、`DEVFREQ_THERMAL=y`。
- step_wise / fair_share / bang_bang 源码在（`gov_step_wise.c` 等）但 defconfig 未选；Kconfig 默认 choice 是 step_wise（`drivers/thermal/Kconfig:92`），实际 .config 取决于厂商 defconfig（**不在本树，未确认**）。但 DT 里 CPU zone 的 trip 全是 passive 且**无 cooling-maps**（主线 dtsi 的 cpu* zone 无 map 节点），即使 governor 跑起来也没有 CPU cooling 绑定可驱动——**CPU 压频不由内核 governor 完成**。
- GPU 例外：`gpuss-*` zone 绑定 `<&gpu THERMAL_NO_LIMIT>`（`mdocs/sm8550.dtsi:6937` 等），GPU 热压走内核 power_allocator/devfreq 通道。

**CPU cooling device（`drivers/thermal/qcom/`）**
- `qti_cpufreq_cdev.c:149` 注册 cpufreq 类 cooling device（`thermal_cooling_device_register`），供 userspace/policy 驱动。
- `thermal_pause.c`：CPU pause cooling（`QTI_CPU_PAUSE_COOLING_DEVICE`），可整组暂停 CPU。
- `cpu_hotplug.c`、`cpu_voltage_cooling.c`、`cx_ipeak_cdev.c`、`ddr_cdev.c`、`qti_devfreq_cdev.c`、`qti_userspace_cdev.c`（userspace 直接控制态 cooling）等，QTI 全家桶齐备，编译与否取决于厂商 defconfig（未确认）。
- **没有经典 msm_thermal**（老平台每 tick 扫温度写 freq 的那套）；替代物是下面的 LMh DCVS。

**LMh DCVS（`msm_lmh_dcvs.c`，`QTI_THERMAL_LIMITS_DCVS`）**
- 每个 cluster 一个 `limits_dcvs_hw`，`core_map` 绑核，`LIMITS_POLLING_DELAY_MS = 10`（`msm_lmh_dcvs.c:43`）10ms 轮询硬件 freq cap（`LIMITS_FREQ_CAP`，`msm_lmh_dcvs.c:40`；cap 值以 19.2MHz 为基，`msm_lmh_dcvs.c:45-49`），注册 cooling device 并对 cpufreq 设上限；默认温度 `LIMITS_TEMP_DEFAULT = 75000`（`msm_lmh_dcvs.c:41`）。

## 4. 用户态热控接口

- **thermal-genl netlink 存在且启用**：`gki_defconfig:427` `CONFIG_THERMAL_NETLINK=y`；`thermal_netlink.c:17-19` 两个多播组 `thermal_genl_sampling`（温度采样推送）与 `thermal_genl_event`（tz/trip/cdev 事件：`THERMAL_GENL_EVENT_TZ_TRIP_UP/DOWN` 等，`thermal_netlink.c:209-221`）。Android thermal-engine 订阅 event 组收 trip 越限通知，据此下发 cooling。
- **userspace cooling device**：`qti_userspace_cdev.c` 提供 userspace 直接写 state 的 cdev；配合 `THERMAL_GOV_USER_SPACE`，userspace 是 CPU 热策略的实际决策者。
- **trip 数值**：CPU zone 90/95/110°C（critical），cluster/aoss 级 115/125°C；`THERMAL_WRITABLE_TRIPS=y`（`gki_defconfig:430`），userspace 可改 trip。
- 板级 dts 里 CPU 无 `cooling-device = <&cpu...>` 绑定（主线 dtsi 无此节点）；GPU 绑 `<&gpu>` 全档位。**（真机 vendor dts 可能有补充，未确认）**

## 5. LMh 在热链路里的位置

一句话：**LMh 是硬件自治限频器**——`lmh.c` 只是其 IRQ 控制器驱动（`lmh.c:204-215` 注册 irq domain，中断使能源于 cpufreq ready），`msm_lmh_dcvs.c` 把 LMh 硬件算出的 freq cap 每 10ms 读出并套到 cpufreq 上；它在 tsens 温度→限频决策这条链里绕过内核 governor/thermal-engine，是 ChiRi 之上（更靠近硬件）的最后一道硬限制。

---

## 对 ChiRi 的含义

1. **单位**：`/sys/class/thermal/thermal_zone*/temp` = m°C。ChiRi 的 `cpu_temp` 若按 0.1°C 或 °C 解释需按此换算；阈值比较（如 85°C → 85000）要用 m°C。
2. **首选 zone**：`soc_max` 保留——它最可能是 virtual-sensor 的**多传感器取 max 聚合值**，天然保守、适合热保护；但要注意它是"最热点"而非"CPU 平均温"。`cpuss0-3` / `cpu*-thermal` 是单传感器真值，若真机存在可作回退或细分源。**名单是否新增**：建议加 `cpuss`（匹配 `cpuss0-3`）；`mtktscpu`/`cpu-1-`/`cpu-0-0-usr` 在本机永远命不中，属跨设备兼容条目。
3. **分层关系**：热链共四层——LMh 硬件限频（最硬，75°C 默认起步，10ms 粒度）→ userspace thermal-engine（netlink 事件 + userspace cdev，90/95°C trip 起压）→ 内核 governor（user_space/pa，CPU 侧无 cooling-map 实际不动作）→ **ChiRi 软件热保护（最外层，超阈值压 CLG 上限）**。ChiRi 压的 `max_freq_khz` 若低于 LMh/thermal-engine 的 cap，实际频率由更硬的那层决定——日志里 "decision=down 但 cur_freq 不动" 的 thermal clamp 即来自 LMh/thermal-engine 通道，不是 ChiRi 写频失败。
4. **聚合值警示**：`soc_max` 是 max 聚合（可能含 GPU/CDSP 等非 CPU 传感器），拿它当 `cpu_temp` 会把非 CPU 热点也算进 CPU 降压依据，偏保守；若需纯 CPU 温度应落到 `cpu*-thermal`/`cpuss*` 单传感器 zone。（聚合成员清单未确认，需真机 DTB。）
5. **电池温度**：走 PMIC 侧（`qcom-spmi-adc-tm5.c` / `bcl_soc.c` 通道），不在 tsens；ChiRi 的电池温度源独立于本报告的 tsens 链路。

## 未确认项

- 真机 `soc_max` zone 的 DTB 定义与聚合成员（vendor dts 不在本树）。
- OnePlus 实际 .config（厂商 defconfig 不在本树）：step_wise 是否同时编译、qti_*_cdev 哪些被启用。
- 真机 `/sys/class/thermal/thermal_zone*/type` 全量名单（建议真机 `ls /sys/class/thermal/*/type` + `cat` 抓一次存档）。
