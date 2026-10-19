# freqbench_tensor 批量适配：拓扑核对结论与一处修正

> 2026-10-20，随 39 机型批量适配产出。核对方式：**设备实测**（`results.json` 的 policy 首核 + `run.log` 的
> `Frequency domains:` 行）对照**内核 DTS**（mainline `qcom,freq-domain` / `cpu-map`）。
> 结论仅关乎**簇边界**；核心名（Cortex-A5x / Kryo）只作背景，不影响本批落表。

## 判据优先级（重要）

`Frequency domains: cpu1 cpu4 cpu7` 这一行是**设备真实暴露的 cpufreq 域**，比 DTS 可靠：

- DTS 的 `cpu-map clusterN` 是**缓存层级**（L2 域），不等于 cpufreq 域。msm8953 的 DTS 有
  `cluster0 = cpu0-3` / `cluster1 = cpu4-7` 两簇，但真机 `run.log` 只报 **`Frequency domains: cpu1`**
  —— 单 policy，两簇是 L2 域不是调频域。故本批 msm8953 按**单簇**落表，与设备一致。
- `bench.py::init_cpus` 枚举 `/sys/devices/system/cpu/cpufreq/policyN`，`policy0` 因
  `REPLACE_CPUS{0:1}` 打印成 `cpu1`。因此 `cpu1` 出现在列表里 = **policy0 存在**，
  不是「边界在 cpu1」。

## 核对结果：36 个可用机型

**边界全部与权威来源一致**，唯一需要修正的是 lga（见下）。

内核 DTS `qcom,freq-domain` 与推导逐条吻合：

| SoC | DTS freq-domain（逐 CPU） | 推导（little/big/prime） |
| --- | --- | --- |
| sm8250 / sm8150 / sm8350 | 0-3 / 4-6 / 7 | 0-3 / 4-6 / 7 ✅ |
| sdm670 / sc7180 | 0-5 / 6-7 | 0-5 / 6-7 ✅ |
| sdm845 | 0-3 / 4-7 | 0-3 / 4-7 ✅ |
| msm8996 | 0-1 / 2-3 | 0-1 / 2-3 ✅ |
| sm6115 / sm6125 | 0-3 / 4-7 | 0-3 / 4-7 ✅ |
| exynos7885（对照） | cluster0=6 核 / cluster1=2 核 | 6 / 2 ✅ |

`sm8350.dtsi` 的 `compatible` 直接写明 4×`arm,cortex-a55` + 3×`arm,cortex-a78` + 1×`arm,cortex-x1`，
是最强的一条独立佐证（ARM 原厂核名而非高通营销名）。

## 修正：lga（Tensor G5）4 个 clock domain 的截取错误

**症状**：lga 的 `results.json` 有 **4 个 policy**（`Frequency domains: cpu1 cpu2 cpu5 cpu7`），
而生成器原按「取前三个 policy」落表 → little 0-1 / big 2-4 / **prime 5-7**。

**后果**：真超大核在**最后一个 policy**（cpu7，21 档，最高 **3782000 kHz**），而 cpu5 属
cpu2/cpu5 共表的中间档（22 档，最高 3052000 kHz）。原表把 prime 错落成 cpu5-7（3 核），
等于**把 X4 整簇丢掉、prime 永久封顶在 3052000 kHz**，前沿表在该段完全失真。

**修法**（`devimpbin/batch_ingest.py::ranges_of` 与 `devimpbin/gen_soc_config.py::topology`）：
≥3 个 policy 时改为 **首 policy = little、末 policy = prime、中间全部并进 big**，
不再截前三个。修正后 lga = little 0-1（2 核）/ big 2-6（5 核）/ prime 7（1 核，21 档 533000-3782000），
指纹 `b3212e1739e53030`，41 个前沿点。

**影响面已证为零**：3-policy 机型（`starts[-1] == starts[2]`）走同一分支、结果不变 ——
`sm7250ab / sm8150 / sm8250 / sm8350 / zuma` 全部仍 `--check: 一致（逐字节相同）`。

**约束记录**：中间各 policy 的频表必须一致，「并进 big」才不做近似。已加
`middle_tables_consistent()` 检查；当前 lga 的 cpu2 与 cpu5 都是 22 档 400000-3052000，一致。

## 遗留（不影响本批）

- `exynos7880`：边界 0-4 / 5-7 与设备一致；但 Exynos 7880 是**同构 8× Cortex-A53**
  （非 big.LITTLE），5+3 是时钟域划分。本批 `soc.yaml` 只写「cpus X-Y（N 核）」**未声称核名**，
  故无错误陈述；日后若写注释勿套用 big.LITTLE 措辞。
- `sdm630`：本地 MIDR 为 0x800/0x801（Kryo 2XX Gold/Silver），而 SD630 官方是全 A53 无 Gold 簇。
  边界 4+4 正确，核名存疑（样本可能非真 SD630 或部件被 mask）。同样未写入核名断言。
- `sdm439 / sm7150 / sm6150 系 / sm7250ab`：mainline 缺对应 dtsi（404/502）或未取到，
  **边界仅由设备实测支撑**，非「权威+实测双证」。
- 本批 36 个机型的**核名**一律未落表，只有边界；核名核对留给后续。
