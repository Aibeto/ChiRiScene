#!/usr/bin/env python3
"""frontier_policy.py —— 帕累托前沿"结果向量"离线生成器

区块索引: [args] [parse_doc] [parse_soc] [frontier] [buckets] [modes] [emit] [check] [main]

定位（与计划 3.4 一致）:
  开发期离线工具，**不进设备、不进 cargo build、不被运行期调用**。设备侧只"查表"，不跑本脚本。
  用途: 读能效模型表 → 复算帕累托前沿 → 算出 reduce/default/boost 三模式的结果向量 → 输出可嵌入的 YAML 块。

输入（只读）:
  mdocs/8550/sm8550-freq-power.md   §3 的三张"簇级"表（little 16 / big 20 / prime 21 行）
                                    列 = 频率 kHz | 簇功耗 W | 簇算力 Mdmips
                                    （不自己乘核数: 用文档给出的簇级值，避免二次换算误差）
  module/config/8550/soc.yaml       capacity / freq_khz（对账与真机容量口径）

输出:
  默认 dry-run: 摘要 + YAML 块打到 stdout
  --write        受控块替换写入目标文件（块内替换、**块外逐字保留**；无标记块时只追加）
  --check        读取现有块并逐字节比对（确定性自检；与生成内容不一致则退出码 1）

约定（审查修正项）:
  * 纯标准库，不依赖 pyyaml（本机 pyyaml 有损坏史）
  * fingerprint 只由输入内容哈希决定，**不含生成时刻**（否则重跑字节不同）
  * 禁止整文件 dump 回写（会丢注释、属机械重排）

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
DOC = ROOT / "mdocs" / "8550" / "sm8550-freq-power.md"
SOC = ROOT / "module" / "config" / "8550" / "soc.yaml"
OUT_NEW = ROOT / "module" / "config" / "8550" / "frontier_policy.yaml"

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
# 复算对账锚点（文档 §5/§7 公布值；容差给 3%，用于发现口径漂移）
ANCHORS = {
    "compute_min": 2294.0,
    "compute_max": 13006.0,
    "power_min": 0.535,
    "power_max": 8.436,
    "hull_points": 46,
    "first_seg_mdmips_per_w": 3655.0,
    "last_seg_mdmips_per_w": 600.0,
}
ANCHOR_TOL = 0.03
# 文档 §5 原话："prime 冲顶段（2092800→3187200 只剩 600 Mdmips/W）" —— 指的是**整段摊薄**的边际，
# 不是顶部单档（2956800→3187200 单档只有约 470 Mdmips/W）。锚点按整段口径取。
PRIME_TOP_FROM_KHZ = 2092800


# [parse_doc] ---------------------------------------------------------------

def parse_cluster_tables(path: Path):
    """解析 §3 的三张簇级表 → {簇: [(freq_khz, power_w, compute_mdmips), ...]}"""
    text = path.read_text(encoding="utf-8")
    out = {}
    for cluster, zh in (("little", "little"), ("big", "big"), ("prime", "prime")):
        m = re.search(rf"^####\s*{zh}\s*簇.*?$", text, re.M)
        if not m:
            raise SystemExit(f"未找到 {cluster} 簇级表标题")
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
    return out


def parse_soc(path: Path):
    """从 soc.yaml 取 capacity 与 freq_khz（文本解析，不引入 yaml 库）

    先剥掉生成块再解析：块内 per_cluster/demand_buckets 也含 `little:` 形态的键，
    重复运行时不能让它污染 cap/freq 取值。
    """
    text = path.read_text(encoding="utf-8")
    b, e = text.find(BLOCK_BEGIN), text.find(BLOCK_END)
    if b >= 0 and e >= 0:
        text = text[:b] + text[e + len(BLOCK_END):]
    cap = {}
    for c in CLUSTERS:
        m = re.search(rf"^\s*{c}:\s*(\d+)\s*$", text, re.M)
        if m:
            cap[c] = int(m.group(1))
    freqs = {}
    for c in CLUSTERS:
        m = re.search(rf"^\s*{c}:\s*\[(.*?)\]", text, re.S | re.M)
        if m:
            freqs[c] = [int(x) for x in re.findall(r"\d+", m.group(1))]
    return cap, freqs


# [frontier] ----------------------------------------------------------------

def enumerate_frontier(tables):
    """6720 组合枚举 → 每个总算力档位取最小总功耗"""
    little, big, prime = (tables[c] for c in CLUSTERS)
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
CLUSTER_CORES = {"little": 3, "big": 4, "prime": 1}


def combo_units(combo, cap, freqs):
    """频率三元组 → 总算力（真机容量单位）；capacity / f_max 缺失的簇按 0 计（调用方回退）"""
    total = 0.0
    for cluster, f in zip(CLUSTERS, combo):
        f_max = freqs.get(cluster, [0])[-1] if freqs.get(cluster) else 0
        if f_max:
            total += CLUSTER_CORES[cluster] * cap.get(cluster, 0) * f / f_max
    return total


def check_anchors(hull, tables):
    """复算自检：与文档公布值对账（容差 3%）

    口径易错点（已修）: "冲顶段 600 Mdmips/W" 指的是 **prime 表自己**末两档的边际能效，
    不是下凸包末段（末段可能混簇，值会明显不同）。
    """
    results = []
    xs = [p[0] for p in hull]
    ys = [p[1] for p in hull]
    prime = tables["prime"]
    top_from = next((r for r in prime if r[0] >= PRIME_TOP_FROM_KHZ), prime[-2])
    top_seg = efficiency(top_from[2], top_from[1], prime[-1][2], prime[-1][1])
    checks = [
        ("总算力下限", min(xs), ANCHORS["compute_min"]),
        ("总算力上限", max(xs), ANCHORS["compute_max"]),
        ("总功耗下限", min(ys), ANCHORS["power_min"]),
        ("总功耗上限", max(ys), ANCHORS["power_max"]),
        ("前沿点数", len(hull), ANCHORS["hull_points"]),
        ("起手段 边际能效", efficiency(hull[0][0], hull[0][1], hull[1][0], hull[1][1]),
         ANCHORS["first_seg_mdmips_per_w"]),
        ("prime 冲顶段 边际能效", top_seg, ANCHORS["last_seg_mdmips_per_w"]),
    ]
    for name, got, want in checks:
        tol = abs(want) * ANCHOR_TOL if want else 0.0
        ok = abs(got - want) <= tol
        results.append((name, got, want, ok))
    return results


# [buckets] -----------------------------------------------------------------

def build_buckets(hull, cap, freqs):
    """前沿点两两之间构成需求桶；桶边界取相邻点算力（**真机容量单位**）的中点

    每个桶直接带该前沿点的三簇目标频点（little_khz/big_khz/prime_khz）——运行期
    查表即可，不用在设备上重算 Cx·f·V²。纯几何定义，不依赖 SLO。
    """
    units = [combo_units(p[2], cap, freqs) for p in hull]
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
        for freq, power, compute in tables[c]:
            h.update(f"{c}:{freq}:{power:.4f}:{compute:.4f}\n".encode())
    for c in CLUSTERS:
        h.update(f"cap:{c}:{cap.get(c, 0)}\n".encode())
        h.update(f"freq:{c}:{','.join(str(f) for f in freqs.get(c, []))}\n".encode())
    return h.hexdigest()[:16]


def render_block(tables, cap, freqs, hull, buckets, modes, enabled):
    fp = fingerprint(tables, cap, freqs)
    L = []
    L.append(BLOCK_BEGIN)
    L.append("# 由 scripts/frontier_policy.py 生成；受控块替换，块外内容请勿手改本块")
    L.append("# 口径: 能效模型 mdocs/8550/sm8550-freq-power.md §3 簇级表 + 本文件 capacity/freq_khz")
    L.append("# 运行期: 只查表落点与逐档功耗（不重算 Cx·f·V²）；表缺失/指纹不符/桶越界 → 回退既有参数组")
    L.append("frontier_policy:")
    L.append(f"  enabled: {str(enabled).lower()}")
    L.append(f'  model_fingerprint: "{fp}"')
    L.append("  # TODO 待拍板: reserve_pct（流畅度下界）与 lead_frac（预判基准提前量）仍为占位值；")
    L.append("  # 真机观察后重跑本脚本覆盖（--reserve / --lead-frac），不要手改本块。")
    L.append("  # 复算自检锚点（与文档 §5/§7 公布值对账，容差 3%）")
    L.append("  anchors:")
    for name, got, want, ok in check_anchors(hull, tables):
        L.append(f"    - {{ name: \"{name}\", got: {round(got, 3)}, want: {want}, ok: {str(ok).lower()} }}")
    L.append("  # 前沿点（46 点；compute=Mdmips(DT 口径)、power=W）")
    L.append("  frontier:")
    for x, y, _ in hull:
        L.append(f"    - {{ compute: {round(x, 1)}, power_w: {round(y, 4)} }}")
    L.append("  # 各簇频表 + 逐档**簇功耗**(W)：功耗列直接取文档 §3 簇级表，供放置成本模型做边际比较；")
    L.append("  # 两列等长且逐档对齐（同下标 = 同一档）。功耗只含动态项、不含漏电，**只能用于相对排序**。")
    L.append("  per_cluster:")
    for c in CLUSTERS:
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
    ap.add_argument("--selftest", action="store_true", help="写盘路径自检（仓库内临时目录，不碰仓库文件）")
    ap.add_argument("--force", action="store_true",
                    help="锚点对账未过时仍允许 --write（默认拒绝落表）")
    ap.add_argument("--target", choices=("new", "soc"), default="soc",
                    help="soc = 并入 module/config/8550/soc.yaml（受控块当前落点，默认）;"
                         "new = 独立 module/config/8550/frontier_policy.yaml（运行期不消费，仅迁移用）")
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

    for p in (DOC, SOC):
        if not p.exists():
            raise SystemExit(f"缺少输入: {p}")

    tables = parse_cluster_tables(DOC)
    cap, freqs = parse_soc(SOC)
    counts = {c: len(tables[c]) for c in CLUSTERS}
    if counts != {"little": 16, "big": 20, "prime": 21}:
        print(f"⚠ 档位数与文档不符: {counts}（期望 little16/big20/prime21）", file=sys.stderr)

    combos = enumerate_frontier(tables)
    # 下凸包几何只吃 (总算力, 总功耗)，但把频率三元组带出（第三维）供运行期查表
    hull = lower_hull([(x, v[0], v[1]) for x, v in combos.items()])
    checks = check_anchors(hull, tables)
    n_combos = len(combos)
    buckets = build_buckets(hull, cap, freqs)
    reserve = dict(DEFAULT_RESERVE)
    if args.reserve is not None:
        reserve = {k: args.reserve for k in reserve}
    modes = build_modes(buckets, args.lead_frac, reserve)
    block = render_block(tables, cap, freqs, hull, buckets, modes, not args.disable)

    print(f"输入: {DOC.relative_to(ROOT)} / {SOC.relative_to(ROOT)}")
    print(f"档位: {counts}  组合: {n_combos}  前沿点: {len(hull)}  桶: {len(buckets)}")
    print(f"指纹: {fingerprint(tables, cap, freqs)}")
    print("复算对账（容差 3%）:")
    for name, got, want, ok in checks:
        print(f"  [{'OK ' if ok else 'FAIL'}] {name}: got={round(got, 3)} want={want}")
    bad = [c for c in checks if not c[3]]
    if bad:
        print(f"⚠ {len(bad)} 项对账未过——先查口径再落表", file=sys.stderr)

    target = OUT_NEW if args.target == "new" else SOC
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
