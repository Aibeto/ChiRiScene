#!/usr/bin/env python3
"""dvmain.py —— `main_*.log` 预设探针（定版 / mode×package / 决策 / 热 / snap）。

用法:
    python scripts/devimp/dvmain.py <解压目录> [--since MMDD-HHMMSS] [--min-n 30] [--out 目录]

覆盖命令文档「三、判定要点」里最容易读错的数字，逐条写进输出文件：
- 定版：module / soc / board / model / android / kernel + schema；**混版本/混机型即报警**
- 每文件行类型分布与时间跨度；批次（父目录 = 一次导出簇）边界
- mode × package：样本数、时间跨度、热档（batt/cpu temp）
- **decide vs actual**：`cur_freq_khz`/`max_freq_khz` 是调度器写 scaling_max 的**决策值**，
  不是实际频率；比值 = cap%（实际频率看 snap 的 `cpu_cur_khz`）
- `decision` / `reason` 分布；`deb_up`/`deb_down` 的**最大连续值**（升/降频速率限制
  streak 计数，不是错误计数——输出文件头已注明）
- 热：`thermal_change` 事件序列（`batt=/cpu=/cap=/free=`）与**真正落在压制带
  `(soft_perf_cap, free_above)` 内的 tick 占比**（`cap=85` 不等于已压制；
  `free_above` 是性能豁免档，不是温度带；tuned 段 cap 列为 `-`）
- snap 侧：每 policy 的 `cpu_cur_khz` vs `cpu_max_khz`、`cpu_governor`，
  `wakeups`/`migrations` **÷2**（2s 增量）；附迁移率量级带供对照
- 不假设存在 `tgtop` 行（44 列拆分版没有）

口径一律以 `.cursor/commands/devimp-log-analysis.md` 为准；本脚本只做读数与统计。
"""

import argparse
import collections
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dvcommon as dc  # noqa: E402

# 迁移率量级带（命令文档「三、判定要点」2026-09-23 实测 Canary Alpha06-04 / 8550）
MIG_BANDS = [
    ("bili 播放态", 4500, 7000),
    ("亮屏 UI（launcher/kernelsu）", 3000, 6000),
    ("MOBA（王者）FAS", 9000, 16000),
]


def build(files, min_n):
    out = []
    heads = [dc.parse_head(f) for f in files]
    fp = dc.fingerprint(heads)

    # ── 定版 ──
    out.append("# [1] 定版（文件头元信息）")
    for k in ("module", "soc", "board", "model", "android", "kernel", "schema_tag"):
        vs = sorted(fp.get(k, []))
        flag = "  [!] 多值" if len(vs) > 1 else ""
        out.append(f"  {k:11s}: {', '.join(vs) if vs else '(缺)'}{flag}")
    mixed = [k for k, v in fp.items() if len(v) > 1]
    if mixed:
        out.append(f"  [!] 同一包内出现多值 {mixed} —— 跨值对比无效（功耗绝对值不可比）")
    else:
        out.append("  [ok] 单一版本/机型，可跨批次对比")
    ts_cols = sorted({h["ts_column"] for h in heads if h.get("ts_column")})
    out.append(f"  ts-column : {', '.join(ts_cols) if ts_cols else '(缺)'}")
    out.append("")

    # ── 行类型分布 / 时间跨度 / 批次 ──
    out.append("# [2] 每文件行类型分布与时间跨度（批次 = 父目录）")
    batches = dc.batches(files)
    for bdir in sorted(batches):
        out.append(f"## 批次 {os.path.basename(bdir)}  ({len(batches[bdir])} 个文件)")
        for fn in sorted(batches[bdir]):
            types = collections.Counter()
            lo = hi = None
            n = 0
            for p, t in dc.iter_rows(fn):
                n += 1
                types[t] += 1
                if lo is None or p[0] < lo:
                    lo = p[0]
                if hi is None or p[0] > hi:
                    hi = p[0]
            out.append(f"  {os.path.basename(fn):52s} rows={n:6d} {lo}~{hi}  {dict(types)}")
    out.append("")
    return out, fp, heads


def mode_package(files, cols, min_n):
    I = dc.index_map(cols)
    out = []
    need = ("mode", "package", "cur_perf", "thermal_cap_pct", "decision", "reason",
            "deb_up", "deb_down", "cluster", "cur_freq_khz", "max_freq_khz",
            "batt_temp", "cpu_temp")
    if any(k not in I for k in need):
        out.append("# [3] 决策轨迹：本 schema 缺列，跳过")
        return out

    # 按 (batch, mode, package) 聚合；tick 行按 cluster 细分
    agg = collections.defaultdict(lambda: collections.defaultdict(list))
    kinds = collections.defaultdict(collections.Counter)
    # deb streak：按 (batch,mode,package,cluster) 时间序列取最大值
    streak = collections.defaultdict(lambda: collections.defaultdict(int))
    decisions = collections.defaultdict(collections.Counter)
    reasons = collections.defaultdict(collections.Counter)
    # 热压制带内的 tick 计数
    band = collections.defaultdict(lambda: collections.Counter())
    free_above_by_batch = collections.defaultdict(set)

    for fn in files:
        batch = os.path.basename(os.path.dirname(fn))
        for p, t in dc.iter_rows(fn):
            key = (batch, p[I["mode"]], p[I["package"]] or "nopkg")
            kinds[key][t] += 1
            tsv = p[0]
            if t == "tick":
                cl = p[I["cluster"]]
                if cl not in ("little", "big", "prime"):
                    continue
                d = agg[key]
                d["ts"].append(tsv)
                d["perf"].append(dc.num(p[I["cur_perf"]]))
                d["cap"].append(dc.num(p[I["thermal_cap_pct"]]))
                c, mx = dc.num(p[I["cur_freq_khz"]]), dc.num(p[I["max_freq_khz"]])
                if c and mx:
                    agg[key]["rat_" + cl].append(c / mx * 100)
                decisions[key][p[I["decision"]]] += 1
                reasons[key][p[I["reason"]] or "-"] += 1
                for col, ctr in (("deb_up", "up"), ("deb_down", "down")):
                    v = dc.num(p[I[col]])
                    if v is not None and v > streak[(key, cl)][ctr]:
                        streak[(key, cl)][ctr] = int(v)
                # 压制带判定需要 free_above（本行没有）→ 先收集，见下方第二遍
                d["_bandrows"].append((tsv, dc.num(p[I["cur_perf"]]), dc.num(p[I["thermal_cap_pct"]])))
            elif t == "snap":
                agg[key]["snap_ts"].append(tsv)
                agg[key]["bt"].append(dc.num(p[I["batt_temp"]]))
                agg[key]["ct"].append(dc.num(p[I["cpu_temp"]]))
            elif t == "event":
                if p[I["decision"]] == "thermal_change":
                    free_above_by_batch[batch].add(p[I["reason"]])

    # 热压制带：free_above 只出现在 thermal_change 事件（reason 形如
    # `batt=41.0 cpu=56.4 cap=85 free=95`）里；取该批次解析出的值。
    def fa_of(batch):
        vals = free_above_by_batch.get(batch) or set()
        parsed = []
        for v in vals:
            for tok in v.replace(";", " ").split():
                if tok.startswith("free="):
                    parsed.append(dc.num(tok[5:]))
        parsed = [x for x in parsed if x is not None]
        return (max(parsed) / 100.0) if parsed else None

    for key, d in agg.items():
        batch = key[0]
        fa = fa_of(batch)
        ctr = collections.Counter()
        for _ts, perf, cap in d.get("_bandrows", []):
            if perf is None or cap is None:
                continue
            if cap >= 100:
                ctr["cap_inactive"] += 1
                continue
            ctr["cap_active"] += 1
            if fa is not None and cap / 100.0 < perf < fa:
                ctr["in_band"] += 1
            elif fa is not None and perf >= fa:
                ctr["exempt(>=free_above)"] += 1
            elif perf <= cap / 100.0:
                ctr["below_cap(min 无操作)"] += 1
        band[key] = ctr

    out.append("# [3] mode × package（tick = 决策轨迹；snap = 1s 环境）")
    out.append("#     decide vs actual：cur_freq/max_freq 是**写入 scaling_max 的决策值**，")
    out.append("#     比值 = cap%；实际频率看 snap 的 cpu_cur_khz（见 [6]）。")
    out.append("#     deb_up/deb_down 是 up_wait/down_wait 的**连续方向 streak 计数**，")
    out.append("#     不是错误计数；下表报「最大连续值」。")
    out.append("#     `-` = 本行该列无样本（如 FAS/tuned 段无 tick → 无决策值、无 cap）。")
    hdr = (f"{'batch':22s} {'mode':9s} {'package':24s} {'n_snap':>6s} {'n_tick':>6s} {'span':23s} "
           f"{'battT':>5s} {'cpuT':>5s} {'capAvg':>6s} | "
           f"{'l_cap%':>6s} {'b_cap%':>6s} {'p_cap%':>6s} | {'debUp':>5s} {'debDn':>5s} | "
           f"{'cap<100':>7s} {'in_band':>7s} {'ratio':>6s}")
    out.append(hdr)
    for key in sorted(agg):
        d = agg[key]
        ns = len(d["snap_ts"])
        nt = len(d["ts"])
        if ns + nt < min_n:
            continue
        allts = sorted(d["snap_ts"] + d["ts"])
        span = f"{allts[0]}~{allts[-1]}" if allts else "-"
        caps = [x for x in d["cap"] if x is not None]
        def rc(cl):
            v = d.get("rat_" + cl, [])
            return f"{dc.avg(v):6.1f}" if v else "     -"
        ub = db = 0
        for cl in ("little", "big", "prime"):
            ub = max(ub, streak[(key, cl)]["up"])
            db = max(db, streak[(key, cl)]["down"])
        b = band.get(key) or collections.Counter()
        ca = b.get("cap_active", 0)
        ratio = f"{b.get('in_band', 0) / ca * 100:5.1f}%" if ca else "     -"
        bt = f"{dc.avg(d['bt']):5.1f}" if d["bt"] else "    -"
        ct = f"{dc.avg(d['ct']):5.1f}" if d["ct"] else "    -"
        capavg = f"{dc.avg(caps):6.0f}" if caps else "     -"
        out.append(f"{key[0][:22]:22s} {key[1]:9s} {key[2][:24]:24s} {ns:6d} {nt:6d} {span:23s} "
                   f"{bt} {ct} {capavg} | "
                   f"{rc('little')} {rc('big')} {rc('prime')} | {ub:5d} {db:5d} | "
                   f"{ca:7d} {b.get('in_band', 0):7d} {ratio}")
    out.append("")

    # decision / reason 分布（取最大的几个 mode×package）
    top = sorted(agg, key=lambda k: -(len(agg[k]["ts"]) + len(agg[k]["snap_ts"])))[:8]
    out.append("# [4] decision / reason 分布（tick 行，按 mode×package）")
    for key in top:
        ds = ", ".join(f"{k}:{v}" for k, v in decisions[key].most_common(6))
        out.append(f"  {key[1]:9s} {key[2][:22]:22s} decision = {ds}")
        rs = ", ".join(f"{k}:{v}" for k, v in reasons[key].most_common(6))
        out.append(f"  {'':9s} {'':22s} reason   = {rs}")
    out.append("")
    return out


def thermal(files, cols):
    I = dc.index_map(cols)
    out = ["# [5] 热：thermal_change 事件与压制带 `(soft_perf_cap, free_above)` 内 tick 占比",
           "#     `cap=85` ≠ 已压制：压制只落在 (cap, free_above) 区间，`>= free_above` 不钳制；",
           "#     tuned 段（playback/akmode 等）cap 列恒为 `-`（FAS/tuned 接管时不走 CLG 限幅）。"]
    events = collections.defaultdict(list)
    for fn in files:
        for p, t in dc.iter_rows(fn):
            if t == "event" and p[I["decision"]] == "thermal_change":
                events[os.path.basename(os.path.dirname(fn))].append(
                    (p[I["mode"]], p[I["package"]] or "-", p[I["reason"]] or "-"))
    if not events:
        out.append("  （本包无 thermal_change 事件：未触发热压制或包太短）")
    for b in sorted(events):
        out.append(f"## 批次 {b}")
        for mode, pkg, reason in events[b]:
            out.append(f"  {mode:9s} {pkg:22s} {reason}")
    out.append("")
    return out


def snap_side(files, cols):
    I = dc.index_map(cols)
    out = ["# [6] snap 侧：每 policy 的 cur/max kHz、governor；wakeups/migrations ÷2（2s 增量）",
           "#     迁移率量级带（2026-09-23 实测 8550，仅作对照）："]
    for name, lo, hi in MIG_BANDS:
        out.append(f"#       {name}: {lo}~{hi}/s")
    need = ("cpu_cur_khz", "cpu_max_khz", "cpu_governor")
    if any(k not in I for k in need):
        out.append("  （本 schema 无 cpu_cur_khz/cpu_governor 列：40 列旧版，跳过 snap 侧）")
        out.append("")
        return out
    pol = collections.defaultdict(lambda: {"cur": [], "max": [], "gov": collections.Counter()})
    mig = collections.defaultdict(list)
    wk = collections.defaultdict(list)
    for fn in files:
        for p, t in dc.iter_rows(fn):
            if t != "snap":
                continue
            key = (p[I["mode"]],)
            cs = dc.parse_policies(p[I["cpu_cur_khz"]])
            ms = dc.parse_policies(p[I["cpu_max_khz"]])
            for name, v in cs.items():
                pol[(key, name)]["cur"].append(v)
            for name, v in ms.items():
                pol[(key, name)]["max"].append(v)
            # governor 是字符串值（schedutil 等），不能走 parse_policies 的数值解析
            for part in (p[I["cpu_governor"]] or "").split(";"):
                if ":" in part:
                    gn, _, gv = part.partition(":")
                    pol[(key, gn.strip())]["gov"][gv.strip()] += 1
            m = dc.num(p[I["migrations"]])
            w = dc.num(p[I["wakeups"]])
            if m is not None:
                mig[key].append(m / 2.0)
            if w is not None:
                wk[key].append(w / 2.0)
    out.append(f"  {'mode':9s} {'policy':8s} {'n':>5s} {'cur_avg':>9s} {'max_avg':>9s} {'cur/max%':>8s}  governor")
    for key in sorted(pol):
        d = pol[key]
        ca, ma = dc.avg(d["cur"]), dc.avg(d["max"])
        ratio = f"{ca / ma * 100:8.1f}" if ma else "       -"
        gov = ", ".join(f"{k}:{v}" for k, v in d["gov"].most_common(3))
        out.append(f"  {key[0][0]:9s} {key[1]:8s} {len(d['cur']):5d} {ca:9.0f} {ma:9.0f} {ratio}  {gov}")
    out.append("")
    out.append("#      wakeups/migrations（÷2 后 = 每秒）")
    out.append(f"  {'mode':9s} {'n':>5s} {'wakeups/s':>10s} {'migrations/s':>13s}")
    for key in sorted(mig):
        out.append(f"  {key[0]:9s} {len(mig[key]):5d} {dc.avg(wk.get(key, [])):10.0f} "
                   f"{dc.avg(mig[key]):13.0f}")
    out.append("")
    return out


def main(argv=None):
    dc.setup_console()
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--since", default="0000-000000")
    ap.add_argument("--min-n", type=int, default=30)
    ap.add_argument("--out", default=None)
    a = ap.parse_args(argv)

    root = os.path.abspath(a.dir)
    files = dc.list_files(root, ("main_",), a.since)
    if not files:
        print(f"[!] {root} 下没有 main_*.log（--since {a.since}）")
        return 1
    cols = None
    for f in files:
        h = dc.parse_head(f)
        if h["cols"]:
            cols = h["cols"]
            break
    if cols is None:
        print("[!] 读不到表头，无法判定 schema")
        return 1

    out, fp, heads = build(files, a.min_n)
    out += mode_package(files, cols, a.min_n)
    out += thermal(files, cols)
    out += snap_side(files, cols)

    workdir = a.out or root
    path = dc.write_report(workdir, "main.txt", out)
    ver = ", ".join(sorted(fp.get("module", []))) or "(无元信息)"
    dev = ", ".join(sorted(fp.get("soc", []))) + " / " + ", ".join(sorted(fp.get("model", [])))
    mixed = [k for k, v in fp.items() if len(v) > 1]
    dc.announce("dvmain", path, [
        f"定版: {ver} | {dev} | {sorted(fp.get('schema_tag', [])) or '?'}",
        f"文件 {len(files)} 个；批次 {len(dc.batches(files))} 个" + ("  [!] 混版本/混机型" if mixed else ""),
    ])
    return 0


if __name__ == "__main__":
    sys.exit(main())
