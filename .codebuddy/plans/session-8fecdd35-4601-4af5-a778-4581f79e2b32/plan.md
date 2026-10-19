# zumapro 帕累托前沿适配实施计划

> **For agentic workers:** 按任务顺序执行，每步都有验证命令与预期输出，不要跳步。步骤用 `- [ ]` 勾选跟踪。

**Goal:** 为 zumapro 落地逐档能效模型文档、泛化前沿生成器、生成并落 `frontier_policy` 受控块、打开 FDP，使 CLG-PF 与 FDP 在 Pixel 9 Pro 上生效。

**Architecture:** 数据链不变，仍是「能效模型 md → `scripts/frontier_policy.py` 离线复算 → `soc.yaml` 受控块 → 运行期只查表」。本次改动把生成器从 8550 单例泛化成 `--soc` 驱动的多 SoC 工具，新增一份 zumapro 数据文档，并在不触碰 8550 生成块一个字节的前提下为 zumapro 产出独立的受控块与 FDP 开关。

**Tech Stack:** Python 3 纯标准库（生成器，不进设备、不进 cargo build）；Rust（运行期消费方已存在，本次只改注释）；YAML 受控块；`cargo check -p chiri --target aarch64-linux-android` 验证。

---

## 背景：设计文档与本计划的关系

设计规格在 `.codebuddy/docs/2026-10-19-zumapro-frontier-design.md`（已自查通过）。本计划把规格里已经定下的口径落成可执行步骤。规格里有一处需要按本计划修正：§4/§5 的公布值取自早期草稿，与最终表格复算有 0.1% 量级偏差；本计划 Task 2 会把规格数字与最终文档、生成器锚点三者对齐到精确复算值。

## 审查中发现的问题（本计划已纳入，读代码时先看这一节）

1. **生成器当前已不可运行（P0 回归）。** commit `b136f30` 把 `mdocs/8550/` 改名为 `mdocs/sm8550/` 时，只同步了 `scripts/frontier_policy.py` 里的两处**注释/字符串**（第 11 行 docstring、第 294 行块内注释），漏掉了第 38 行的**功能常量**。`python3 scripts/frontier_policy.py --check` 现在直接退出：
   ```
   缺少输入: /home/aibeto/code/ChiRi/mdocs/8550/sm8550-freq-power.md
   ```
   （exit code 1）。Task 0 修它。

2. **`parse_soc()` 的 capacity 正则会在 zumapro 上抓错段。** 现有正则 `^\s*{c}:\s*(\d+)\s*$` 是全文件首个匹配。zumapro 的 `[idle]` 段是块状写法：
   ```yaml
   idle_us:
     exit_latency:
       little: 160
   ```
   而 8550 的 `[idle]` 是行内流式（`exit_latency: { little: 750, ... }`），所以 8550 侥幸只匹配到 `capacity`。实测把 zumapro 现状文本喂进现正则，得到 `little=160, big=190, prime=220`（就是 exit_latency 的三个值，错段）。Task 1 把解析改成按段取。

3. **`render_block()` 里有多处写死的 8550 字样与数字**：`# 前沿点（46 点…`（第 305 行）是字面量 46，不是 `len(hull)`；`compute=Mdmips(DT 口径)`、功耗注释「只含动态项、不含漏电」都是 8550 口径。这些对 zumapro 全错，且 zumapro 的表**是含漏电并已校正**的。Task 1 一并参数化。

## 数据落点与最终实测值（本计划全部数字的出处）

全部由 `freqbench/results/zumapro/main/results.json` 复算，口径 `cluster_W = n×active − (n−1)×idle`（漏电校正），`CMdmips = coremarks_per_mhz × f_MHz × 核数 ÷ 1000`。本会话已核对它复算 8550 时能逐字重现 `soc.yaml` 块内的 7 个 got 值（`2293.7 / 13005.8 / 0.535 / 8.436 / 46 / 3652.778 / 599.946`），复算逻辑忠实。

| 项 | 值 |
| --- | --- |
| 档位数 | little 10 / big 17 / prime 18 |
| 核数 | little 4 / big 3 / prime 1（来自 `soc.yaml [topology]`） |
| 容量（freqbench 折算，prime=1024） | little 283 / big 714 / prime 1024 |
| 容量（DT 折算，交叉验证） | little 282 / big 712 / prime 1024 |
| 前沿点数 | 38 |
| 算力区间 | 32308.6 – 138924.2 CMdmips |
| 功耗区间 | 0.4269 – 6.3108 W |
| 起手段边际能效 | 91863.1 CMdmips/W |
| 顶段边际能效（`prime_top_from_khz = 3015000`） | 2521.4 CMdmips/W |
| `big 1795000` 坏点入选次数 | 0 |
| prime 首次离开 700 MHz | 在第 14 个前沿点，前一点算力 77739.6 |
| 桶边界逆序对（38 点，283/714/1024） | 0，`combo_units` 1001.0 – 4298.0 |

容量取整说明：实测 big 为 **714.60**，`round()` 得 715，规格写的是 714。本计划采用规格值 714（截断），因为该值只在真机 `cpu_capacity` 读不到时作兜底、且 714/715 两种取法下桶边界都是零逆序对（实测 1001.0–4298.0 与 1001.4–4301.0）。若要求数学取整，Task 3 把 714 改 715 即可，无需改本文档其他部分。**这一项需用户确认。**

---

## 文件结构

| 文件 | 动作 | 职责 |
| --- | --- | --- |
| `scripts/frontier_policy.py` | 改 | 泛化为 `--soc` 驱动；修 path 常量与 capacity 段落取法 |
| `mdocs/zumapro/zumapro-freq-power.md` | 新建 | zumapro 逐档能效模型（生成器唯一数据源，§3 三张表） |
| `module/config/zumapro/soc.yaml` | 改 | 填 `[capacity]`/`[freq_khz]`、修 `[dpc]`、落受控块 |
| `module/config/zumapro/feature.yaml` | 改 | 打开 FDP 三参数 |
| `.codebuddy/docs/2026-10-19-zumapro-frontier-design.md` | 改 | §4/§5 数字对齐精确复算值 |
| `agentsdocs/06-kernel.md` | 改 | 修 zumapro 容量/dpc/调频器三处记录 |
| `agentsdocs/07-todos.md` | 改 | zumapro 台账去掉「无前沿表」，登记新缺口 |
| `mdocs/socList.md` | 改 | zumapro 帕累托前沿勾上与 8550 并列 |
| `src/chiri/energy_cost.rs`、`src/common.rs`、`src/main.rs` | 改注释 | 「仅 8550」类断言已失效 |

写作用域（互不重叠）：`scripts/`、`mdocs/`、`module/config/zumapro/`、`.codebuddy/docs/`、`agentsdocs/`、`src/`。除 Task 6 的注释外不动 `src/`。

---

## Task 0: 修生成器的输入路径常量（P0，先修这个）

**Files:**
- Modify: `scripts/frontier_policy.py:38`

- [ ] **Step 1: 复现故障**

```bash
cd /home/aibeto/code/ChiRi && python3 scripts/frontier_policy.py --check
```

Expected: FAIL，`缺少输入: /home/aibeto/code/ChiRi/mdocs/8550/sm8550-freq-power.md`，exit code 1。

- [ ] **Step 2: 改成真实路径**

`scripts/frontier_policy.py:38` 当前内容：

```python
DOC = ROOT / "mdocs" / "8550" / "sm8550-freq-power.md"
```

改为：

```python
DOC = ROOT / "mdocs" / "sm8550" / "sm8550-freq-power.md"
```

（Task 1 会把这一行整体搬进 `SOC_PROFILES`。本步先只改路径，让基线恢复可运行，便于拿到干净的回归对照。）

- [ ] **Step 3: 验证 8550 基线恢复且块逐字节未变**

```bash
cd /home/aibeto/code/ChiRi && python3 scripts/frontier_policy.py --check; echo "EXIT=$?"
```

Expected: 打印 `输入: mdocs/sm8550/sm8550-freq-power.md / module/config/8550/soc.yaml`、`档位: {'little': 16, 'big': 20, 'prime': 21}  组合: 6358  前沿点: 46  桶: 46`、`指纹: 92bcedf210e6aed1`、7 项对账全 `[OK ]`、`--check: 一致（逐字节相同）`，`EXIT=0`。

- [ ] **Step 4: 记录回归基线**

把两个值抄进 Task 1 的验收对照：指纹 `92bcedf210e6aed1`、块逐字节一致（`--check` exit 0）。**Task 1 改动后必须仍然满足这两条。**

---

## Task 1: 泛化生成器为 `--soc` 驱动

**Files:**
- Modify: `scripts/frontier_policy.py`（第 37-40、104-124、171-182、185-211、289-356、425-500 行区块）

- [ ] **Step 1: 写解析层的失败用例（先测再改）**

在仓库内临时目录跑断言，不新增测试框架（本仓库 Python 侧无测试入口，生成器自带的 `--check` 是主回归门）：

```bash
cd /home/aibeto/code/ChiRi && python3 - <<'PY'
import re
from pathlib import Path
text = Path("module/config/zumapro/soc.yaml").read_text(encoding="utf-8")
# 现正则：全文件首个匹配
for c in ("little", "big", "prime"):
    m = re.search(rf"^\s*{c}:\s*(\d+)\s*$", text, re.M)
    print(f"现正则 {c}: {m.group(1) if m else None}")
PY
```

Expected: FAIL 语义。打印 `little: 160`、`big: 190`、`prime: 220`。这正是 `[idle] exit_latency` 的值，**不是** capacity。

- [ ] **Step 2: 改 docstring 顶部（第 6-29 行区块）**

把「输入（只读）」两条改为按 SoC 描述，并写明新参数：

```python
"""frontier_policy.py —— 帕累托前沿"结果向量"离线生成器

区块索引: [args] [soc] [parse_doc] [parse_soc] [frontier] [buckets] [modes] [emit] [check] [main]

定位（与计划 3.4 一致）:
  开发期离线工具，**不进设备、不进 cargo build、不被运行期调用**。设备侧只"查表"，不跑本脚本。
  用途: 读能效模型表 → 复算帕累托前沿 → 算出 reduce/default/boost 三模式的结果向量 → 输出可嵌入的 YAML 块。

输入（只读，按 --soc 从 [soc] 段取）:
  能效模型 md   §3 的三张"簇级"表，列 = 频率 kHz | 簇功耗 W | 簇算力（单位随 SoC 变）
                （不自己乘核数: 用文档给出的簇级值，避免二次换算误差）
  soc.yaml      capacity / freq_khz / topology（核数由 topology 区间长度算）

输出（按 --soc 决定落点）:
  默认 dry-run: 摘要 + YAML 块打到 stdout
  --write        受控块替换写入目标文件（块内替换、**块外逐字保留**；无标记块时只追加）
  --check        读取现有块并逐字节比对（确定性自检；与生成内容不一致则退出码 1）
  --soc          目标机型键（见 [soc] 的 SOC_PROFILES）；缺省 8550 保持历史行为

约定（审查修正项）:
  * 纯标准库，不依赖 pyyaml（本机 pyyaml 有损坏史）
  * fingerprint 只由输入内容哈希决定，**不含生成时刻**（否则重跑字节不同）
  * 禁止整文件 dump 回写（会丢注释、属机械重排）
  * 解析必须**按段取**，不能用「全文件首个匹配」——zumapro 的 [idle] 段是块状写法，
    段内也有 little/big/prime 整数键，全文件匹配会抓到 idle 的值而不是 capacity

TODO: 两个口径待用户拍板，先取占位值并可用 CLI 覆盖——
      `--lead-frac` 预判基准提前量（占需求桶跨度的比例，default 基准；reduce ×0.5 / boost ×2 已定）
      `--reserve`   流畅度下界（需求之上预留算力的百分比，三模式各自一份）
"""
```

- [ ] **Step 3: 用 SoC 档案表替换 `DOC`/`SOC`/`OUT_NEW`/`ANCHORS`/`PRIME_TOP_FROM_KHZ`**

替换第 37-67 行整块为：

```python
ROOT = Path(__file__).resolve().parent.parent

BLOCK_BEGIN = "# >>> frontier_policy (generated) >>>"
BLOCK_END = "# <<< frontier_policy <<<"

CLUSTERS = ("little", "big", "prime")
# 预判提前量的模式乘子（用户已定量: default 为基准）
MODE_LEAD_MULT = {"reduce": 0.5, "default": 1.0, "boost": 2.0}
DEFAULT_LEAD_FRAC = 0.30  # TODO: 基准提前量待拍板（占需求桶跨度比例）
DEFAULT_RESERVE = {  # TODO: 流畅度下界待拍板（需求之上预留算力 %）
    "reduce": 0.0,
    "default": 0.10,
    "boost": 0.25,
}
ANCHOR_TOL = 0.03


# [soc] ---------------------------------------------------------------------
# 每 SoC 一份输入与对账档案。脚本其余部分与 SoC 无关，故新增机型只改本段。
# 新增 SoC = 加一条：doc（能效模型，§3 需有 `#### little 簇` 形态的三张表）、
#            soc（module/config/<soc>/soc.yaml，需有 [topology] [capacity] [freq_khz]）、
#            out_new（--target new 的落点）、prime_top_from_khz（顶段边际能效起点档）、
#            anchors（文档公布值，容差 ANCHOR_TOL）、compute_unit（块注释里的算力单位名）、
#            power_comment（块注释里的功耗口径说明，两行）。
SOC_PROFILES = {
    "8550": {
        "doc": ROOT / "mdocs" / "sm8550" / "sm8550-freq-power.md",
        "soc": ROOT / "module" / "config" / "8550" / "soc.yaml",
        "out_new": ROOT / "module" / "config" / "8550" / "frontier_policy.yaml",
        # 文档 §5 原话："prime 冲顶段（2092800→3187200 只剩 600 Mdmips/W）" —— 指的是**整段摊薄**的边际，
        # 不是顶部单档（2956800→3187200 单档只有约 470 Mdmips/W）。锚点按整段口径取。
        "prime_top_from_khz": 2092800,
        "anchors": {
            "compute_min": 2294.0,
            "compute_max": 13006.0,
            "power_min": 0.535,
            "power_max": 8.436,
            "hull_points": 46,
            "first_seg_mdmips_per_w": 3655.0,
            "last_seg_mdmips_per_w": 600.0,
        },
        "compute_unit": "Mdmips(DT 口径)",
        "power_comment": [
            "  # 各簇频表 + 逐档**簇功耗**(W)：功耗列直接取文档 §3 簇级表，供放置成本模型做边际比较；",
            "  # 两列等长且逐档对齐（同下标 = 同一档）。功耗只含动态项、不含漏电，**只能用于相对排序**。",
        ],
    },
    "zumapro": {
        "doc": ROOT / "mdocs" / "zumapro" / "zumapro-freq-power.md",
        "soc": ROOT / "module" / "config" / "zumapro" / "soc.yaml",
        "out_new": ROOT / "module" / "config" / "zumapro" / "frontier_policy.yaml",
        # prime 末两档 3015000 → 3105000 的整段边际（文档 §5）
        "prime_top_from_khz": 3015000,
        "anchors": {
            "compute_min": 32308.6,
            "compute_max": 138924.2,
            "power_min": 0.4269,
            "power_max": 6.3108,
            "hull_points": 38,
            "first_seg_mdmips_per_w": 91863.1,
            "last_seg_mdmips_per_w": 2521.4,
        },
        "compute_unit": "CMdmips(CoreMark 实测折算)",
        "power_comment": [
            "  # 各簇频表 + 逐档**簇功耗**(W)：功耗列直接取文档 §3 簇级表，供放置成本模型做边际比较；",
            "  # 两列等长且逐档对齐（同下标 = 同一档）。功耗为 PMIC ODPM 实测净能量、已做漏电校正，**只能用于相对排序**。",
        ],
    },
}
DEFAULT_SOC = "8550"
```

- [ ] **Step 4: 加段落取用辅助函数**

在 `# [parse_doc]` 之前插入：

```python
# [parse_soc] ---------------------------------------------------------------

def section_lines(text: str, name: str):
    """取顶层 `name:` 段落（到下一个列 0 的非注释行前）。

    必需按段取：zumapro 的 [idle] 段写成
        idle_us:
          exit_latency:
            little: 160
    段内也有 little/big/prime 的整数键，用「全文件首个匹配」会抓到 idle 的值当 capacity。
    """
    out, inside = [], False
    for ln in text.splitlines():
        if not inside:
            if re.match(rf"^{re.escape(name)}\s*:", ln):
                inside = True
                out.append(ln)
            continue
        # 段内允许空行、缩进行、列 0 的注释行；列 0 的其他键即段落结束
        if ln.strip() and not ln.startswith((" ", "\t", "#")):
            break
        out.append(ln)
    return "\n".join(out)
```

- [ ] **Step 5: 重写 `parse_soc()`**

替换现有 `parse_soc` 函数体为：

```python
def parse_soc(path: Path):
    """从 soc.yaml 取 capacity / freq_khz / 各簇核数（文本解析，不引入 yaml 库）

    先剥掉生成块再解析：块内 per_cluster/demand_buckets 也含 `little:` 形态的键，
    重复运行时不能让它污染 cap/freq 取值。再按段取，避免抓到 [idle] 段的整数键。
    """
    text = path.read_text(encoding="utf-8")
    b, e = text.find(BLOCK_BEGIN), text.find(BLOCK_END)
    if b >= 0 and e >= 0:
        text = text[:b] + text[e + len(BLOCK_END):]
    cap_text = section_lines(text, "capacity")
    freq_text = section_lines(text, "freq_khz")
    topo_text = section_lines(text, "topology")
    cap = {}
    for c in CLUSTERS:
        m = re.search(rf"^\s*{c}:\s*(\d+)\s*$", cap_text, re.M)
        if m:
            cap[c] = int(m.group(1))
    freqs = {}
    for c in CLUSTERS:
        m = re.search(rf"^\s*{c}:\s*\[(.*?)\]", freq_text, re.S | re.M)
        if m:
            freqs[c] = [int(x) for x in re.findall(r"\d+", m.group(1))]
    cores = {}
    for c in CLUSTERS:
        m = re.search(rf"^\s*{c}:\s*\{{\s*start:\s*(\d+),\s*end:\s*(\d+)", topo_text, re.M)
        if m:
            cores[c] = int(m.group(2)) - int(m.group(1))
    return cap, freqs, cores
```

- [ ] **Step 6: 用 `topology` 核数替换写死的 `CLUSTER_CORES`**

删除第 171-172 行：

```python
# 真机容量单位口径（文档 §2.2）: 单核算力 = cpu_capacity × f / f_max；簇算力 = 单核 × 核数
CLUSTER_CORES = {"little": 3, "big": 4, "prime": 1}
```

替换为：

```python
# 真机容量单位口径（文档 §2.2）: 单核算力 = cpu_capacity × f / f_max；簇算力 = 单核 × 核数
# 核数不写死，由 soc.yaml [topology] 的区间长度算（8550 → 3/4/1，zumapro → 4/3/1）
```

并把 `combo_units` 与 `build_buckets` 改为吃 `cores` 参数：

```python
def combo_units(combo, cap, freqs, cores):
    """频率三元组 → 总算力（真机容量单位）；capacity / f_max / 核数 缺失的簇按 0 计（调用方回退）"""
    total = 0.0
    for cluster, f in zip(CLUSTERS, combo):
        f_max = freqs.get(cluster, [0])[-1] if freqs.get(cluster) else 0
        if f_max:
            total += cores.get(cluster, 0) * cap.get(cluster, 0) * f / f_max
    return total
```

```python
def build_buckets(hull, cap, freqs, cores):
    """前沿点两两之间构成需求桶；桶边界取相邻点算力（**真机容量单位**）的中点

    每个桶直接带该前沿点的三簇目标频点（little_khz/big_khz/prime_khz）——运行期
    查表即可，不用在设备上重算 Cx·f·V²。纯几何定义，不依赖 SLO。
    """
    units = [combo_units(p[2], cap, freqs, cores) for p in hull]
    # …以下循环体逐字不变…
```

（`build_buckets` 的后半段循环体不动，只改签名与 `units` 一行。）

- [ ] **Step 7: 让 `check_anchors()` 吃锚点参数**

替换签名为 `def check_anchors(hull, tables, anchors, prime_top_from_khz):`，并把函数体里的 `PRIME_TOP_FROM_KHZ` 与 `ANCHORS[...]` 换成形参：

```python
def check_anchors(hull, tables, anchors, prime_top_from_khz):
    """复算自检：与文档公布值对账（容差 3%）

    口径易错点（已修）: "冲顶段 600 Mdmips/W" 指的是 **prime 表自己**末两档的边际能效，
    不是下凸包末段（末段可能混簇，值会明显不同）。
    """
    results = []
    xs = [p[0] for p in hull]
    ys = [p[1] for p in hull]
    prime = tables["prime"]
    top_from = next((r for r in prime if r[0] >= prime_top_from_khz), prime[-2])
    top_seg = efficiency(top_from[2], top_from[1], prime[-1][2], prime[-1][1])
    checks = [
        ("总算力下限", min(xs), anchors["compute_min"]),
        ("总算力上限", max(xs), anchors["compute_max"]),
        ("总功耗下限", min(ys), anchors["power_min"]),
        ("总功耗上限", max(ys), anchors["power_max"]),
        ("前沿点数", len(hull), anchors["hull_points"]),
        ("起手段 边际能效", efficiency(hull[0][0], hull[0][1], hull[1][0], hull[1][1]),
         anchors["first_seg_mdmips_per_w"]),
        ("prime 冲顶段 边际能效", top_seg, anchors["last_seg_mdmips_per_w"]),
    ]
    for name, got, want in checks:
        tol = abs(want) * ANCHOR_TOL if want else 0.0
        ok = abs(got - want) <= tol
        results.append((name, got, want, ok))
    return results
```

- [ ] **Step 8: 参数化 `render_block()` 的 SoC 相关行**

替换签名为：

```python
def render_block(tables, cap, freqs, cores, hull, buckets, modes, enabled, prof):
```

函数体内三处替换（其余行逐字不动）：

```python
    L.append(f"# 口径: 能效模型 {prof['doc'].relative_to(ROOT)} §3 簇级表 + 本文件 capacity/freq_khz")
```

```python
    L.append(f"  # 前沿点（{len(hull)} 点；compute={prof['compute_unit']}、power=W）")
```

把原来的两行功耗注释换成：

```python
    for ln in prof["power_comment"]:
        L.append(ln)
```

（`--soc 8550` 时这三处渲染出的字符串与现状**逐字符相同**，故 8550 块保持逐字节不变。）

- [ ] **Step 9: 重写 `main()` 的 SoC 接线与档位校验**

在 `ap.add_argument("--selftest"…)` 之前插入 `--soc`：

```python
    ap.add_argument("--soc", choices=sorted(SOC_PROFILES), default=DEFAULT_SOC,
                    help="目标机型键（见脚本 [soc] 段 SOC_PROFILES；缺省 8550 保持历史行为）")
```

把 `--target` 的 help 改为按 SoC 拼：

```python
    ap.add_argument("--target", choices=("new", "soc"), default="soc",
                    help="soc = 并入本机 soc.yaml（受控块当前落点，默认）;"
                         " new = 独立 frontier_policy.yaml（运行期不消费，仅迁移用）")
```

把「缺输入」检查到 `counts` 告警这一段替换为：

```python
    prof = SOC_PROFILES[args.soc]

    for p in (prof["doc"], prof["soc"]):
        if not p.exists():
            raise SystemExit(f"缺少输入: {p}")

    tables = parse_cluster_tables(prof["doc"])
    cap, freqs, cores = parse_soc(prof["soc"])
    counts = {c: len(tables[c]) for c in CLUSTERS}
    # 不再写死「期望 little16/big20/prime21」：档位数随 SoC 变，只查退化表
    thin = {c: n for c, n in counts.items() if n < 2}
    if thin:
        print(f"⚠ 档位表过稀: {thin}（每簇至少 2 档才有边际能效，先补文档 §3）", file=sys.stderr)
    if any(c not in cores for c in CLUSTERS):
        print(f"⚠ [topology] 缺簇核数: {[c for c in CLUSTERS if c not in cores]}", file=sys.stderr)
```

把后续三处调用改为：

```python
    hull = lower_hull([(x, v[0], v[1]) for x, v in combos.items()])
    checks = check_anchors(hull, tables, prof["anchors"], prof["prime_top_from_khz"])
    n_combos = len(combos)
    buckets = build_buckets(hull, cap, freqs, cores)
    reserve = dict(DEFAULT_RESERVE)
    if args.reserve is not None:
        reserve = {k: args.reserve for k in reserve}
    modes = build_modes(buckets, args.lead_frac, reserve)
    block = render_block(tables, cap, freqs, cores, hull, buckets, modes, not args.disable, prof)
```

把打印与落点两处改为：

```python
    print(f"SoC: {args.soc}")
    print(f"输入: {prof['doc'].relative_to(ROOT)} / {prof['soc'].relative_to(ROOT)}")
    print(f"档位: {counts}  核数: {cores}  组合: {n_combos}  前沿点: {len(hull)}  桶: {len(buckets)}")
```

```python
    target = prof["out_new"] if args.target == "new" else prof["soc"]
```

- [ ] **Step 10: 验证 8550 回归逐字节不变（本任务的核心验收）**

```bash
cd /home/aibeto/code/ChiRi && python3 scripts/frontier_policy.py --soc 8550 --check; echo "EXIT=$?"
```

Expected: `指纹: 92bcedf210e6aed1`、7 项对账全 `[OK ]`、`--check: 一致（逐字节相同）`、`EXIT=0`。**若指纹或逐字节一致性任一不符，停手回查 Task 1 的 Step 6/8，不要继续。**

- [ ] **Step 11: 验证缺省行为不变（不传 `--soc`）**

```bash
cd /home/aibeto/code/ChiRi && python3 scripts/frontier_policy.py --check; echo "EXIT=$?"
```

Expected: 同 Step 10，`EXIT=0`。

- [ ] **Step 12: 验证段落取用修好了（用 zumapro 现状文本）**

此时 `mdocs/zumapro/` 还不存在，只测 `parse_soc` 单元行为：

```bash
cd /home/aibeto/code/ChiRi && python3 - <<'PY'
import importlib.util, sys
spec = importlib.util.spec_from_file_location("fp", "scripts/frontier_policy.py")
fp = importlib.util.module_from_spec(spec); sys.modules["fp"] = fp
spec.loader.exec_module(fp)
cap, freqs, cores = fp.parse_soc(fp.SOC_PROFILES["zumapro"]["soc"])
print("cap:", cap, "freqs:", {k: len(v) for k, v in freqs.items()}, "cores:", cores)
PY
```

Expected: `cap: {}`（zumapro 的 `[capacity]` 还是空 TODO，段内无数值键）、`freqs: {}`（`[freq_khz]` 段尚未创建）、`cores: {'little': 4, 'big': 3, 'prime': 1}`。**关键点：cap 必须是空字典，不是 `{'little': 160, 'big': 190, 'prime': 220}`。**

- [ ] **Step 13: 提交（需用户授权）**

```bash
cd /home/aibeto/code/ChiRi && git add scripts/frontier_policy.py && git -c user.name=13812773784 -c user.email=tc.aibeto@outlook.com commit -m "refactor(frontier): 泛化前沿生成器为 --soc 驱动

- 修 b136f30 漏改的 DOC 路径常量（脚本此前已无法运行）
- parse_soc 改为按段取：zumapro 的 [idle] 段含 little/big/prime 整数键，
  全文件首个匹配会把 idle 的值当 capacity
- 核数由 [topology] 区间长度算，锚点/顶段起点/算力单位按 SoC 分组
- 8550 回归：指纹 92bcedf210e6aed1，受控块逐字节不变"
```

---

## Task 2: 写 zumapro 能效模型文档

**Files:**
- Create: `mdocs/zumapro/zumapro-freq-power.md`

文档语言要求：参考 `humanizer-zh`，不用破折号、不写夸大结论、不写「挑战与展望」式套段、不用加粗小标题列表。数字全部来自 `freqbench/results/zumapro/main/results.json`，四个小数位的功耗与一位小数的算力。**表头与标题形态必须让生成器可解析**：标题是 `#### little 簇（…）`，表三列，第一列是纯数字 kHz，表格后必须留空行。

- [ ] **Step 1: 建目录并写文档**

```bash
cd /home/aibeto/code/ChiRi && mkdir -p mdocs/zumapro
```

创建 `mdocs/zumapro/zumapro-freq-power.md`，内容为：

````markdown
# zumapro 频率与功耗对照（freqbench 实测）

> 索引：`[source]` `[leakage]` `[efi]` `[em]` `[frontier]` `[limits]`

- 功耗与算力来源：`freqbench/results/zumapro/main/results.json`（Pixel 9 Pro，board=zumapro，kernel 6.1.75-g7910ee17f109，`sultan@sultan-box`，2024-09-13 构建）。
- 该份数据没有 `housekeeping` 键，而 kdrag0n 的 `bench.py:471` 无条件写这个键，所以它不是整机差分口径。prime 的 `idle.power_mean` 恒为 0.0 mW、little 在 19.8 到 38.7 mW 之间，整机差分不可能出现这两个形态。判读为 tensor 分支的 PMIC ODPM 逐轨净能量方法。
- 口径收益：ODPM 逐轨净能量与 ChiRi 的 `Cx·f·V²` 同属 CPU 动态功耗，不含屏幕、DDR、GPU。这也是 8550 那张表想对齐的口径，区别只在 8550 是算出来的、这里是量出来的。
- 定位：校准参照与前沿表的唯一数据源，不是运行期输入。生成器 `scripts/frontier_policy.py` 读本文件 §3 的三张表，因此**三张表的表头与列序不可改**。
- 生成 2026-10-19。

## 1. [source] 频率档位与来源核对

三簇档位取自 freqbench 的逐档测量，且与内核 DT 的 `dsu-bci-constraint-table` 不矛盾：

| 簇 | policy | 核型 | 档位数 | 频率范围 kHz |
| --- | --- | --- | ---: | --- |
| little | policy0（CPU0-3） | A520 | 10 | 820000 – 1950000 |
| big | policy4（CPU4-6） | A720 | 17 | 357000 – 2600000 |
| prime | policy7（CPU7） | X4 | 18 | 700000 – 3105000 |

little 的十档与 DT `zumapro-cpu.dtsi` 的 `dsu-bci-constraint-table`（260 到 269 行）逐字节一致：`1950000 1849000 1696000 1548000 1425000 1328000 1197000 1098000 955000 820000`。

prime 有一处需要说明：`zumapro-cpu.dtsi:292` 的 `cpufreq_domain2` 声明 `min-freq = <500000>`，但 freqbench 与 Google 的 `pixel_em` 表都从 700000 起。固件没有暴露 500 MHz 这一档，本文件按 700000 起算。

## 2. [leakage] 漏电校正

`results.json` 的 `active.power_mean` 是「最忙单核 + 其余核空闲」时的整轨读数，`idle.power_mean` 是该档整轨的静态读数。直接把 `active` 乘以核数会把同一份漏电重复计 N 次。簇功耗按

```
cluster_W = n × active − (n − 1) × idle
```

还原：整个簇从空闲切到满载，只多出一份动态功耗，静态部分在两种状态下都存在。

校正幅度与核数、漏电占比有关：little 四核在 820 MHz 从 0.2255 W 降到 0.1661 W，占 26.3%，因为它那档 idle 有 19.79 mW；big 三核在 357 MHz 从 0.1051 W 降到 0.1020 W，占 2.9%，因为它的 idle 只有 1.53 mW；prime 单核两个公式等价，无需校正。

## 3. [efi] 簇级功耗与算力

算力走实测，不做 `capacity × f / f_max` 的线性假设：`CMdmips = coremarks_per_mhz × f[MHz] × 核数 ÷ 1000`。这与 8550 文档的 `Mdmips` 不是同一个单位，两机不可横向比较。

下面三张表是生成器的直接输入。功耗取四位小数，算力取一位小数。

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

big 的 1795000 档是一个实测坏点：它比更低频的 1945000 档更贵（1.3543 W 对 1.1286 W，算力也更低）。原值保留并在此标注，不修正。枚举下凸包时它一次都没有入选。

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

## 4. [em] Google EM 旁证

`pixel_em` 的自报功耗表（`zumapro-a1-foplp.dts` 的 profiles `default`）可作旁证。三列经 `pixel_em.c:656` 的 `sscanf` 顺序确认为 `freq | capacity | power`，单位由 `include/linux/energy_model.h:88` 的 `EM_MAX_POWER = 64000000`（微瓦）钉死。

比值（EM 除以本文件实测）：

| 簇 | 中位 | 区间 |
| --- | --- | --- |
| little | 0.93 | 0.76 到 1.08 |
| big | 1.86 | 1.63 到 2.00 |
| prime | 1.85 | 1.72 到 1.91 |

big 与 prime 稳定在 1.85 附近，形状基本一致，只差一个常数倍率。little 有正负 15% 的漂移，低频偏低、高频偏高，正好落在漏电最重的那一簇。

EM 的 capacity 列在最高频档是 188 / 731 / 1024，与本文件折算的容量 283 / 714 / 1024 相比，little 差 33%，big 与 prime 一致。所以 EM 的容量列不能直接当 `cpu_capacity` 用。

DT 路径可作第二重交叉验证：`capacity-dmips-mhz` 为 little 449 / big 850 / prime 1024，按 `dmips × f_max` 归一到 1024 得 282 / 712 / 1024，与本文件折算的 283 / 714 / 1024 差 0.4%。

## 5. [frontier] 前沿复算

按 §3 三张表枚举 10 × 17 × 18 = 3060 个组合，每个总算力档位取最小总功耗，再做单调下凸包，得 38 个前沿点。

- 算力 32308.6 到 138924.2 CMdmips，功耗 0.4269 到 6.3108 W。
- 起手段边际能效 91863.1 CMdmips/W，末段（3015000 → 3105000）2521.4 CMdmips/W。
- 起点三簇全在最低档（820 / 357 / 700 MHz），终点三簇全满频（1950 / 2600 / 3105 MHz）。
- prime 在需求到达 82666.5 CMdmips 之前一直压在 700 MHz，之后才逐级抬升。这与 8550 的结论一致：单核算力需求越过阈值才启用 prime。

各簇效率峰值（ULPMark-CM，I/mJ）：

| 簇 | 峰值 | 最高频处 | 倍率 |
| --- | --- | --- | --- |
| little | 820 MHz @ 68.0 | 1950 MHz @ 44.7 | 1.52 倍 |
| big | 578 MHz @ 90.9 | 2600 MHz @ 26.8 | 3.39 倍 |
| prime | 700 MHz @ 47.1 | 3105 MHz @ 10.9 | 4.31 倍 |

## 6. [limits] 边界与注意

- 低频档的读数本来就比高频档噪。§3 的功耗列取四位小数是为了让下凸包的比较稳定，不代表测量精度有四位。
- `results.json` 里的 `active.power_mean` 与 `idle.power_mean` 是原始读数，只有 §2 校正后的 `cluster_W` 才是净动态功耗。
- 本文件的功耗是整簇口径，与该档 `active` 的单核读数不可混用。
- `pixel_em` 的 `cam1` 到 `cam3`、`game` 几个 profile 的功耗比 `default` 高一到三成，例如 little 820 MHz 从 43000 µW 到 56000 µW。EM 表随场景切换，而 freqbench 只在 `default` 下测。前沿表按 `default` 处理，理由见设计文档 §7：绝对功率不参与决策，参与的是相对能效，`default` 是同一条曲线上的保守取样点。
````

- [ ] **Step 2: 验证文档可被生成器解析**

```bash
cd /home/aibeto/code/ChiRi && python3 - <<'PY'
import importlib.util, sys
spec = importlib.util.spec_from_file_location("fp", "scripts/frontier_policy.py")
fp = importlib.util.module_from_spec(spec); sys.modules["fp"] = fp
spec.loader.exec_module(fp)
tables = fp.parse_cluster_tables(fp.SOC_PROFILES["zumapro"]["doc"])
print("档位:", {c: len(tables[c]) for c in fp.CLUSTERS})
print("little 首档:", tables["little"][0], "末档:", tables["little"][-1])
print("big 1795 档:", [r for r in tables["big"] if r[0] == 1795000])
print("prime 首档:", tables["prime"][0], "末档:", tables["prime"][-1])
PY
```

Expected: `档位: {'little': 10, 'big': 17, 'prime': 18}`；`little 首档: (820000, 0.1661, 15344.3)`；`big 1795 档: [(1795000, 1.3543, 47806.2)]`；`prime 首档: (700000, 0.1588, 7476.7)`。

- [ ] **Step 3: 把设计规格的 §4/§5 数字对齐到精确复算值**

`编辑 .codebuddy/docs/2026-10-19-zumapro-frontier-design.md`，把 §4 一节的三个数字与 §5 的锚点清单换成精确值。

§4 当前正文第一段：

```
3060 个组合（10 × 17 × 18）枚举出 38 点下凸包。算力 32338 到 138883 CMdmips，功耗 0.427 到 6.311 W。首段边际能效 91505 CMdmips/W，顶段 2579 CMdmips/W。
```

替换为：

```
3060 个组合（10 × 17 × 18）枚举出 38 点下凸包。算力 32308.6 到 138924.2 CMdmips，功耗 0.4269 到 6.3108 W。首段边际能效 91863.1 CMdmips/W，顶段 2521.4 CMdmips/W。
```

§5 结尾当前正文：

```
锚点对账的容差 `ANCHOR_TOL = 0.03` 保留。zumapro 的锚点值取第 4 节实测结果：`compute_min` 32338.3、`compute_max` 138882.9、`power_min` 0.4270、`power_max` 6.3108、`hull_points` 38、`first_seg` 91505.2、`last_seg` 2578.7。
```

替换为：

```
锚点对账的容差 `ANCHOR_TOL = 0.03` 保留。zumapro 的锚点值取第 4 节实测结果：`compute_min` 32308.6、`compute_max` 138924.2、`power_min` 0.4269、`power_max` 6.3108、`hull_points` 38、`first_seg` 91863.1、`last_seg` 2521.4。
```

同节里「zumapro 是 3015000」一句保留不变。

§4 的表格（三张簇级表）也要换成 Task 2 Step 1 的精确值，逐表替换 little 首行 `| 820000 | 0.166 | 15370 |` 这类行。改动清单：little 算力列由 15370/17901/20581/22437/24892/26710/29016/31790/34658/36551 换成 15344.3/17900.8/20540.4/22433.3/24919.0/26727.2/29022.0/31770.0/34761.4/36550.3，功耗由 0.166/0.202/0.241/0.267/0.315/0.354/0.408/0.478/0.603/0.702 换成 0.1661/0.2019/0.2415/0.2674/0.3152/0.3543/0.4077/0.4784/0.6031/0.7015；big 与 prime 用 Task 2 Step 1 同表替换。

- [ ] **Step 4: 确认文档不再有硬编码断言冲突**

在 Task 3 与 Task 4 完成后回看本步。此时只核对：文档 §5 公布的 7 个数与 Task 1 Step 3 写入 `SOC_PROFILES["zumapro"]["anchors"]` 的 7 个数逐位一致。

```bash
cd /home/aibeto/code/ChiRi && grep -n "32308.6\|138924.2\|0.4269\|6.3108\|91863.1\|2521.4\|38 点\|38" mdocs/zumapro/zumapro-freq-power.md | grep -v "^.*|" | head
```

Expected: 能看到 §5 里的算力区间、功耗区间、两个边际能效值，数字与 `SOC_PROFILES` 一致。

---

## Task 3: 填 zumapro 的 `soc.yaml` 基线

**Files:**
- Modify: `module/config/zumapro/soc.yaml:14-22`（`[capacity]` 与 `[freq]`）、`module/config/zumapro/soc.yaml:36-40`（`[dpc]`）

- [ ] **Step 1: 替换 `[capacity]` 段与其上方注释**

`module/config/zumapro/soc.yaml` 第 14 到 18 行当前内容：

```
# [capacity] TODO: 真机 cpu_capacity 未采样，暂不填。
# 该机 DT 里只有 capacity-dmips-mhz（little 193 / big 927 / prime 1024），它与 sysfs
# cpu_capacity **不是同一量纲**——后者是「dmips × policy 最高频」折算后再归一化到 1024 的值，
# 本机 arch_topology 的折算口径未确认，勿把 DT 值直接填进来。
# 影响面：仅帕累托前沿的需求折算与 FDP，本机无 frontier_policy，当前不受影响。
```

替换为：

```
# [capacity] EAS capacity 兜底（真机 cpu_capacity 读不到时用；真机 sysfs 永远优先）。
# 取值 283 / 714 / 1024，来自 freqbench 实测折算（coremarks_per_mhz × f_max 归一到 prime=1024；
# 实测 282.75 / 714.60 / 1024.00）。
# 原记录 little 193 / big 927 / prime 1024 已作废：那三个数是 Tensor G3（zuma-cpu.dtsi:55,91,127），
# 不是 zumapro。zumapro 本机 DT（zumapro-cpu.dtsi:52,88,115）是 449 / 850 / 1024，按
# 「dmips × f_max」折算得 282 / 712 / 1024，与实测路径差 0.4%，两条独立路径闭合。
# 注意 DT 的 capacity-dmips-mhz 与 sysfs cpu_capacity 仍不是同一量纲，勿直接把 DT 值填进来。
# 本值只作真机读不到时的兜底：需求折算 Σ(核 util × capacity) 用它，影响面是 CLG-PF 与 FDP。
capacity:
  little: 283
  big: 714
  prime: 1024
```

- [ ] **Step 2: 替换 `[freq]` 段**

第 20 到 22 行当前内容：

```
# [freq] TODO: 本机 DT 没有 CPU OPP（opp-hz 缺失，freq 由 cpufreq_domain 驱动/固件提供），
# 无静态档表可抄；以真机 scaling_available_frequencies 为准，取到后补三簇档位
# （写 scaling_max_freq 前须 floor 对齐到表内档位）。
```

替换为（`freq_khz` 用与 8550 同构的块状写法，档位来自 freqbench，已与 DT `dsu-bci-constraint-table` 核对）：

```
# [freq_khz] 频率档位兜底（kHz 升序；scaling_available_frequencies 读不到时用）。
# 来源：freqbench 逐档实测（mdocs/zumapro/zumapro-freq-power.md §1 表），little 十档已与
# DT zumapro-cpu.dtsi:260-269 的 dsu-bci-constraint-table 逐字节核对一致。DT 无 CPU OPP
# （opp-hz 缺失，频率由 cpufreq_domain 驱动/固件提供），故本表以实测为准。
# 写 scaling_max_freq 前必须 floor 对齐到表内档位（内核 clamp 向下取最近档）。
# prime 起始档是 700000 而非 DT cpufreq_domain2 声明的 min-freq 500000：500 MHz 固件未暴露。
# 本表必须与 mdocs/zumapro/zumapro-freq-power.md §3 逐簇档位数一致，生成器会断言。
freq_khz:
  # little / policy0：10 档 820000-1950000
  little:
    [
      820000,
      955000,
      1098000,
      1197000,
      1328000,
      1425000,
      1548000,
      1696000,
      1849000,
      1950000,
    ]
  # big / policy4：17 档 357000-2600000
  big:
    [
      357000,
      578000,
      648000,
      787000,
      910000,
      1065000,
      1221000,
      1328000,
      1418000,
      1549000,
      1795000,
      1945000,
      2130000,
      2245000,
      2367000,
      2450000,
      2600000,
    ]
  # prime / policy7：18 档 700000-3105000
  prime:
    [
      700000,
      1164000,
      1396000,
      1557000,
      1745000,
      1885000,
      1999000,
      2147000,
      2294000,
      2363000,
      2499000,
      2687000,
      2802000,
      2914000,
      2943000,
      2970000,
      3015000,
      3105000,
    ]
```

- [ ] **Step 3: 修 `[dpc]` 段**

第 36 到 40 行当前内容：

```
# [dpc] dynamic-power-coefficient（来自同一份 dtb 的逐核属性）。校准基准，本期无运行时消费方。
dpc:
  little: 70
  big: 513
  prime: 757
```

替换为：

```
# [dpc] dynamic-power-coefficient（来自 zumapro-cpu.dtsi 的逐核属性）。校准基准，本期无运行时消费方。
# 原记录 70 / 513 / 757 已作废：那是 Tensor G3 的数。本机 DT 实际是 176 / 325 / 748。
dpc:
  little: 176
  big: 325
  prime: 748
```

- [ ] **Step 4: 更新文件头区块索引**

第 1 行当前内容：

```
# zumapro/soc.yaml: [topology] [capacity] [freq] [idle] [dpc] [cpu_governors]
```

替换为：

```
# zumapro/soc.yaml: [topology] [capacity] [freq] [idle] [dpc] [cpu_governors] [frontier_policy]
```

并在第 4 行 `soc: "zumapro"` 之前追加两行说明（与 8550 同构）：

```
# [frontier_policy] 是例外：它是**策略表**（帕累托前沿结果向量），由
# scripts/frontier_policy.py 离线生成、受控块替换（块外内容勿手改本块）。
```

- [ ] **Step 5: 验证解析结果**

```bash
cd /home/aibeto/code/ChiRi && python3 - <<'PY'
import importlib.util, sys
spec = importlib.util.spec_from_file_location("fp", "scripts/frontier_policy.py")
fp = importlib.util.module_from_spec(spec); sys.modules["fp"] = fp
spec.loader.exec_module(fp)
cap, freqs, cores = fp.parse_soc(fp.SOC_PROFILES["zumapro"]["soc"])
print("cap:", cap)
print("freqs:", {k: (len(v), v[0], v[-1]) for k, v in freqs.items()})
print("cores:", cores)
PY
```

Expected: `cap: {'little': 283, 'big': 714, 'prime': 1024}`、`freqs: {'little': (10, 820000, 1950000), 'big': (17, 357000, 2600000), 'prime': (18, 700000, 3105000)}`、`cores: {'little': 4, 'big': 3, 'prime': 1}`。

- [ ] **Step 6: 验证 8550 未被波及**

```bash
cd /home/aibeto/code/ChiRi && python3 scripts/frontier_policy.py --soc 8550 --check; echo "EXIT=$?"
```

Expected: `--check: 一致（逐字节相同）`，`EXIT=0`。

---

## Task 4: 生成并落 zumapro 的 `frontier_policy` 受控块

**Files:**
- Modify: `module/config/zumapro/soc.yaml`（追加受控块）

- [ ] **Step 1: dry-run 复算并核对锚点**

```bash
cd /home/aibeto/code/ChiRi && python3 scripts/frontier_policy.py --soc zumapro 2>&1 | head -20
```

Expected:

```
SoC: zumapro
输入: mdocs/zumapro/zumapro-freq-power.md / module/config/zumapro/soc.yaml
档位: {'little': 10, 'big': 17, 'prime': 18}  核数: {'little': 4, 'big': 3, 'prime': 1}  组合: 3051  前沿点: 38  桶: 38
指纹: <16 位十六进制>
复算对账（容差 3%）:
  [OK ] 总算力下限: got=32308.6 want=32308.6
  [OK ] 总算力上限: got=138924.2 want=138924.2
  [OK ] 总功耗下限: got=0.427 want=0.4269
  [OK ] 总功耗上限: got=6.311 want=6.3108
  [OK ] 前沿点数: got=38 want=38
  [OK ] 起手段 边际能效: got=91863.141 want=91863.1
  [OK ] prime 冲顶段 边际能效: got=2521.365 want=2521.4
```

7 项必须全 `[OK ]`。组合数 3051 与原始 3060 的差是总算力重复档位合并，与 8550 的 6358 对 6720 同因。**任一项 FAIL 就停手**：先查 Task 3 的表格与 §5 锚点是否自洽，不要用 `--force` 绕过。

- [ ] **Step 2: 落表**

```bash
cd /home/aibeto/code/ChiRi && python3 scripts/frontier_policy.py --soc zumapro --write
```

Expected: `已写入: module/config/zumapro/soc.yaml（受控块替换，块外未动）`。

- [ ] **Step 3: 校验块已落且确定性**

```bash
cd /home/aibeto/code/ChiRi && python3 scripts/frontier_policy.py --soc zumapro --check; echo "EXIT=$?"
```

Expected: `--check: 一致（逐字节相同）`，`EXIT=0`。

- [ ] **Step 4: 校验块边界与关键字段**

```bash
cd /home/aibeto/code/ChiRi && grep -n "frontier_policy (generated)\|<<< frontier_policy <<<" module/config/zumapro/soc.yaml && grep -c "point:" module/config/zumapro/soc.yaml && grep -n "enabled:\|model_fingerprint:\|little: { capacity" module/config/zumapro/soc.yaml && grep -n "prime_top\|min-freq" module/config/zumapro/soc.yaml | head
```

Expected: 块起止标记各 1 行；`point:` 计数为 **152**（38 个 `demand_buckets` 行 × 1 + 38 个 `modes` 行 × 3 模式；本会话已用 fixture 实测确认此值）；`enabled: true`；`model_fingerprint` 已填；`little: { capacity: 283, freq_khz: [820000, ...`；`soc: "zumapro"` 与 `[topology]` 段仍在块外。

- [ ] **Step 5: 校验 38 个桶边界严格升序**

```bash
cd /home/aibeto/code/ChiRi && python3 - <<'PY'
import re
from pathlib import Path
t = Path("module/config/zumapro/soc.yaml").read_text(encoding="utf-8")
b = t.find("# >>> frontier_policy (generated) >>>"); e = t.find("# <<< frontier_policy <<<")
blk = t[b:e]
lo = [float(m) for m in re.findall(r"lo_units: ([\d.]+)", blk)]
print("桶数:", len(lo), "首:", lo[0], "末:", lo[-1])
print("升序:", all(lo[i] < lo[i+1] for i in range(len(lo)-1)))
PY
```

Expected: `桶数: 38`、`首: 1001.0`、`末: 4283.2`、`升序: True`。桶数 38 是「`combo_units` 零逆序对」的落点，若桶数不是 38 或升序为 False，说明 Task 3 的 `[capacity]` 与 `[freq_khz]` 有误。注意 `末` 是末桶的 `lo_units`（末两点算力中点），**不是** `combo_units` 的上限 4298.0（后者是全核满载值）；本会话已用 fixture 实测确认 4283.2。

- [ ] **Step 6: 运行期消费方编译检查**

```bash
cd /home/aibeto/code/ChiRi && cargo check -p chiri --target aarch64-linux-android
```

Expected: 编译通过，无 error。此步确认新块能被 `SocConfig` 的 `[frontier_lenient]` 宽松反序列化接受（字段名与 `Frontier*` 结构逐字对齐）。

- [ ] **Step 7: 再确认 8550 块未动**

```bash
cd /home/aibeto/code/ChiRi && git diff --stat module/config/8550/ && python3 scripts/frontier_policy.py --soc 8550 --check; echo "EXIT=$?"
```

Expected: `git diff --stat` 对 `module/config/8550/` 无输出（未改）；`--check: 一致（逐字节相同）`，`EXIT=0`。

---

## Task 5: 打开 zumapro 的 FDP

**Files:**
- Modify: `module/config/zumapro/feature.yaml:44`（`Affinity` 段内）

- [ ] **Step 1: 替换 FDP 注释并插入三参数**

`module/config/zumapro/feature.yaml` 第 44 行当前内容：

```
  # FDP（fdp_enabled）依赖 soc.yaml 的 frontier_policy 逐档功耗表，本机无该表 → 保持关闭。
```

替换为：

```
  # ---- FDP：帕累托前沿主导的放置内核（2026-10-20 起启用）----
  # 用 mdocs/zumapro/zumapro-freq-power.md §3 的**逐档簇功耗**做双边净收益比较：把线程从源簇搬到
  # 目标簇，只有 [源簇让出后−现状] + [目标簇承接后−现状] < 0（且超阈值）才迁。数据来自
  # soc.yaml 的 frontier_policy 受控块 `per_cluster.<簇>.power_w`，与 8550 同一条数据链。
  # 本机功耗是 ODPM 实测净能量并做过漏电校正（设计文档 §7），与 8550 的推算表不同源，
  # 但两者都只进成本模型的**相对**比较，阈值量纲相同。
  # 只作用于**后台/批处理线程 promote**（成本模型不含延迟项，前台路径一律不动）。
  fdp_enabled: true
  # 只在该次搬迁能省 ≥ 该值(W) 时才动：防在噪声上反复搬（0.02 = 20 mW）
  fdp_cost_hysteresis_w: 0.02
  # 每轮每个目的簇的迁入条数硬上限：成本项只在跨轮生效，同轮几十条线程会一起判「big 最便宜」，
  # 必须靠配额兜住同向涌入（0 视为 1，不允许「无上限」）
  fdp_max_inserts_per_round: 2
  # 迁入后目标簇的模型工作频率不得高于「该值 × 该簇最高频」——
  # P-a「任何簇贴顶都不行」在代码里的落点
  fdp_dst_freq_cap_ratio: 0.85
```

三参数取 8550 同值，理由：`fdp_cost_hysteresis_w` 是绝对量纲的工程阈值（不是物理常数）；`fdp_max_inserts_per_round` 与簇规模无关；`fdp_dst_freq_cap_ratio` 是「不许贴顶」的通用护栏。逐档功耗表是含漏电校正的实测值，不改变这三个阈值的量级。

- [ ] **Step 2: 校验 YAML 层级没写错**

```bash
cd /home/aibeto/code/ChiRi && python3 -c "
import re
t = open('module/config/zumapro/feature.yaml').read()
for line in t.splitlines():
    if re.match(r'^[A-Za-z\[]', line) or 'fdp_' in line: print(repr(line[:70]))
" | head -20
```

Expected: `fdp_*` 四行出现在 `Affinity:` 与 `CoreCtl:` 之间，缩进为两格；`Affinity:` 段仍是一个顶层键。

- [ ] **Step 3: 编译检查**

```bash
cd /home/aibeto/code/ChiRi && cargo check -p chiri --target aarch64-linux-android
```

Expected: 通过。FDP 的配置字段（`fdp_enabled` / `fdp_cost_hysteresis_w` / `fdp_max_inserts_per_round` / `fdp_dst_freq_cap_ratio`）已在 `src/chiri/config.rs:1316-1325` 定义，无需改 Rust。

- [x] **Step 4: 改写 feature.yaml 文件头的过期注释（已作废，2026-10-20）**

2026-10-20：Stardust 家族功耗调频器已从代码库整体移除，`module/config/*/feature.yaml` 里已无对应配置段，本步无对象。原计划在此把该段注释从「本机无 `frontier_policy` → 恒回退 hw_max」改写为「已有该段但桶号未拍板」，该改写不再执行。`frontier_policy` 与 FDP 的生效不受影响，本步的 grep 校验也无需再跑。

- [ ] **Step 5: 提交（需用户授权）**

```bash
cd /home/aibeto/code/ChiRi && git add mdocs/zumapro module/config/zumapro/soc.yaml module/config/zumapro/feature.yaml .codebuddy/docs/2026-10-19-zumapro-frontier-design.md && git -c user.name=13812773784 -c user.email=tc.aibeto@outlook.com commit -m "feat(zumapro): 落地帕累托前沿表并启用 FDP

- 新增 mdocs/zumapro/zumapro-freq-power.md（freqbench 实测 + 漏电校正）
- soc.yaml 填 capacity 283/714/1024 与三簇频表，修 dpc 176/325/748
- 生成器落受控块：38 前沿点，指纹与锚点全部对账通过
- feature.yaml 打开 FDP 三参数（与 8550 同值）"
```

---

## Task 6: 回填权威文档并清掉失效断言

**Files:**
- Modify: `agentsdocs/06-kernel.md:35`、`agentsdocs/06-kernel.md:37`
- Modify: `agentsdocs/07-todos.md:132`
- Modify: `mdocs/socList.md`
- Modify: `src/chiri/energy_cost.rs:55`、`src/common.rs:263,326`、`src/main.rs:122`

- [ ] **Step 1: 修 `agentsdocs/06-kernel.md:35` 的容量记录**

该行当前内容：

```
- capacity：DT 只有 `capacity-dmips-mhz`（cpu0-3 = 193、cpu4-6 = 927、cpu7 = 1024），它与 sysfs `cpu_capacity` **不同量纲**——后者是「dmips × policy 最高频」折算后归一化到 1024。本机 arch_topology 的折算口径未确认，**勿把 DT 值直接当 sysfs 值填进 soc.yaml**；真机 `cpu_capacity` 待采样（FAS 算力权重日志或 sysfs）。
```

替换为：

```
- capacity：**原记录 193 / 927 / 1024 是 Tensor G3 的数，已作废**（那三个值一字不差地出现在 `zuma-cpu.dtsi`，不是本机）。zumapro 本机 DT（`zumapro-cpu.dtsi:52,88,115`）是 `capacity-dmips-mhz` little 449 / big 850 / prime 1024，按「dmips × f_max」归一到 1024 得 **282 / 712 / 1024**；freqbench 实测路径得 **283 / 714 / 1024**，两条独立路径差 0.4%。已写进 `module/config/zumapro/soc.yaml [capacity]` 作 sysfs 兜底。DT 的 `capacity-dmips-mhz` 与 sysfs `cpu_capacity` 仍不是同一量纲，**勿直接把 DT 值填进 soc.yaml**；真机 `cpu_capacity` 待采样（FAS 算力权重日志或 sysfs）后与本兜底值对账。
```

- [ ] **Step 2: 修 `agentsdocs/06-kernel.md:37` 的 dpc 记录**

该行当前内容：

```
- dpc：little 70 / big 513 / prime 757。
```

替换为：

```
- dpc：little 176 / big 325 / prime 748（`zumapro-cpu.dtsi` 逐核属性）。**原记录 70 / 513 / 757 是 Tensor G3 的数，已作废。** 校准基准，本期无运行时消费方。
```

- [ ] **Step 3: 修 `agentsdocs/06-kernel.md` 里「DT 没有 CPU OPP」那行**

该行当前内容：

```
- **DT 没有 CPU OPP**（无 `opp-hz`，只有 `cpufreq_domain0-2` 与外围设备的 `freq-table-hz`）→ 频表来自驱动/固件，只能取真机 `scaling_available_frequencies`。
```

替换为：

```
- **DT 没有 CPU OPP**（无 `opp-hz`，只有 `cpufreq_domain0-2` 与外围设备的 `freq-table-hz`）→ 频表来自驱动/固件，只能取真机 `scaling_available_frequencies`。已取到：freqbench 逐档实测 little 10 档 820000-1950000 / big 17 档 357000-2600000 / prime 18 档 700000-3105000，与 DT `dsu-bci-constraint-table`（`zumapro-cpu.dtsi:260-269`）的 little 十档逐字节一致。**prime 起始档是 700000，不是 `cpufreq_domain2` 声明的 `min-freq 500000`**（500 MHz 固件未暴露）。
```

- [ ] **Step 4: 更新 `agentsdocs/07-todos.md:132` 的 zumapro 条目**

该条目④当前说的是「capacity 与完整频表缺口」。改为记录已完成与新缺口：

在条目④末尾追加：

```
  **④ 已结（2026-10-20）**：`capacity` 已定 283 / 714 / 1024（freqbench 实测折算，DT 路径 282 / 712 / 1024 交叉验证，原 193/927/1024 是 Tensor G3 之误）；三簇频表已落 `soc.yaml [freq_khz]`（10/17/18 档）；`[dpc]` 修为 176/325/748。**保留待办**：真机 `cpu_capacity` 采样后与兜底值对账；DT `cpufreq_domain2` 的 min-freq 500000 与固件实际首档 700000 的差异需在真机确认一次。
```

并在条目⑤之后新增第六点：

```
  ⑥ 帕累托前沿与 FDP 已于 2026-10-20 落地（`soc.yaml` 受控块 38 个前沿点 + `feature.yaml` `fdp_enabled: true`）。**真机验证项与 8550 同口径**：CLG-PF 落点只下调，看同场景稳态频率是否下移而掉帧不增；触摸窗口内 PF 与 decay 同口径关闭。**另一个独立缺口**：本机调速器是 `sched_pixel`，它是否改写 CLG 的 `scaling_max_freq` 写入会使 PF 落点失效，需单独确认（若是，按 `[cpu_governors]` 注释把 `sched_pixel` 删掉即回退 `schedutil`）。
```

- [ ] **Step 5: 更新 `mdocs/socList.md`**

Zumapro 行的「帕累托前沿」列当前是 `- [ ]`。改为 `- [x]`：

```
| Zumapro | 测试中   | 20,950,120  | 6        | 6        | - [x]      | - [x]    | Pixel 9 Pro                |
```

已适配列的 `SM8550` 行不动。Zumapro 的适配情况仍写「测试中」：表已落地，但真机验证项（Task 6 Step 4 的 ⑥）尚未执行。

- [ ] **Step 6: 登记一个副作用：落块会打开 zumapro 的两列遥测**

落 `frontier_policy` 块会连带启用 `status.csv` 的 `cpu_dyn_w` / `resid_w` 两列。原因：`cpu_dynamic_power_w()`（`src/chiri/energy_cost.rs:64`）进门先查 `has_cpu_energy_table()`，无表的 SoC 直接返回 `None` 让该列写 `-`；而 `cluster_power_w()`（`src/chiri/energy_cost.rs:56`）读的正是 `soc_frontier_policy().per_cluster.<簇>.power_w`。zumapro 有了块，这两列从此有值，不再是 `-`。

把这一条写进 `agentsdocs/07-todos.md` 的 zumapro 段落（与 Task 7 Step 4 的遗留项并列），要点两条：

1. 这是**预期行为**，不是回归。zumapro 的逐档功耗是 ODPM 实测净能量并做过漏电校正，比 8550 的推算表更贴 `cpu_dyn_w` 的定义（该列是 CPU 动态功耗下界）。
2. 回来后要复核一次真机 `status.csv`：`resid_w = batt_power_w − cpu_dyn_w` 在充电行仍无意义；低负载分位的 `cpu_dyn_w` 量级是否合理（zumapro 的 little 820 MHz 簇功耗 0.1661 W，比 8550 同档的 0.0690 W 高，属两机不同微架构，不作跨机对照）。

- [ ] **Step 7: 清掉 `src/` 里失效的「仅 8550」断言（只改注释）**

逐个改，**只改注释文字、不动任何代码行**：

`src/chiri/energy_cost.rs:55`：
```
/// 非 8550 / 前沿段缺失 / 表不齐 → `None`
```
改为
```
/// 无 frontier_policy 段 / 表不齐 → `None`（8550 与 zumapro 都有该段）
```

`src/common.rs:263`：
```
    /// 帕累托前沿查表段（仅 8550 具备采用条件；其它 SoC 的 soc.yaml 无本段 → 天然 None → 走原比例路径）
```
改为
```
    /// 帕累托前沿查表段（8550 / zumapro 已具备；无本段的 SoC → 天然 None → 走原比例路径）
```

`src/common.rs:326`：
```
// [frontier] 帕累托前沿策略段，8550 专用，由 scripts/frontier_policy.py 生成到 soc.yaml
```
改为
```
// [frontier] 帕累托前沿策略段，由 scripts/frontier_policy.py --soc <机型> 生成到对应 soc.yaml
```

`src/main.rs:122`：
```
    // [pf_chr] 写本机 CLG 是否启用帕累托前沿（`1` 启用，仅 8550；`0` 未启用）：静态资产，启动写一次，只作 WebUI 只读接触点
```
改为
```
    // [pf_chr] 写本机 CLG 是否启用帕累托前沿（`1` 启用，当前 8550 与 zumapro；`0` 未启用）：静态资产，启动写一次，只作 WebUI 只读接触点
```

原 Stardust 功耗调频器模块（`src/chiri/` 下）已于 2026-10-20 随功能整体删除，本处无对象：原计划在此改该文件内的一条 tier 档位 `TODO` 注释，注释已随文件移除，无需再改。

- [ ] **Step 8: 改权威文档里三处「只有 8550」的断言**

这三处是 CLAUDE 之外的事实源，必须逐字改。**只改「唯一/只有」这层限定，保留「其它 SoC 无本段」这句**（它仍然成立）。

`agentsdocs/02-convention.md:29`，把：

```
只有 8550 的 `soc.yaml` 有 `per_cluster.*.power_w`，其余 SoC 两列恒 `-`
```

改为：

```
8550 与 zumapro 的 `soc.yaml` 有 `per_cluster.*.power_w`（zumapro 自 2026-10-20 起），其余 SoC 两列恒 `-`
```

`agentsdocs/02-convention.md:87`，把：

```
只有 8550 有该段（唯一具备采用条件的机型），其它 SoC 无段 → `common::soc_frontier_policy()` 返回 None 走原逻辑。
```

改为：

```
8550 与 zumapro 有该段（具备逐档能效模型 + 实测 capacity 的机型），其它 SoC 无段 → `common::soc_frontier_policy()` 返回 None 走原逻辑。新增机型的生成器用法：`python3 scripts/frontier_policy.py --soc <键>`（键在脚本 [soc] 段的 `SOC_PROFILES`）。
```

`agentsdocs/03-chiri.md:116`，把标题与那句限定改掉。标题里的 `，仅 8550）` 改为 `，8550 / zumapro）`；正文里：

```
**其它 SoC 无该段、天然走原比例路径**（口径：只有 8550 具备采用条件——有逐档能效模型与真机 capacity）。
```

改为：

```
**其它 SoC 无该段、天然走原比例路径**（口径：具备逐档能效模型与实测 capacity 的机型才启用——目前是 8550 与 zumapro）。
```

同行的 `pf.chr`（`1` = 启用，仅 8550）改为 `（`1` = 启用，当前 8550 与 zumapro）`。

`agentsdocs/03-chiri.md:237`，把：

```
**非 8550 无该段**（无 capacity / 无 `per_cluster` 功耗表）
```

改为：

```
**无该段的 SoC**（无 capacity / 无 `per_cluster` 功耗表）
```

并把该行「表是纯动态项、不含漏电」后的口径补一句，因为两机的表来源不同：

```
表是纯动态项、不含漏电，只作相对排序。**两机表不同源**：8550 是按 `Cx·f·V²` 推算的，zumapro 是 PMIC ODPM 实测净能量并做过漏电校正（`cluster_W = n×active − (n−1)×idle`）；两者都只进相对比较，不可跨机比绝对值。
```

`module/config/8550/soc.yaml:7`，把：

```
# 8550 是唯一具备采用条件的机型（有逐档能效模型 + 真机 capacity）；其它 SoC 无本段 →
```

改为：

```
# 8550 与 zumapro 具备采用条件（有逐档能效模型 + 实测 capacity）；其它 SoC 无本段 →
```

**不要动**：`src/chiri/config.rs`、`module/config/8550/feature.yaml` 里指 8550 具体调参值的「仅 8550」（例如 uclamp 85、touch 档位、`normal_fg_exclude_little`），那些是机型专属取值，不是能力断言。

- [ ] **Step 9: 格式化与编译**

```bash
cd /home/aibeto/code/ChiRi && cargo fmt --all && cargo fmt --all --check && cargo check -p chiri --target aarch64-linux-android
```

Expected: `cargo fmt --all --check` 无输出（干净）；`cargo check` 通过。注释改动不引入逻辑变化，但仓库口径要求在改动后格式化。

- [ ] **Step 10: 检查没有引入只差空白的噪声 diff**

```bash
cd /home/aibeto/code/ChiRi && git diff --numstat src/ && echo "--- 忽略空白 ---" && git diff -w --numstat src/
```

Expected: 两列数字应完全一致（本次只改注释文字，不应产生纯空白差异）。若不一致，说明格式化翻动了无关行，按 `feedback_fix_scope.md` 逐行合并处理，**不要用 `git checkout` 批量回滚**。

---

## Task 7: 收尾验证与遗留项登记

- [ ] **Step 1: 双 SoC 并存的最终校验**

```bash
cd /home/aibeto/code/ChiRi && for s in 8550 zumapro; do echo "=== $s ==="; python3 scripts/frontier_policy.py --soc $s --check; echo "EXIT=$?"; done
```

Expected: 两个 SoC 都打印 `--check: 一致（逐字节相同）` 与 `EXIT=0`。

- [ ] **Step 2: 确认两个块互不串门**

```bash
cd /home/aibeto/code/ChiRi && grep -c "frontier_policy (generated)" module/config/8550/soc.yaml module/config/zumapro/soc.yaml && git diff --stat module/config/8550/
```

Expected: 两文件各 1 个起始标记；`module/config/8550/` 无 diff 输出。

- [ ] **Step 3: 运行期字段名与块内键逐字对齐的自检**

```bash
cd /home/aibeto/code/ChiRi && grep -o "enabled\|model_fingerprint\|anchors\|frontier\|per_cluster\|demand_buckets\|modes\|anticipate" module/config/zumapro/soc.yaml | sort -u
```

Expected: 输出 `anchors`、`anticipate`、`demand_buckets`、`enabled`、`frontier`、`model_fingerprint`、`modes`、`per_cluster`。这八个键与 `src/common.rs` 的 `FrontierPolicy` 结构逐字对应（`src/common.rs:333-360`）。

- [ ] **Step 4: 登记遗留项到台账**

把下面三条写进 `agentsdocs/07-todos.md` 的 zumapro 段落（与 Task 6 Step 4 的 ⑥ 合并成同一段）：

1. **CLG-PF 真机验证**（R3 口径，只做长时观察）：落点只下调、掉帧不增；触摸窗口内 PF 关闭。
2. **`sched_pixel` 与 PF 写入是否打架**：真机看 `clg-pf-enabled` 日志与 `scaling_max_freq` 是否被改写。
3. **原 Stardust 功耗调频器的 tier 桶号未拍板** → 该功能已于 2026-10-20 整体移除，此项作废；`frontier_policy` 与 FDP 的验证项不受影响。

- [ ] **Step 5: 报告本计划未覆盖的项**

向用户明确三件**没有**做、且需要单独决策的事：

1. `--lead-frac`（占位 0.30）与 `--reserve`（reduce 0 / default 0.10 / boost 0.25）仍是占位值，zumapro 沿用同值。这两个参数只在 `modes` 段里出现，运行期当前没有消费方，故不影响本次生效。
2. `mdocs/zumapro/` 的目录归属：本次按用户指定放 `mdocs/`（与 `mdocs/sm8550/` 同构），而 AGENTS.md 原文说 `mdocs/` 只放项目原有文档。两者不一致已按用户指令处理，但 AGENTS.md 未同步修改。
3. `capacity` 的 big 值取 714（截断）还是 715（四舍五入），见本计划开头的「容量取整说明」。

---

## 验收口径汇总

| 项 | 判据 |
| --- | --- |
| 8550 回归 | `python3 scripts/frontier_policy.py --soc 8550 --check` exit 0，指纹 `92bcedf210e6aed1`，`git diff module/config/8550/` 无输出 |
| zumapro 复算 | 7 项锚点全 `[OK ]`，前沿点 38，桶 38 且严格升序 |
| zumapro 确定性 | `--soc zumapro --check` exit 0 |
| 生成器段落取用 | zumapro `[capacity]` 解析为 283/714/1024，不得是 160/190/220 |
| 运行期兼容 | `cargo check -p chiri --target aarch64-linux-android` 通过 |
| 格式 | `cargo fmt --all --check` 干净 |
| 配置契约 | `build.rs` 的 `assert_required_configs()` 通过（`module/config/zumapro/` 三件套齐全，本次未动 meta.yaml） |
| 未越权 | 两处 commit 都要用户明确授权后才执行 |
