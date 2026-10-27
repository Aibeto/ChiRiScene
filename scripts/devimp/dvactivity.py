#!/usr/bin/env python3
r"""dvactivity.py —— app / activity 级规则取证探针（45 列 +activity 包专用）。

用法:
    python scripts/devimp/dvactivity.py <解压目录|批次目录|单文件> [--since MMDD-HHMMSS]
        [--min-n 30] [--top 25] [--out 路径]

区块索引: [why] [collect] [agg] [cand] [report]

[why] 定位
- `rules.yaml` 的 `app_modes` / `activity_params` 只能靠**真机取样**下规则：键要不要写、
  写成 boost 还是 reduce，判据是「组内多数核已过升频阈值（over_cores）」与
  「组内多数核低于降频阈值（under_cores）」，配合 util 与 freq/max（贴顶程度）。
- 本脚本只做读数与统计，**不另立判读口径**；口径一律以
  `.cursor/commands/devimp-log-analysis.md` 的「三、判定要点」为准。
- 口径（沿用 2026-10-22 首版 activity 规则取证，现固化）：
  · over_cores  = 组内 util > up_threshold 的核数（性能不足信号）
  · under_cores = 组内 util < down_threshold 的核数（性能充裕信号）
  · freq/max    = 决策选频 cur_freq_khz ÷ 硬件上限 max_freq_khz（贴顶程度）
  · 样本 < --min-n 的 activity 不进规则候选表（只进总览，标 `样本不足`）

[collect] 读数口径
- 只吃 `main_*.log`（tick 行给调度信号，snap 行给 batt_p）。
- 行过滤同 dvcommon（`^\d{2}:\d{2}:\d{2}\.\d{3},`）；activity 列（末列）为 `-`/空 = 未知，
  单独归 `(-)` 桶，**不参与规则候选**。
- 包名优先取行内 `package` 列，缺失时回退文件名 `main_<pkg>_<ts>.log`。
- cluster 列直接是 little/big/prime 字符串（不按机型猜分界）。

[agg] 能量口径
- Wh 一律是 **样本等效**（sum(W)/3600，假设每条有效 snap 覆盖 1 s），与 dvpower 同口径；
  不做积分、不外推缺口、不跨包/模式边界。
- 放电方向取同批次 `status.csv` 的 `charge` 列（按秒戳对齐）——**禁止用 main 日志
  `batt_i` 符号判方向**：该列是整数化读数且各机型符号约定相反（PHB110 上 `batt_i<0`
  一侧 v≈4.0~4.2、P≈17 W，实为充电），按符号判会把能量占比算成权威值的 2 倍
  （2026-10-24 A02-14 踩坑，口径已固化进 05-maint [analysis]）。
- 只统计 `0 < batt_p <= 20 W` 的放电 snap 行（同 dvpower 的尖峰剔除口径；
  >20 W 与非正值为异常读数，不进均值/Wh，剔除数进 [7]）。

[cand] 候选判据（阈值集中在此，改一处即可）
- 性能不足（建议 boost）：big 组 over_cores 均值 >= 1.0 **或** freq/max 均值 >= 0.85
- 性能充裕（建议 reduce）：big 组 under_cores 均值 >= 2.0 **且** big util 均值 <= 0.35
- 两者都满足时同时出现在两表，人工裁决（不自动排他）
"""

import argparse
import collections
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dvcommon as dc  # noqa: E402

# [cand] 候选阈值（唯一定义处）
OVER_BIG_MIN = 1.0      # big 组平均过阈值核数 → 性能不足
FREQ_TOP_MIN = 0.85     # big 组平均选频/上限 → 贴顶
UNDER_BIG_MIN = 2.0     # big 组平均空闲核数 → 性能充裕
UTIL_BIG_MAX = 0.35     # big 组平均 util 上限（配合 UNDER 判定充裕）

CLUSTERS = ("little", "big", "prime")

PW_SANITY_MAX_W = 20.0  # snap 功率合理性上限（dvpower 同口径）：>20W 为异常读数不进均值/Wh


class Agg:
    """按 (cluster) 累积 tick 信号 + snap 功率。

    功率只留**放电侧**：方向取同批次 status.csv 的 `charge` 列（按秒戳对齐），
    **禁止用 main 日志 `batt_i` 符号判方向**——该列整数化且各机型符号相反，
    充电行会把「按包能耗」抬高 2~3 倍（PHB110 A02-14 实测 incallui 4.14 W →
    修正后 1.58 W）。另有 0 < W <= 20 的合理性过滤（同 dvpower）。`pn` 仍记
    全部方向已判定的 snap 行数。
    """

    __slots__ = ("n", "util", "over", "under", "fr", "p", "pn", "modes", "off",
                 "son", "soff", "nomap", "nomatch", "badw")

    def __init__(self):
        self.n = 0
        self.util = []
        self.over = []
        self.under = []
        self.fr = []
        self.p = []
        self.pn = 0
        self.modes = collections.Counter()
        self.off = 0      # snap 行中 screen_on=0 的行数（息屏轴判定用；tick 行该列恒 `-`）
        self.son = 0      # snap 行中 screen_on=1 的行数
        self.nomap = 0    # snap 行中「批次无 charge_map（status.csv 缺失/戳不匹配）」的行数
        self.nomatch = 0  # snap 行中「有 charge_map 但该秒未命中放电集合」的行数（多为充电/缺秒）
        self.badw = 0     # 放电侧被 0<W<=20 合理性过滤剔除的行数（同 dvpower 口径）

    def util_avg(self):
        return dc.avg(self.util)

    def over_avg(self):
        return dc.avg(self.over)

    def under_avg(self):
        return dc.avg(self.under)

    def fr_avg(self):
        return dc.avg(self.fr)

    def wh(self):
        return sum(self.p) / 3600.0

    def p_avg(self):
        return dc.avg(self.p)


def _pkg_from_name(path):
    base = os.path.basename(path)
    if not base.startswith("main_"):
        return "-"
    return base[len("main_"):].rsplit("_", 1)[0]


def load_charge_map(root):
    """[collect] 放电方向权威源 = status.csv 的 `charge`（**不用电流符号**）。

    返回 {批次戳: set("HH:MM:SS")}：该秒内 status.csv 记为 discharging。
    main 日志与同戳的 `x_<ts>/status.csv` 成对（devimp 侧在 `x_devimp_<ts>/`）。
    实测坑：main 日志 `batt_i` 是整数化读数且**各机型符号约定相反**
    （PHB110 A02-14 实测 batt_i<0 一侧 batt_v≈4.0~4.2 且 P≈17 W，实为充电），
    因此**禁止**用 batt_i 符号定方向。
    """
    import glob as _g
    out = {}
    for p in sorted(_g.glob(os.path.join(root, "**", "status.csv"), recursive=True)):
        stamp = re.search(r"(?:^x_(?:devimp_)?)(\d{4}-\d{6})$",
                          os.path.basename(os.path.dirname(p)))
        if not stamp:
            continue
        secs = out.setdefault(stamp.group(1), set())
        cols, recs = dc.read_status_file(p)
        for r in recs:
            if (r.get("charge") or "") != "discharging":
                continue
            t = (r.get("timestamp") or r.get("ts") or "")[:8]
            if len(t) == 8:
                secs.add(t)
    return out


def collect(files, idx, charge_map):
    """[collect] 逐文件读行 → key=(pkg, activity) → {cluster: Agg}；另返全局告警。"""
    data = collections.defaultdict(lambda: collections.defaultdict(Agg))
    unknown_rows = 0
    total_rows = 0
    for path in files:
        fpkg = _pkg_from_name(path)
        # 批次戳取**父目录名**（x_devimp_<ts>/），不能用文件名戳（main_<pkg>_<ts>.log 是分段戳）
        mb = re.search(r"(?:^x_(?:devimp_)?)(\d{4}-\d{6})$",
                       os.path.basename(os.path.dirname(path)))
        stamp = mb.group(1) if mb else None
        dis = charge_map.get(stamp) if stamp else None
        for p, rtype in dc.iter_rows(path, min_cols=45):
            total_rows += 1
            pkg = p[idx["package"]].strip() or fpkg
            act = p[idx["activity"]].strip() if len(p) > idx["activity"] else ""
            if not act or act == "-":
                act = "(-)"
                unknown_rows += 1
            agg = data[(pkg, act)][p[idx["cluster"]].strip() or "-"]
            if rtype == "snap":
                w = dc.num(p[idx["batt_p"]])
                if w is not None:
                    agg.pn += 1
                    scr = p[idx["screen_on"]].strip()
                    if scr == "0":
                        agg.off += 1
                    elif scr == "1":
                        agg.son += 1
                    if dis is None:
                        agg.nomap += 1
                    elif p[0][:8] in dis:
                        if 0.0 < w <= PW_SANITY_MAX_W:
                            agg.p.append(w)
                        else:
                            agg.badw += 1   # >20W/非正值为异常读数（dvpower 同口径）
                    else:
                        agg.nomatch += 1    # 有 map 但该秒未命中：多为充电秒
                continue
            if rtype != "tick":
                continue
            agg.n += 1
            agg.modes[p[idx["mode"]]] += 1
            agg.util.append(dc.num(p[idx["max_util"]]))
            agg.over.append(dc.num(p[idx["over_cores"]]))
            agg.under.append(dc.num(p[idx["under_cores"]]))
            cur, mx = dc.num(p[idx["cur_freq_khz"]]), dc.num(p[idx["max_freq_khz"]])
            agg.fr.append(cur / mx if cur and mx else None)
    return data, unknown_rows, total_rows


def row_line(pkg, act, sub, min_n):
    """单行表体：pkg/act + 三簇 (util/over/under) + big 贴顶 + 功率。"""
    def cell(c):
        a = sub.get(c)
        if not a or a.n == 0:
            return "     -/    - u   - "
        return "%5.1f/%5.1f u%4.2f" % (a.over_avg() * 1.0, a.under_avg() * 1.0, a.util_avg())
    big = sub.get("big")
    fr = big.fr_avg() if big and big.n else 0.0
    n = max((a.n for a in sub.values()), default=0)
    pn = sum(a.pn for a in sub.values())
    pdis = sum(len(a.p) for a in sub.values())
    wh = sum(a.wh() for a in sub.values())
    pav = sum(sum(x.p) for x in sub.values()) / pdis if pdis else 0.0
    off = sum(a.off for a in sub.values())
    on = sum(a.son for a in sub.values())
    base = off + on
    modes = collections.Counter()
    for a in sub.values():
        modes.update(a.modes)
    m = "/".join(k for k, _ in modes.most_common(2)) or "-"
    flag = "" if n >= min_n else " 样本不足"
    return (f"{pkg[:26]:<26s} {act[:34]:<34s} {m:<10s} {n:6d} "
            f"{cell('little')} {cell('big')} {cell('prime')} {fr:5.2f} "
            f"{off * 100.0 / base if base else 0:5.1f}% {pn:6d} {pdis:6d} "
            f"{pav:5.2f} {wh:7.3f}{flag}")


HEAD = (f"{'package':<26s} {'activity':<34s} {'mode':<10s} {'n':>6s} "
        f"{'little ov/un/u':<18s} {'big ov/un/u':<18s} {'prime ov/un/u':<18s} "
        f"{'f/max':>5s} {'屏off':>6s} {'snap':>6s} {'放电n':>6s} {'P_avg':>5s} {'Wh':>7s}")


def main(argv=None):
    dc.setup_console()
    ap = argparse.ArgumentParser()
    ap.add_argument("target", help="解压根目录 / 批次目录 / 单个 main_*.log")
    ap.add_argument("--since", default="0000-000000")
    ap.add_argument("--min-n", type=int, default=30)
    ap.add_argument("--top", type=int, default=25)
    ap.add_argument("--out", default=None)
    a = ap.parse_args(argv)

    if os.path.isfile(a.target):
        files = [a.target]
        workdir = os.path.dirname(a.target)
    else:
        files = dc.list_files(a.target, ("main_",), a.since)
        workdir = a.target
    if not files:
        print("[!] 未找到 main_*.log（--since 过大或目录不对）")
        return 2

    heads = [dc.parse_head(f) for f in files]
    fp = dc.fingerprint(heads)
    cols, tag = heads[0]["cols"], heads[0]["schema_tag"]
    if cols is None or "activity" not in cols:
        print("[!] 该包不是 45 列 (+activity) 格式，activity 级取证无意义")
        return 2
    idx = dc.index_map(cols)

    # 单文件输入时上溯两级找解包根（main 在 <root>/<tag>/x_devimp_<ts>/，status 在 <root>/<tag>/x_<ts>/）
    croot = (os.path.dirname(os.path.dirname(os.path.abspath(files[0])))
             if os.path.isfile(a.target) else a.target)
    charge_map = load_charge_map(croot)
    data, unknown_rows, total_rows = collect(files, idx, charge_map)
    if not charge_map:
        print("[!] 未找到同批次 status.csv：能量列全空，只有调度信号可用")
    out = []
    out.append("# dvactivity —— app / activity 级规则取证")
    out.append(f"# target : {os.path.abspath(a.target)}")
    out.append(f"# 文件 {len(files)} 个；行 {total_rows}；activity 未知行 {unknown_rows} "
               f"({unknown_rows * 100.0 / max(total_rows, 1):.1f}%)")
    for k in ("module", "soc", "board", "model", "schema_tag"):
        if k in fp:
            out.append(f"# {k}: {sorted(fp[k])}")
    out.append("")

    # [report] 1. 按 app×activity（tick 样本降序）
    keys = sorted(data.keys(),
                  key=lambda k: -max((x.n for x in data[k].values()), default=0))
    out.append("## [1] 按 app × activity（util 列 = max_util 均值；ov/un = over/under_cores 均值；f/max = big 组选频贴顶）")
    out.append(HEAD)
    for k in keys[:a.top * 3]:
        out.append(row_line(k[0], k[1], data[k], a.min_n))
    out.append("")

    # [report] 2. 按 app 聚合（activity 合并）
    bypkg = collections.defaultdict(lambda: collections.defaultdict(Agg))
    for (pkg, _act), sub in data.items():
        for c, aagg in sub.items():
            t = bypkg[pkg][c]
            t.n += aagg.n
            t.pn += aagg.pn
            t.util += aagg.util
            t.over += aagg.over
            t.under += aagg.under
            t.fr += aagg.fr
            t.p += aagg.p
            t.off += aagg.off
            t.son += aagg.son
            t.nomap += aagg.nomap
            t.nomatch += aagg.nomatch
            t.badw += aagg.badw
            t.modes.update(aagg.modes)
    out.append("## [2] 按 app（activity 合并；用于 app 级规则）")
    out.append(HEAD)
    for pkg in sorted(bypkg, key=lambda p: -max((x.n for x in bypkg[p].values()), default=0))[:a.top]:
        out.append(row_line(pkg, "(*)", bypkg[pkg], a.min_n))
    out.append("")

    # [cand] 3/4. 规则候选
    def cand_rows(pred, title, hint):
        out.append(f"## {title}（{hint}）")
        out.append(HEAD)
        hit = 0
        for k in keys:
            sub = data[k]
            if k[1] == "(-)":
                continue
            big = sub.get("big")
            if not big or big.n < a.min_n:
                continue
            if not pred(big):
                continue
            out.append(row_line(k[0], k[1], sub, a.min_n))
            hit += 1
            if hit >= a.top:
                break
        if not hit:
            out.append("（无）")
        out.append("")

    cand_rows(lambda b: b.over_avg() >= OVER_BIG_MIN or b.fr_avg() >= FREQ_TOP_MIN,
              "[3] 性能不足候选（建议 boost / 抬门槛）",
              f"判据 big over>={OVER_BIG_MIN} 或 f/max>={FREQ_TOP_MIN}")
    cand_rows(lambda b: b.under_avg() >= UNDER_BIG_MIN and b.util_avg() <= UTIL_BIG_MAX,
              "[4] 性能充裕候选（建议 reduce / 收紧平滑）",
              f"判据 big under>={UNDER_BIG_MIN} 且 util<={UTIL_BIG_MAX}")

    # [report] 5. 能量占比（样本等效 Wh，snap 行 batt_p）
    tot_wh = sum(x.wh() for sub in data.values() for x in sub.values())
    act_wh = sorted(((k, sum(x.wh() for x in sub.values())) for k, sub in data.items()),
                    key=lambda kv: -kv[1])
    out.append(f"## [5] 能量占比（样本等效 Wh = sum(W)/3600；总 {tot_wh:.3f} Wh）")
    out.append(f"{'package':<26s} {'activity':<34s} {'Wh':>8s} {'占比':>7s}")
    for k, w in act_wh[:a.top]:
        out.append(f"{k[0][:26]:<26s} {k[1][:34]:<34s} {w:8.3f} {w * 100.0 / tot_wh if tot_wh else 0:6.1f}%")
    out.append("")
    pkg_wh = sorted(((p, sum(x.wh() for x in sub.values())) for p, sub in bypkg.items()),
                    key=lambda kv: -kv[1])
    out.append("## [6] 能量占比（按 app）")
    out.append(f"{'package':<34s} {'Wh':>8s} {'占比':>7s}")
    for p, w in pkg_wh[:a.top]:
        out.append(f"{p[:34]:<34s} {w:8.3f} {w * 100.0 / tot_wh if tot_wh else 0:6.1f}%")
    out.append("")

    # [report] 7. 告警
    out.append("## [7] 告警")
    nmap = sum(x.nomap for sub in data.values() for x in sub.values())
    nmat = sum(x.nomatch for sub in data.values() for x in sub.values())
    nbad = sum(x.badw for sub in data.values() for x in sub.values())
    out.append(f"- 放电方向取自同批次 status.csv 的 `charge`（**不用 batt_i 符号**：main 侧 "
               f"batt_i 整数化且各机型符号约定相反）。批次无 charge_map（status.csv 缺失/"
               f"戳不匹配）的 snap 行 {nmap} 条、有 map 但该秒未命中放电集合（多为充电秒）"
               f"{nmat} 条——两类都不计能量。")
    out.append(f"- 放电侧被合理性过滤剔除（非正值或 >{PW_SANITY_MAX_W:.0f} W 异常读数，"
               f"dvpower 同口径）的 snap 行 {nbad} 条。")
    out.append("- Wh / P_avg 只取放电侧 snap 行；`snap` 列是全部有效 snap 行数，`放电n` 是其中"
               "放电侧行数——两列差距大说明该 app 长期插着充电器，能量占比不适用于它。")
    out.append("- Wh 为样本等效口径（sum(W)/3600），不等同墙钟积分能量；跨包/跨模式边界不积分。")
    out.append("- `屏off` = 该键 snap 行中 screen_on=0 的占比：占比高说明负载发生在息屏，"
               "属 SceneMode（息屏轴）地盘，不该靠亮屏侧的 boost/reduce 去调。")
    out.append("- `(-)` 桶 = activity 未采到（息屏/无前台/采样节流），不参与规则候选；"
               "未知行占比高说明该机 activity 采样命中率低，规则覆盖面要先看这个数。")
    if len(fp.get("module", [])) > 1 or len(fp.get("model", [])) > 1:
        out.append(f"- [!] 混版本/混机型：{ {k: sorted(v) for k, v in fp.items()} }")

    path = a.out or os.path.join(workdir, "activity.txt")
    p = dc.write_report(os.path.dirname(path) or ".", os.path.basename(path), out)
    n_act = len([k for k in data if k[1] != "(-)"])
    dc.announce("dvactivity", p, [
        f"定版 {sorted(fp.get('module', ['?']))[0]} | {tag}",
        f"app {len(bypkg)} 个，activity 键 {n_act} 个（样本 >= {a.min_n} 的 "
        f"{len([k for k in data if k[1] != '(-)' and max((x.n for x in data[k].values()), default=0) >= a.min_n])} 个）",
        f"样本等效总能量 {tot_wh:.3f} Wh；activity 未知行占比 "
        f"{unknown_rows * 100.0 / max(total_rows, 1):.1f}%",
    ])
    return 0


if __name__ == "__main__":
    sys.exit(main())
