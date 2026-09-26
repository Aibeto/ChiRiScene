# SM8550 内核 DT 对账：厂商真机 DT vs 上游 mainline dtsi

日期：2026-09-26 ｜ 对象：`android_kernel_oneplus_sm8550`（OnePlus 11，Linux 5.15.180，分支 oneplus/sm8550_b_16.0.0_oneplus_11）
结论先行：**本次检查未能在内核检出中找到任何厂商 DT 源码**——任务核心问题（280/855/1024 是否在厂商 dts 里有实证）**在本仓库内无法验证**，原因与证据见下。

## 一、文件清单与 include 链（检出内实际情况）

- `arch/arm64/boot/dts/qcom/`：只有上游 mainline 系 dts/dtsi，最高到 `sm8350.dtsi`。**没有 sm8550 的任何 dts/dtsi，没有 salami/oneplus11/CPH2451/PHB110 板级文件**（`dir /b` 全量清单 + `git ls-files | findstr salami` 均为空）。
- `arch/arm64/boot/dts/vendor`：是一个 **symlink**（git blob 120000，内容一行）：指向 `../../../../../qcom/proprietary/devicetree`。
- 该目标目录 `qcom/proprietary/devicetree` **在本工作区不存在**（`dir` 报系统找不到路径）；`git ls-tree HEAD qcom` 为空（HEAD 树中无此条目、非 gitlink），`.gitmodules` 不存在。即厂商 DT 属于另一个随 QCOM AU/OnePlus 同步流程单独拉取的 repo，本内核检出未包含。
- `dts/Makefile`（23、35–37 行）：`subdir-y += vendor` 仅当 `$(dtstree)/vendor/Makefile` 存在才编入——构建时也是靠外部 repo 注入。
- git 顶 3 条提交（b9bdf47513b9 等）为 OnePlus 同步提交（CPH2447/2449/2451、PHB110，基于 AU_TAG ...004.146），但未带来 DT 源。

**mainline 参照**：`mdocs/sm8550.dtsi`（本地副本，7202 行），关键行号：CPU0–7 节点 76–295 行；CPU idle-states 340–378 行；`gpu_opp_table` 2889 行；cpufreq_hw 5880–5887 行；`thermal-zones` 6138 行起，cpuss0–3-thermal 6157–6211 行。

## 二、capacity 对账

- mainline（mdocs/sm8550.dtsi）：little `capacity-dmips-mhz = <326>` / `dynamic-power-coefficient = <251>`（80–81、113–114、141–142 行）；big 693/447（169–170 等）；prime 1024/1057（281–282 行）。
- 厂商 DT：**未确认**——源文件不在检出内，无从查找 280/855/1024。
- 因此「真机 FAS 实测 cpu_capacity = 280/855/1024 ← 厂商板级 DT 覆盖 mainline 326/693/1024」这一因果链在 DT 源层面仍是**假设**（方向合理：280/855/1024 与高通下游常见标定风格一致，但本次无文件证据，勿写成已证实）。
- cache 层级（next-level-cache → l2/l3）：mainline 每 CPU 挂 L2、簇共享 L3（76/94 行等）；厂商改动**未确认**。

## 三、OPP 对账

- mainline CPU OPP 表与真机 `scaling_available_frequencies` 已逐档核对一致（little 16 档 307200–2016000 / big 20 档 499200–2803200 / prime 21 档 595200–3187200，见 `.codebuddy/docs/2026-09-26-sm8550-opp-ladder.md` 与 8550/soc.yaml 注释）——即厂商在 CPU 频率档位上**没有增删**（真机侧实证）。
- OPP 电压差异：**未确认**（mainline 本地副本无 regulator 微调信息可比，厂商 DT 缺失）。
- GPU：mainline `gpu_opp_table` 在 2889 行；真机档位未在本次范围内核对，**未确认**。

## 四、thermal zones 对账

- mainline：`thermal-zones`（6138 行）含 cpuss0-thermal … cpuss3-thermal（6157/6175/6193/6211 行）等；具体 trip 温度与 cooling-map 本次未逐行摘录（如需可后补）。
- `soc_max`：**mainline sm8550.dtsi 中不存在**（全文 grep 无命中）。真机 thermal zone type 含 `soc_max`，说明该 zone 来自厂商（Oplus/QCOM 下游）DT 或 Oplus 驱动注册——具体在哪个文件哪行**未确认**（厂商 DT 缺失）。ChiRi 温度探测名单把 `soc_max` 当合法探测目标是对的，继续保留。

## 五、cpufreq_hw / idle-states

- freq-domain（mainline，79/112/140/168/196/224/252/280 行）：CPU0–2 → domain 0，CPU3–6 → domain 1，CPU7 → domain 2，cpufreq_hw 三域（5880 行 reg-names freq-domain0/1/2）。与真机 policy0/3/7、soc.yaml topology（little 0–3 / big 3–7 / prime 7–8）完全吻合；无厂商改动的实证（也未发现反证）。
- idle-states（mainline 340–378 行）：silver/gold/goldplus-rail-power-collapse 每 CPU 态 + cluster-sleep-0/1 两级簇态；soc.yaml 的 idle_us（750/1300/1350、6700/8136/7480、簇 2350/4400）即出自此处。厂商是否改过延迟参数：**未确认**。

## 六、其他（L3/LLCC、interconnect、PSCI）

厂商 DT 缺失，以上各项均**未确认**。mainline sm8550.dtsi 有 L3 cache 节点与 LLCC/interconnect 定义（本次未展开），若后续拿到厂商 DT 应优先比对这些 + capacity。

## 七、对 ChiRi 的含义

1. **soc.yaml 不需要改**：capacity 280/855/1024 的依据是「真机 FAS 日志实测」而非「mainline dtsi」，本次结果不削弱它，继续以真机值为兜底（soc.yaml 注释里「厂商板级 DTS 覆盖了 mainline 值」一句建议改弱为「厂商值，DT 源未及比对，来源待证」）。
2. **要修正的推断**：此前计划里若把「去厂商 dts 找 280/855/1024」当作可执行步骤，应改为运行时取证——真机上读 `/sys/firmware/devicetree/base/cpus/cpu@*/capacity-dmips-mhz`（十六进制字节序）与 `/proc/device-tree` 一锤定音，比翻内核源码 dump 可靠且成本极低。
3. `mdocs/sm8550.dtsi` 作为 mainline 基准继续有效（freq-domain、OPP 档位、idle-states 三块与真机一致的部分可直接当基准）；capacity/dpc/thermal trip 则**不能**当真机基准用。
4. `soc_max` 来自厂商侧（mainline 无此 zone）——ChiRi 温度探测按真机 zone 名匹配的路线正确。
5. 若将来需要厂商 DT：去拉 `qcom/proprietary/devicetree` 对应 repo（OnePlus 11 通常在 device/oplus/ 或 vendor 侧的 dtb repo），本内核检出永远不会包含它。

---
*本次终端调用 3 次（目录清单×2 + git 树核对/清理×1），scratch `devimpbin/tmp_kern_dt.txt` 已删除；内核目录零写入。*
