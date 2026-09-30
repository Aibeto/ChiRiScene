#!/usr/bin/env python3
"""dvpower.py —— 按包功耗归因（`status.csv` 权威口径 + devimp 侧交叉验证）。

用法:
    python scripts/devimp/dvpower.py <解压目录> [--since MMDD-HHMMSS] [--out 目录]

判读铁律（口径以 `.cursor/commands/devimp-log-analysis.md`「三、判定要点」为准）：
1. **功耗权威源 = `status.csv` 的 `charge` + `batt_power_w`**；devimp 侧的 `batt_p`/`P_avg`
   只作交叉验证。**历史口径（仅轻载场景/旧包适用）**：早期机型 `batt_i` 整数化，「排除
   batt_i==0」会删掉约 58% 的 <0.5A 轻载秒 → P_avg 高估（2026-09-24 实测 aweme 3.35 vs 2.14 W）。
   `logd_1001-045147` 起本机电流已到 0.2~5.5A，`[8]` 与 status.csv **几乎相等**（FAS 3.95 vs 3.95），
   该结论不再普遍成立，**不要外推到常规负载**（ChiRi 侧 `batt_power_w` 尺子已核实为准）。
2. **放电方向按 `charge` 列判**（`== discharging`），**不要用电流符号**（方向随内核/机型而异）。
3. 1 行 ≈ 1 秒采样，故 ΣW/3600 = Wh；表内 `n` 列是样本数（≈秒）。
4. `migrations`/`wakeups` 是 2s 差分，换算每秒要 ÷2（本脚本不统计迁移率，看 dvmain.py）。
5. **cap 序列不可跨批次连读**：daemon 因 `devimp/` 触顶 128MB 重启时把热状态重置回 100
   （本次 4.6 h 内 3 次）→ `[4]` 段同时给全量档位秒数与按批次明细。
6. `thermal_cap_pct` 各档**累计占用秒数**是判断「热是常态还是偶发」的主指标；
   定位「某档咬住多久 / 咬在什么温度带」看 `[5]` 迁移时间线与 `[6]` 档位温度区间，
   按 mode 看能量与 gpu/psi 加权看 `[7]`。

输出 `power.txt`：只写文件，stdout 只打几行摘要 + 路径（PowerShell 重定向会毁掉中文，见已知坑 16）。
"""

import argparse
import collections
import csv
import glob
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dvcommon as dc  # noqa: E402

STAMP_RE = re.compile(r"(\d{4}-\d{6})")


def cap_tier(raw):
    """`thermal_cap_pct` 归一到整数档。

    status.csv 偶发浮点残差（实测 `70.00001`）——不归一会在档位表里裂成两个档，
    让「各档累计秒数」这个主指标失真。
    """
    v = dc.num(raw)
    if v is None:
        return (raw or "-").strip() or "-"
    return str(int(round(v))) if abs(v - round(v)) < 0.01 else f"{v:g}"


def status_files(root, since):
    """收集 root 下（递归）所有 `status.csv`。

    status.csv 的 `timestamp` 列**没有日期** → `--since MMDD-HHMMSS` 只能按**父目录**
    `x_<MMDD-HHMMSS>/` 的批次戳筛（与其它探针的 `--since` 同口径：一律按 MMDD-HHMMSS 比大小）。
    目录名无批次戳的照收（不误删）。
    """
    found = []
    for p in glob.glob(os.path.join(root, "**", "status.csv"), recursive=True):
        m = STAMP_RE.search(os.path.basename(os.path.dirname(p)))
        if m and m.group(1) < since:
            continue
        found.append(p)
    return sorted(found)


def read_status(paths):
    """读 status.csv，返回 [(batch, row_dict)]。

    按**表头名**取值而非列位置：status.csv 会增列（实测已有 `screen_prop`，与 dvstatus 的
    22/23 列 STATUS_COLS 不一致），按名字取不会因加列而错位。
    """
    rows = []
    for fn in paths:
        batch = os.path.basename(os.path.dirname(fn))
        with open(fn, encoding="utf-8", errors="replace") as fh:
            for r in csv.DictReader(fh):
                if not (r.get("timestamp") or "").strip():
                    continue
                rows.append((batch, r))
    return rows


def _share(energy, total_wh):
    return energy / total_wh * 100 if total_wh else 0.0


def _t(v):
    """温度列格式化：缺失/异常（None）→ `?`，别让 None 进 f-string 变成 'nan'。"""
    return "?" if v is None else f"{v:.0f}"


def _row_mode_pkg(mode, pkg, v, energy, total_wh):
    """[1] 段一行：mode × package 全字段。"""
    pw = sorted(v["pw"])
    n = len(pw)
    return (f"{mode:9s} {pkg[:26]:26s} {n:6d} {sum(pw) / n:6.2f} "
            f"{dc.pct(pw, .5):5.2f} {dc.pct(pw, .95):6.2f} {pw[-1]:6.2f} "
            f"{dc.avg(v['bt']):5.1f} {dc.avg(v['ct']):5.1f} {dc.avg(v['cap']):4.0f} "
            f"{dc.avg(v['gpu']):5.1f} {dc.avg(v['psi']):6.2f} "
            f"{energy:5.2f} {_share(energy, total_wh):5.1f}%")


def _row_pkg(pkg, v, energy, total_wh):
    """[2] 段一行：跨 mode 的 package 汇总。"""
    pw = sorted(v["pw"])
    n = len(pw)
    return (f"{pkg[:28]:28s} {n:6d} {sum(pw) / n:6.2f} {dc.pct(pw, .5):5.2f} "
            f"{dc.pct(pw, .95):6.2f} {pw[-1]:6.2f} {energy:5.2f} "
            f"{_share(energy, total_wh):5.1f}%")


def status_report(paths, rows):
    """返回 (输出行 list, 摘要 dict)。"""
    out = []
    charge = collections.Counter()
    batch_secs = collections.Counter()
    batch_dis = collections.Counter()
    cap_secs = collections.Counter()
    batch_cap = collections.defaultdict(collections.Counter)
    agg = collections.defaultdict(lambda: collections.defaultdict(list))
    tot = collections.defaultdict(lambda: collections.defaultdict(list))
    span = [None, None]
    seq = collections.defaultdict(list)        # 批次 → 按序样本（含充电行，档位时间线要连续）
    tier_temp = collections.defaultdict(list)
    tier_pkg = collections.defaultdict(collections.Counter)

    for batch, r in rows:
        ch = (r.get("charge") or "-").strip() or "-"
        charge[ch] += 1
        batch_secs[batch] += 1
        cap_raw = cap_tier(r.get("thermal_cap_pct"))
        cap_secs[cap_raw] += 1
        batch_cap[batch][cap_raw] += 1
        ts = (r.get("timestamp") or "").strip()[:12]
        if ts:
            span[0] = ts if span[0] is None or ts < span[0] else span[0]
            span[1] = ts if span[1] is None or ts > span[1] else span[1]
        mode = (r.get("mode") or "-").strip() or "-"
        pkg = (r.get("package") or "").strip() or "-"
        bt = dc.num(r.get("batt_temp"))
        seq[batch].append((ts, cap_raw, bt, mode, pkg))
        if bt is not None:
            tier_temp[cap_raw].append(bt)
        tier_pkg[cap_raw][pkg] += 1
        if ch != "discharging":
            continue
        pw = dc.num(r.get("batt_power_w"))
        if pw is None or pw <= 0:
            continue
        batch_dis[batch] += 1
        for d in (agg[(mode, pkg)], tot[pkg]):
            d["pw"].append(pw)
            d["bt"].append(dc.num(r.get("batt_temp")))
            d["ct"].append(dc.num(r.get("cpu_temp")))
            d["cap"].append(dc.num(r.get("thermal_cap_pct")))
            d["gpu"].append(dc.num(r.get("gpu_busy_pct")))
            d["psi"].append(dc.num(r.get("psi_cpu_some")))

    total_wh = sum(sum(d["pw"]) for d in agg.values()) / 3600.0
    total_n = sum(len(d["pw"]) for d in agg.values())

    out.append("# [1] 放电段 按 mode × package（status.csv 权威：charge=discharging 且 batt_power_w>0）")
    out.append("#     n = 样本数（≈秒）；能量 = ΣW/3600；% = 占放电总能量之比。")
    out.append(f"{'mode':9s} {'package':26s} {'n':>6s} {'avg W':>6s} {'p50':>5s} {'p95':>6s} "
               f"{'max':>6s} {'battT':>5s} {'cpuT':>5s} {'cap':>4s} {'gpu%':>5s} {'psi':>6s} "
               f"{'Wh':>5s} {'share':>6s}")
    for k in sorted(agg, key=lambda k: (-sum(agg[k]["pw"]), k[0], k[1])):
        e = sum(agg[k]["pw"]) / 3600.0
        out.append(_row_mode_pkg(k[0], k[1], agg[k], e, total_wh))
    out.append("")
    out.append("# [2] 放电段 按 package 汇总（跨 mode；归因看这张表）")
    out.append(f"{'package':28s} {'n':>6s} {'avg W':>6s} {'p50':>5s} {'p95':>6s} {'max':>6s} "
               f"{'Wh':>5s} {'share':>6s}")
    for k in sorted(tot, key=lambda k: -sum(tot[k]["pw"])):
        out.append(_row_pkg(k, tot[k], sum(tot[k]["pw"]) / 3600.0, total_wh))
    out.append("")
    out.append(f"# [3] 累计能量（仅放电）：{total_wh:.1f} Wh / {total_n / 3600.0:.1f} h，"
               f"均值 {total_wh * 3600.0 / total_n if total_n else 0:.2f} W"
               f"（{total_n} 个 ≈秒样本）")
    out.append("")

    out.append("# [4] thermal_cap_pct 各档累计占用秒数（**判断热是常态还是偶发的主指标**）")
    out.append("#     分母 = 全部行（含充电/无效行）；批次间 daemon 重启会把热状态重置回 100，")
    out.append("#     **cap 序列不可跨批次连读**，故下方给按批次明细。")
    out.append(f"  {'cap':>5s} {'秒':>7s} {'占比':>7s}")
    alln = sum(cap_secs.values())
    for cap_v, n in sorted(cap_secs.items(), key=lambda kv: -kv[1]):
        out.append(f"  {cap_v:>5s} {n:7d} {n / alln * 100 if alln else 0:6.1f}%")
    out.append("")
    out.append(f"  {'批次':22s} {'行':>7s} {'放电≈秒':>8s}  档位分布（按秒降序）")
    for b in sorted(batch_secs):
        dist = ", ".join(f"{k}:{v}" for k, v in batch_cap[b].most_common(5))
        out.append(f"  {b[:22]:22s} {batch_secs[b]:7d} {batch_dis[b]:8d}  {dist}")
    out.append("")

    out.append("# [5] cap 档迁移时间线（按批次；段 = 「起始时刻 → 该档」+ 温度走向 + 持续秒）")
    out.append("#     **判「卡在某一档多久」用**：段末温度若已明显低于该档跳闸点（feature.yaml 阈值 − 该档回滞），")
    out.append("#     说明这一档解除过慢（2026-09-28 实测：中档解除点 40℃ 低于软限 41℃，cap=60 有约一半")
    out.append("#     秒数落在 batt 40~41℃ 桶）——先查 `hysteresis_mid_c` 再谈 cap 值。")
    for b in sorted(batch_secs):
        segs = []
        for ts, cap, bt, _m, _p in seq.get(b, ()):
            if segs and segs[-1][0] == cap:
                segs[-1][3] = bt
                segs[-1][4] += 1
            else:
                segs.append([cap, ts, bt, bt, 1])
        out.append(f"  批次 {b}")
        for cap, ts, bt0, bt1, secs in segs:
            out.append(f"    {ts}  cap={cap:>3}  {_t(bt0)}→{_t(bt1)}℃  {secs}s")
    out.append("")

    out.append("# [6] 各 cap 档的电池温度区间与主要包（**判「档位是否咬在跳闸点以下」的第二把尺**）")
    out.append("#     同档秒数在 [4]；此处看「这个档位实际对应的温度带」，与跳闸点/解除点对照。")
    out.append(f"  {'cap':>5s} {'秒':>7s} {'battT min/avg/max':>19s}   主要包（前 3，按秒）")
    for cap_v, n in sorted(cap_secs.items(), key=lambda kv: -kv[1]):
        temps = tier_temp.get(cap_v) or []
        rng = f"{min(temps):.0f} / {sum(temps) / len(temps):.1f} / {max(temps):.0f}" if temps else "-"
        pk = ", ".join(f"{k} {v}" for k, v in tier_pkg[cap_v].most_common(3))
        out.append(f"  {cap_v:>5s} {n:7d} {rng:>19s}   {pk}")
    out.append("")

    out.append("# [7] 按 mode 的能量汇总（gpu_busy / psi_cpu 为**能量加权**均值，仅放电段）")
    out.append("#     判读：gpu 高 → 先查渲染侧；psi_cpu 高 → 先查调度/核数；两者都低 → 前台负载本身轻，")
    out.append("#     能量落在后台/待机（配合 [2] 里 kernelsu/`-` 这类非前台包一起看）。")
    out.append("#     **不做「按 gpu% 拆功率」**：gpu_busy 是占用率不是功率占比，没有模型支撑的拆法就是伪精确。")
    out.append(f"  {'mode':9s} {'Wh':>6s} {'share':>6s} {'n':>7s} {'avg W':>6s} {'gpu%':>5s} {'psi':>5s}")
    by_mode = collections.defaultdict(lambda: collections.defaultdict(list))
    for (mode, _pkg), d in agg.items():
        for k, v in d.items():
            by_mode[mode][k].extend(x for x in v if x is not None)
    for mode in sorted(by_mode, key=lambda m: -sum(by_mode[m]["pw"])):
        d = by_mode[mode]
        e = sum(d["pw"]) / 3600.0
        out.append(f"  {mode:9s} {e:6.2f} {_share(e, total_wh):5.1f}% {len(d['pw']):7d} "
                   f"{sum(d['pw']) / len(d['pw']):6.2f} {dc.avg(d['gpu']):5.1f} {dc.avg(d['psi']):5.1f}")
    out.append("")
    return out, dict(charge=dict(charge), total_wh=total_wh, total_n=total_n,
                     top=sorted(tot, key=lambda k: -sum(tot[k]["pw"]))[:3],
                     tot_wh={k: sum(v["pw"]) / 3600.0 for k, v in tot.items()})


def devimp_report(root, since):
    """devimp 侧 P_avg 交叉验证（**不作为结论**）。

    口径复刻 `scripts/devimp-analyze.py`：只取 `snap` 行、放电方向由 `batt_i` 两侧功率中位数
    枚举（一侧落在 0.05~30 W 即认为该侧为放电）、功率值取 devimp 的 `batt_p` 列，
    且 `batt_i == 0` 的行被排除。**历史口径（仅轻载场景/旧包适用）**：早期机型 `batt_i`
    整数化时，排除 0 等于删掉所有 <0.5 A 的轻载秒（约 58%），均值被抬高；`logd_1001-045147`
    起本机电流已到 0.2~5.5 A，该偏差不再普遍出现，[8] 与 status.csv 已几乎相等。
    """
    files = dc.list_files(root, ("main_",), since)
    out = ["# [8] 交叉验证：devimp 侧 P_avg（`batt_p` 列 / snap 行 / batt_i 符号定放电侧）",
           "#     **不要用作结论**（权威源 = status.csv 的 charge + batt_power_w）",
           "#     历史高估口径仅轻载场景/旧包适用：早期 batt_i 整数化时排除 batt_i==0 会删掉 <0.5A 秒；",
           "#     logd_1001-045147 起电流已到 0.2~5.5A，本表已与 status.csv 几乎相等。"]
    if not files:
        out.append(f"  （无 main_*.log：--since {since} 下无 devimp 侧样本，跳过交叉验证）")
        out.append("")
        return out, None
    schema, recs = None, []
    for fn in files:
        h = dc.parse_head(fn)
        if not h["cols"] or "batt_p" not in h["cols"]:
            continue
        schema = schema or h["schema_tag"]
        I = dc.index_map(h["cols"])
        for p, t in dc.iter_rows(fn):
            if t != "snap":
                continue
            v, bp = dc.num(p[I["batt_i"]]), dc.num(p[I["batt_p"]])
            if v is None or bp is None:
                continue
            recs.append((p[I["mode"]], p[I["package"]] or "nopkg", v, bp))
    if not recs:
        out.append("  （devimp 侧无可用 snap 样本，跳过）")
        out.append("")
        return out, None
    pos = [bp for _m, _p, v, bp in recs if v > 0]
    neg = [bp for _m, _p, v, bp in recs if v < 0]

    def sane(v):
        return bool(v) and 0.05 <= dc.pct(v, 0.5) <= 30.0
    if sane(pos) and not sane(neg):
        disch_positive, note = True, "枚举：batt_i>0 侧为放电"
    elif sane(neg) and not sane(pos):
        disch_positive, note = False, "枚举：batt_i<0 侧为放电"
    else:
        disch_positive, note = True, "[!] 两侧都『合理』或都异常，方向需人工确认；本次按 batt_i>0 侧"
    g = collections.defaultdict(list)
    for m, pkg, v, bp in recs:
        if (v > 0) == disch_positive:
            g[(m, pkg)].append(bp)
    out.append(f"  schema={schema}  文件 {len(files)} 个  放电方向: {note}")
    out.append(f"{'mode':9s} {'package':26s} {'n_devimp':>8s} {'devimp P_avg':>12s} {'p50':>5s} {'p95':>6s}")
    for k in sorted(g, key=lambda k: -len(g[k])):
        v = g[k]
        out.append(f"{k[0]:9s} {k[1][:26]:26s} {len(v):8d} {dc.avg(v):12.2f} "
                   f"{dc.pct(v, .5):5.2f} {dc.pct(v, .95):6.2f}")
    out.append("")
    out.append("#     对照读法：同一 mode×package 行与 [1] 表并排看；两值应接近，devimp 明显偏高时")
    out.append("#     按轻载/整数化口径解读，不要当结论。")
    out.append("")
    return out, dict(devimp={k: dc.avg(v) for k, v in g.items()}, note=note)


def main(argv=None):
    dc.setup_console()
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--since", default="0000-000000")
    ap.add_argument("--out", default=None)
    a = ap.parse_args(argv)

    root = os.path.abspath(a.dir)
    paths = status_files(root, a.since)
    rows = read_status(paths)

    out = ["# dvpower —— 按包功耗归因（status.csv 权威 + devimp 侧交叉验证）",
           f"# dir   : {root}",
           f"# since : {a.since}（status.csv 无日期列 → 按父目录 x_<MMDD-HHMMSS>/ 批次戳筛）",
           "# 口径（判读铁律，正文见 .cursor/commands/devimp-log-analysis.md）:",
           "#   1) 功耗权威源 = charge + batt_power_w；devimp P_avg 只作交叉验证（高估口径仅轻载/旧包适用）。",
           "#   2) 放电方向按 charge 列判，**不要用电流符号**。",
           "#   3) 1 行 ≈ 1 秒 → ΣW/3600 = Wh（n 列 = 样本数≈秒）。",
           "#   4) migrations/wakeups 是 2s 差分，换算每秒 ÷2（本脚本不统计，见 dvmain.py）。",
           "#   5) 按包功耗归因看 [2]；[4] 的档位秒数=热常态/偶发判据；cap 不可跨批次连读。",
           "#   6) 定位「档位咬住多久」看 [5] 迁移时间线 + [6] 档位温度区间；按 mode 的能量汇总看 [7]。",
           "#   7) [8] 是 devimp 侧交叉验证，**不作结论**。",
           ""]
    summary = None
    if paths:
        out.append(f"# 源文件 {len(paths)} 个: " + ", ".join(
            os.path.relpath(p, root).replace("\\", "/") for p in paths))
        out.append(f"# 数据行 {len(rows)}")
        so, summary = status_report(paths, rows)
        # charge 分布提到最前（方向核对）
        out.append("# charge 分布（权威方向信号）: " + str(summary["charge"]))
        out.append("")
        out += so
    else:
        out.append(f"## [!] 无 status.csv（--since {a.since}）—— 排放功耗/能量/热档结论不可得（优雅降级）")
        out.append("")

    do, dv = devimp_report(root, a.since)
    out += do

    workdir = a.out or root
    path = dc.write_report(workdir, "power.txt", out)
    lines = [f"status.csv {len(paths)} 个，行 {len(rows)}"]
    if summary and summary["total_n"]:
        lines.append(f"放电累计 {summary['total_wh']:.1f} Wh，"
                     f"均值 {summary['total_wh'] * 3600.0 / summary['total_n']:.2f} W，"
                     f"样本 {summary['total_n']}")
        for k in summary["top"]:
            w = summary["tot_wh"][k]
            lines.append(f"  {k}: {w:.2f} Wh ({w / summary['total_wh'] * 100:.0f}%)")
    elif summary:
        lines.append("无有效放电样本（charge=discharging 且 batt_power_w>0 的行为 0）"
                     "—— 功耗均值不可得，明细见 power.txt")
    else:
        lines.append("[!] 缺 status.csv：按包功耗结论不可得")
    if dv:
        lines.append(f"devimp 交叉验证 {len(dv['devimp'])} 组（{dv['note']}）")
    dc.announce("dvpower", path, lines)
    return 0


if __name__ == "__main__":
    sys.exit(main())
