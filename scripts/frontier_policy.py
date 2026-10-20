#!/usr/bin/env python3
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

import argparse
import hashlib
import re
import sys
from pathlib import Path

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
    "sm8550": {
        "doc": ROOT / "mdocs" / "sm8550" / "sm8550-freq-power.md",
        "soc": ROOT / "module" / "config" / "sm8550" / "soc.yaml",
        "out_new": ROOT / "module" / "config" / "sm8550" / "frontier_policy.yaml",
        # 文档 §5 原话："prime 冲顶段（2092800→3187200 只剩 600 Mdmips/W）" —— 指的是**整段摊薄**的边际，
        # 不是顶部单档（2956800→3187200 单档只有约 470 Mdmips/W）。锚点按整段口径取。
        "prime_top_from_khz": 2092800,
        # 复算对账锚点（文档 §5/§7 公布值；容差给 3%，用于发现口径漂移）
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
        # 复算对账锚点（文档 §4/§5 公布值；容差 3%）
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
DEFAULT_SOC = "sm8550"

# [soc] 批量适配：手写档案只留给有人工核对过的机型（sm8550 / zumapro）。
# 其余 SoC 由 devimpbin/batch_ingest.py 从 freqbench_tensor 结果折算出 mdocs/<soc>/<soc>-freq-power.md，
# 档案按下方约定自动生成——只信实测、不编调优值：
#   * prime_top_from_khz 取 prime 表倒数第二档（末段边际能效的起点）
#   * anchors 置空 = 该机型没有独立公布值可对账，块内不写对账段（避免自产自对的假自检）
#   * compute_unit / power_comment 用实测口径的通用措辞
BATCH_COMPUTE_UNIT = "CMdmips(CoreMark 实测折算)"
BATCH_POWER_COMMENT = [
    "  # 各簇频表 + 逐档**簇功耗**(W)：功耗列由 devimpbin/batch_ingest.py 从 freqbench_tensor",
    "  # 实测折算（净动态、已做漏电校正），**只能用于相对排序**；档位与 freq_khz 逐档对齐。",
]


def generic_profile(soc: str):
    """按约定为无手写档案的 SoC 合成一份 [soc] 档案；缺输入文件返回 None"""
    doc = ROOT / "mdocs" / soc / f"{soc}-freq-power.md"
    soc_yaml = ROOT / "module" / "config" / soc / "soc.yaml"
    if not doc.exists() or not soc_yaml.exists():
        return None
    prof = {
        "doc": doc,
        "soc": soc_yaml,
        "out_new": ROOT / "module" / "config" / soc / "frontier_policy.yaml",
        "prime_top_from_khz": None,  # 落表时按 prime 表倒数第二档现算
        "anchors": {},               # 无独立公布值：不写对账段
        "compute_unit": BATCH_COMPUTE_UNIT,
        "power_comment": BATCH_POWER_COMMENT,
    }
    try:
        tables = parse_cluster_tables(doc)
    except SystemExit:
        return None
    if "prime" in tables and len(tables["prime"]) >= 2:
        prof["prime_top_from_khz"] = tables["prime"][-2][0]
    return prof


def resolve_profile(soc: str):
    """取该 SoC 的档案：手写优先，其次通用合成；都取不到则 None"""
    if soc in SOC_PROFILES:
        return SOC_PROFILES[soc]
    return generic_profile(soc)


# [parse_doc] ---------------------------------------------------------------

def parse_cluster_tables(path: Path):
    """解析 §3 的三张簇级表 → {簇: [(freq_khz, power_w, compute_mdmips), ...]}"""
    text = path.read_text(encoding="utf-8")
    out = {}
    for cluster, zh in (("little", "little"), ("big", "big"), ("prime", "prime")):
        m = re.search(rf"^####\s*{zh}\s*簇.*?$", text, re.M)
        if not m:
            # 单簇 / 双簇 SoC（如 msm8917 只有 little、msm8996 只有 little+big）：
            # 表不存在 = 该簇不存在，跳过；至少一张表由调用方兜底断言
            continue
        rows = []
        for line in text[m.end():].splitlines():
            line = line.strip()
            if not line:
                if rows:
                    break
                continue
            if not line.startswith("|"):
                if rows:
                    break
                continue
            cells = [c.strip() for c in line.strip("|").split("|")]
            if len(cells) < 3 or not cells[0].isdigit():
                continue  # 表头 / 分隔行
            freq = int(cells[0])
            power = float(cells[1])
            compute = float(cells[2])
            rows.append((freq, power, compute))
        if not rows:
            raise SystemExit(f"{cluster} 簇级表为空")
        out[cluster] = rows
    if not out:
        raise SystemExit("文档 §3 未找到任何簇级表（至少需要 little）")
    return out


def monotone_compute(tables):
    """把每簇算力列修正为随频率**单调不减**，并报告被抬起的档位

    算力在物理上必然随频率单调不减（同簇同核数，频率越高算力只会持平或上升）。
    实测表出现逆序 = CoreMark 单档噪声（sm8350 prime 1420800 档 coremarks_per_mhz
    5.22，邻档 8.83 / 7.75），而生成器的 hull 与桶边界都要拿算力排序：
    非单调列会让「hull 序」与「combo_units 序」互相矛盾，产出 lo_units 非单调的桶表，
    运行期二分查表随即落到错桶（见 07-todos 的 sm8350 条）。

    修法取**保守侧**：逆序档抬到「到此为止见过的最大值」（前缀最大值），
    即把测低的档当作「至少与前档同算力」，不用邻档插值（会凭空造出未测点）。
    返回 (修正后的 tables, [(簇, 频率, 原值, 修正值), ...])，供调用方打印告警。
    """
    out, fixes = {}, []
    for cluster, rows in tables.items():
        fixed, run = [], None
        for freq, power, compute in rows:
            if run is None or compute > run:
                run = compute
            elif compute < run:
                fixes.append((cluster, freq, compute, run))
            fixed.append((freq, power, run))
        out[cluster] = fixed
    return out, fixes


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


# [frontier] ----------------------------------------------------------------

def enumerate_frontier(tables):
    """三簇档位全组合枚举（档数×档数×档数，随 soc.yaml 频表变化，8550 现为 6358、zumapro 为 3051）
    → 每个总算力档位取最小总功耗"""
    # 缺失簇用「单档 0 频/0 功耗/0 算力」占位：枚举与枚举后求和都退化成该簇不存在，
    # 单簇 SoC 因此与三簇 SoC 走同一条代码路径（0 档不贡献算力，选中也不会写非零 kHz）
    missing = [(0, 0.0, 0.0)]
    little = tables.get("little", missing)
    big = tables.get("big", missing)
    prime = tables.get("prime", missing)
    best = {}
    for fl, pl, cl in little:
        for fb, pb, cb in big:
            for fp, pp, cp in prime:
                total_c = round(cl + cb + cp, 3)
                total_p = pl + pb + pp
                cur = best.get(total_c)
                if cur is None or total_p < cur[0]:
                    best[total_c] = (total_p, (fl, fb, fp))
    return best


def lower_hull(points):
    """(总算力, 总功耗, 频率组合) 的单调下凸包（保留左转，得到凸的下包络）

    几何只用前两维；第三维（该点的 little/big/prime 频率三元组）原样带出，
    供运行期把「需求桶」直接映射成各簇目标频点。
    """
    pts = sorted(points)
    hull = []
    for p in pts:
        while len(hull) >= 2:
            x1, y1 = hull[-2][0], hull[-2][1]
            x2, y2 = hull[-1][0], hull[-1][1]
            cross = (x2 - x1) * (p[1] - y1) - (y2 - y1) * (p[0] - x1)
            if cross <= 0:
                hull.pop()
            else:
                break
        hull.append(p)
    return hull


def efficiency(x1, y1, x2, y2):
    """区段的边际能效 Mdmips/W"""
    dp = y2 - y1
    return (x2 - x1) / dp if dp > 0 else float("inf")


# 真机容量单位口径（文档 §2.2）: 单核算力 = cpu_capacity × f / f_max；簇算力 = 单核 × 核数
# 核数不写死，由 soc.yaml [topology] 的区间长度算（8550 → 3/4/1，zumapro → 4/3/1）


def combo_units(combo, cap, freqs, cores):
    """频率三元组 → 总算力（真机容量单位）；capacity / f_max / 核数 缺失的簇按 0 计（调用方回退）"""
    total = 0.0
    for cluster, f in zip(CLUSTERS, combo):
        f_max = freqs.get(cluster, [0])[-1] if freqs.get(cluster) else 0
        if f_max:
            total += cores.get(cluster, 0) * cap.get(cluster, 0) * f / f_max
    return total


def check_anchors(hull, tables, anchors, prime_top_from_khz):
    """复算自检：与文档公布值对账（容差 3%）

    口径易错点（已修）: "冲顶段 600 Mdmips/W" 指的是 **prime 表自己**末两档的边际能效，
    不是下凸包末段（末段可能混簇，值会明显不同）。
    """
    # 批量机型无独立公布值：不编造期望值，直接不对账（块内也不写对账段，避免自产自对）
    if not anchors:
        return []
    results = []
    xs = [p[0] for p in hull]
    ys = [p[1] for p in hull]
    prime = tables["prime"]
    top_from = next(
        (r for r in prime if prime_top_from_khz is not None and r[0] >= prime_top_from_khz),
        prime[-2],
    )
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


# [buckets] -----------------------------------------------------------------

def build_buckets(hull, cap, freqs, cores):
    """前沿点两两之间构成需求桶；桶边界取相邻点算力（**真机容量单位**）的中点

    每个桶直接带该前沿点的三簇目标频点（little_khz/big_khz/prime_khz）——运行期
    查表即可，不用在设备上重算 Cx·f·V²。纯几何定义，不依赖 SLO。

    **不变式（落表前硬断言）**：`units` 必须严格非降。运行期 `bucket_for_demand()`
    对 `lo_units` 做二分（partition_point），数组一旦非单调，查表结果未定义——不是慢，
    是**错桶**：需求落到相邻桶、目标频点偏低（过度降频）或本应有桶却回退比例路径。
    `units` 用的是 capacity×f/f_max 折算算力，与文档 §3 算力列**不同源**，所以
    「文档算力单调」推不出「units 单调」；这里显式兜底，坏表一律拒绝落盘。
    """
    units = [combo_units(p[2], cap, freqs, cores) for p in hull]
    bad = [
        (i, units[i], units[i + 1])
        for i in range(len(units) - 1)
        if units[i + 1] < units[i] - 1e-9
    ]
    if bad:
        detail = "、".join(f"#{i} {a:.1f} → #{i + 1} {b:.1f}" for i, a, b in bad[:5])
        raise SystemExit(
            f"桶表不可落盘：hull 的 combo_units 非单调（{len(bad)} 处逆序：{detail}）。"
            "运行期二分查表会落到错桶。先查文档 §3 算力列是否非单调（本脚本已按前缀"
            "最大值自动修，仍有逆序说明 capacity / freq_khz 与文档 §3 表不一致）。"
        )
    buckets = []
    for i, p in enumerate(hull):
        lo = units[i] if i == 0 else (units[i - 1] + units[i]) / 2.0
        hi = units[-1] if i == len(hull) - 1 else (units[i] + units[i + 1]) / 2.0
        buckets.append({
            "point_index": i,
            "lo_units": round(lo, 1),
            "hi_units": round(hi, 1),
            "target_compute": round(p[0], 1),
            "target_power_w": round(p[1], 4),
            "margin_mdmips_per_w": (
                round(efficiency(p[0], p[1], hull[i + 1][0], hull[i + 1][1]), 1)
                if i + 1 < len(hull) else 0.0
            ),
            "little_khz": p[2][0],
            "big_khz": p[2][1],
            "prime_khz": p[2][2],
        })
    return buckets


# [modes] -------------------------------------------------------------------

def build_modes(buckets, lead_frac, reserve):
    """三模式结果向量：桶 → 目标档/倾向 + 预判交叠带（上升/下降对称）

    enter_units / exit_units = 「距桶边界还剩多少真机容量单位就交叠 / 撤出」；
    两种临界点（需求跨桶、落点贴顶）共用同一组提前量，乘子 reduce 0.5 / default 1.0 / boost 2.0。
    """
    modes = {}
    for mode, mult in MODE_LEAD_MULT.items():
        lead = lead_frac * mult
        rows = []
        for b in buckets:
            span = max(b["hi_units"] - b["lo_units"], 1.0)
            band = round(span * lead, 1)
            rows.append({
                "point_index": b["point_index"],
                "target_compute": b["target_compute"],
                "target_power_w": b["target_power_w"],
                "enter_units": band,
                "exit_units": band,
            })
        modes[mode] = {
            "lead_mult": mult,
            "lead_frac": round(lead, 4),
            "reserve_pct": reserve[mode],
            "buckets": rows,
        }
    return modes


# [emit] --------------------------------------------------------------------

def fingerprint(tables, cap, freqs):
    """确定性指纹: 只由输入内容决定，不含生成时刻"""
    h = hashlib.sha256()
    for c in CLUSTERS:
        for freq, power, compute in tables.get(c, []):
            h.update(f"{c}:{freq}:{power:.4f}:{compute:.4f}\n".encode())
    for c in CLUSTERS:
        h.update(f"cap:{c}:{cap.get(c, 0)}\n".encode())
        h.update(f"freq:{c}:{','.join(str(f) for f in freqs.get(c, []))}\n".encode())
    return h.hexdigest()[:16]


def render_block(tables, cap, freqs, hull, buckets, modes, enabled, prof):
    # cores 未入参：本块只渲染 per_cluster 的 capacity/freq/power 与前沿/桶表，核数已折算进
    # combo_units（build_buckets 阶段），落块不需要它（a818354 曾传入但函数体内未使用）
    fp = fingerprint(tables, cap, freqs)
    L = []
    L.append(BLOCK_BEGIN)
    L.append("# 由 scripts/frontier_policy.py 生成；受控块替换，块外内容请勿手改本块")
    L.append(f"# 口径: 能效模型 {prof['doc'].relative_to(ROOT)} §3 簇级表 + 本文件 capacity/freq_khz")
    L.append("# 运行期: 只查表落点与逐档功耗（不重算 Cx·f·V²）；表缺失/指纹不符/桶越界 → 回退既有参数组")
    L.append("frontier_policy:")
    L.append(f"  enabled: {str(enabled).lower()}")
    L.append(f'  model_fingerprint: "{fp}"')
    L.append("  # TODO 待拍板: reserve_pct（流畅度下界）与 lead_frac（预判基准提前量）仍为占位值；")
    L.append("  # 真机观察后重跑本脚本覆盖（--reserve / --lead-frac），不要手改本块。")
    anchor_rows = check_anchors(hull, tables, prof["anchors"], prof["prime_top_from_khz"])
    if anchor_rows:
        L.append("  # 复算自检锚点（与文档 §5/§7 公布值对账，容差 3%）")
        L.append("  anchors:")
        for name, got, want, ok in anchor_rows:
            L.append(f"    - {{ name: \"{name}\", got: {round(got, 3)}, want: {want}, ok: {str(ok).lower()} }}")
    else:
        L.append("  # 无独立公布值可对账：本机型不设 anchors 段（实测折算表，逐档见能效模型文档 §3）")
    L.append(f"  # 前沿点（{len(hull)} 点；compute={prof['compute_unit']}、power=W）")
    L.append("  frontier:")
    for x, y, _ in hull:
        L.append(f"    - {{ compute: {round(x, 1)}, power_w: {round(y, 4)} }}")
    for ln in prof["power_comment"]:
        L.append(ln)
    L.append("  per_cluster:")
    for c in CLUSTERS:
        # 缺席簇（单簇/双簇 SoC）不落 per_cluster：查表侧按「0 = 表未提供」处理，写空数组反而误导
        if not tables.get(c):
            continue
        if len(tables[c]) != len(freqs.get(c, [])):
            raise SystemExit(
                f"{c} 档位数不一致：文档 §3 有 {len(tables[c])} 档、soc.yaml freq_khz 有 "
                f"{len(freqs.get(c, []))} 档——先对齐档位再落表"
            )
        freqs_txt = ", ".join(str(f) for f, _, _ in tables[c])
        power_txt = ", ".join(f"{p:.4f}" for _, p, _ in tables[c])
        L.append(
            f"    {c}: {{ capacity: {cap.get(c, 0)}, freq_khz: [{freqs_txt}],"
            f" power_w: [{power_txt}] }}"
        )
    L.append("  # 需求桶: lo_units/hi_units 是真机容量单位边界；三簇 kHz 是查表落点（运行期仍需 floor 对齐）")
    L.append("  demand_buckets:")
    for b in buckets:
        L.append(
            "    - { point: %d, lo_units: %s, hi_units: %s, little_khz: %d, big_khz: %d, prime_khz: %d,"
            " target_compute: %s, target_power_w: %s, margin_mdmips_per_w: %s }"
            % (b["point_index"], b["lo_units"], b["hi_units"], b["little_khz"], b["big_khz"],
               b["prime_khz"], b["target_compute"], b["target_power_w"],
               b["margin_mdmips_per_w"])
        )
    L.append("  # 三模式结果向量（lead_mult 已定: reduce 0.5 / default 1.0 / boost 2.0）")
    L.append("  modes:")
    for mode in ("reduce", "default", "boost"):
        m = modes[mode]
        L.append(f"    {mode}:")
        L.append(f"      lead_mult: {m['lead_mult']}")
        L.append(f"      lead_frac: {m['lead_frac']}")
        L.append(f"      reserve_pct: {m['reserve_pct']}")
        L.append("      buckets:")
        for r in m["buckets"]:
            L.append(
                "        - { point: %d, target_compute: %s, target_power_w: %s,"
                " enter_units: %s, exit_units: %s }"
                % (r["point_index"], r["target_compute"], r["target_power_w"],
                   r["enter_units"], r["exit_units"])
            )
    L.append("  # 预判交叠带: 上升沿/下降沿共用同一组提前量（enter=exit），单位=真机容量单位")
    L.append("  anticipate:")
    L.append("    min_dwell_ticks: 20")
    L.append("    adjacent_only: true")
    L.append("    mode_mul: { reduce: 0.5, default: 1.0, boost: 2.0 }")
    L.append(BLOCK_END)
    return "\n".join(L) + "\n"


# [check] -------------------------------------------------------------------

def existing_block(path: Path):
    if not path.exists():
        return None
    text = path.read_text(encoding="utf-8")
    b = text.find(BLOCK_BEGIN)
    e = text.find(BLOCK_END)
    if b < 0 or e < 0:
        return None
    return text[b:e + len(BLOCK_END)] + "\n"


def write_block(path: Path, block: str):
    """受控块替换: 块内替换、块外逐字保留；无标记块时只追加

    尾部空行处理: 块后若残留一个换行则吃掉，避免每轮多留一个空行（保证重复写幂等）。
    """
    if not block.endswith("\n"):
        block += "\n"
    # 读走默认换行转换（CRLF→LF）、写固定 LF：git 的归一化目标就是 LF，
    # 否则 Windows 上 write_text 会把整份文件翻成 CRLF，制造全文件级 diff
    if path.exists():
        text = path.read_text(encoding="utf-8")
    else:
        text = ""
    b, e = text.find(BLOCK_BEGIN), text.find(BLOCK_END)
    if b >= 0 and e >= 0:
        tail = text[e + len(BLOCK_END):]
        if tail.startswith("\n"):
            tail = tail[1:]
        new = text[:b] + block + tail
    else:
        sep = "" if (not text or text.endswith("\n")) else "\n"
        new = text + sep + ("\n" if text else "") + block
    with path.open("w", encoding="utf-8", newline="\n") as fh:
        fh.write(new)


def selftest():
    """写盘路径自检（仓库内临时目录，用完即删，不碰仓库文件）：块外逐字保留 / 块内替换 / 重复写幂等"""
    import shutil
    d = ROOT / "devimpbin" / "tmp_selftest"
    shutil.rmtree(d, ignore_errors=True)
    d.mkdir(parents=True, exist_ok=True)
    try:
        p = d / "t.yaml"
        p.write_text("# head\nkeep: 1\n", encoding="utf-8")

        def blk(tag):
            return BLOCK_BEGIN + "\n" + tag + "\n" + BLOCK_END + "\n"

        write_block(p, blk("v1"))
        assert p.read_text(encoding="utf-8") == "# head\nkeep: 1\n\n" + blk("v1"), "首次追加不符"
        write_block(p, blk("v2"))
        assert p.read_text(encoding="utf-8") == "# head\nkeep: 1\n\n" + blk("v2"), "块内替换不符"
        write_block(p, blk("v2"))
        assert p.read_text(encoding="utf-8") == "# head\nkeep: 1\n\n" + blk("v2"), "重复写不幂等"
    finally:
        shutil.rmtree(d, ignore_errors=True)
    print("selftest OK: 块外逐字保留 / 块内替换 / 重复写幂等")
    return 0


# [main] --------------------------------------------------------------------

def main(argv=None):
    ap = argparse.ArgumentParser(description="帕累托前沿结果向量生成器（离线）")
    ap.add_argument("--write", action="store_true", help="写回目标文件（默认 dry-run 只打印）")
    ap.add_argument("--check", action="store_true", help="与现有块逐字节比对")
    ap.add_argument("--soc", default=DEFAULT_SOC,
                    help="目标机型键（手写档案见 [soc] 段；批量机型需先有 mdocs/<soc>/ 文档；缺省 sm8550）")
    ap.add_argument("--selftest", action="store_true", help="写盘路径自检（仓库内临时目录，不碰仓库文件）")
    ap.add_argument("--force", action="store_true",
                    help="锚点对账未过时仍允许 --write（默认拒绝落表）")
    ap.add_argument("--target", choices=("new", "soc"), default="soc",
                    help="soc = 并入本机 soc.yaml（受控块当前落点，默认）;"
                         " new = 独立 frontier_policy.yaml（运行期不消费，仅迁移用）")
    ap.add_argument("--lead-frac", type=float, default=DEFAULT_LEAD_FRAC,
                    help="预判基准提前量（占需求桶跨度比例）TODO 待拍板")
    ap.add_argument("--reserve", type=float, default=None,
                    help="统一覆盖三模式的流畅度下界 reserve_pct（TODO 待拍板）")
    ap.add_argument("--disable", action="store_true",
                    help="生成的块写 enabled: false（该 SoC 回退既有参数组，不查表）")
    args = ap.parse_args(argv)
    if args.check and args.write:
        ap.error("--check 与 --write 不能同时使用（前者只比对、后者只落盘）")

    if args.selftest:
        return selftest()

    prof = resolve_profile(args.soc)
    if prof is None:
        raise SystemExit(f"没有 {args.soc} 的档案：手写档案见 [soc] 段，或先跑 devimpbin/batch_ingest.py 产出 mdocs/<soc>/ 文档")

    for p in (prof["doc"], prof["soc"]):
        if not p.exists():
            raise SystemExit(f"缺少输入: {p}")

    tables = parse_cluster_tables(prof["doc"])
    # 算力列先修单调：非单调列会让 hull 序与 combo_units 序互相矛盾 → 桶表 lo_units 非单调
    tables, compute_fixes = monotone_compute(tables)
    if compute_fixes:
        print(f"⚠ 算力列非单调 {len(compute_fixes)} 处，已按前缀最大值抬起：", file=sys.stderr)
        for cluster, freq, was, now in compute_fixes:
            print(f"    {cluster} {freq} kHz: {was:.1f} → {now:.1f} CMdmips", file=sys.stderr)
    cap, freqs, cores = parse_soc(prof["soc"])
    counts = {c: len(tables.get(c, [])) for c in CLUSTERS}
    # 不再写死「期望 little16/big20/prime21」：档位数随 SoC 变，只查退化表
    # 只为「有表但档数<2」告警；表都不存在的簇是该 SoC 本来就没有那个簇，不是缺数据
    thin = {c: n for c, n in counts.items() if 0 < n < 2}
    if thin:
        print(f"⚠ 档位表过稀: {thin}（有表但不足 2 档，边际能效无从计算）", file=sys.stderr)
    # 只对「有簇表却缺核数」告警：单簇/双簇 SoC 的缺席簇不是缺数据（它本来就没有那个簇）
    no_cores = [c for c in CLUSTERS if tables.get(c) and c not in cores]
    if no_cores:
        print(f"⚠ [topology] 有簇表却缺核数: {no_cores}", file=sys.stderr)

    combos = enumerate_frontier(tables)
    # 下凸包几何只吃 (总算力, 总功耗)，但把频率三元组带出（第三维）供运行期查表
    hull = lower_hull([(x, v[0], v[1]) for x, v in combos.items()])
    checks = check_anchors(hull, tables, prof["anchors"], prof["prime_top_from_khz"])
    n_combos = len(combos)
    buckets = build_buckets(hull, cap, freqs, cores)
    reserve = dict(DEFAULT_RESERVE)
    if args.reserve is not None:
        reserve = {k: args.reserve for k in reserve}
    modes = build_modes(buckets, args.lead_frac, reserve)
    block = render_block(tables, cap, freqs, hull, buckets, modes, not args.disable, prof)

    print(f"SoC: {args.soc}")
    print(f"输入: {prof['doc'].relative_to(ROOT)} / {prof['soc'].relative_to(ROOT)}")
    print(f"档位: {counts}  核数: {cores}  组合: {n_combos}  前沿点: {len(hull)}  桶: {len(buckets)}")
    print(f"指纹: {fingerprint(tables, cap, freqs)}")
    print("复算对账（容差 3%）:")
    for name, got, want, ok in checks:
        print(f"  [{'OK ' if ok else 'FAIL'}] {name}: got={round(got, 3)} want={want}")
    bad = [c for c in checks if not c[3]]
    if bad:
        print(f"⚠ {len(bad)} 项对账未过——先查口径再落表", file=sys.stderr)

    target = prof["out_new"] if args.target == "new" else prof["soc"]
    if args.check:
        cur = existing_block(target)
        if cur is None:
            print(f"--check: {target.relative_to(ROOT)} 内没有生成块（尚未落表）")
            return 1
        same = cur == block
        print(f"--check: {'一致（逐字节相同）' if same else '不一致（表过期或手改过）'}")
        return 0 if same else 1

    if args.write:
        # 锚点对账未过 = 本表已与文档公布值对不上（口径漂移）：落表等于把错表发到设备，直接拒绝
        if bad and not args.force:
            print("拒绝落表：锚点对账未过（确认口径无误后用 --force 强制写入）", file=sys.stderr)
            return 2
        write_block(target, block)
        print(f"已写入: {target.relative_to(ROOT)}（受控块替换，块外未动）")
    else:
        print("--- dry-run（--write 才落盘）---")
        print(block)
    return 0 if not bad else 2


if __name__ == "__main__":
    raise SystemExit(main())
