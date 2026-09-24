#!/usr/bin/env python3
"""dvaff.py —— `aff_*.log` 预设探针（@A 动作、@S 差分帧、绑定线程轨迹）。

用法:
    python scripts/devimp/dvaff.py <解压目录> [--since MMDD-HHMMSS] [--out 目录]

口径（命令文档「三、判定要点」+ 已知坑 11/13/14）：
- `@A` 按 `(act, reason, result)` 计数；result 归类 ok / e3(ESRCH) / e22(EINVAL) /
  e0(errno 不可得) / other。**`result=e0` 不是成功**（`affinity::io_result_tag` 用
  `raw_os_error().unwrap_or(0)`，拿不到 errno 时写 e0）。
- `<场景>_bulk` 汇总帧**单独统计**：它们才是清理规模的正解；逐条 `bind_release`
  只代表「真有内核动作」的条目（pin/move_group/restore 等）。
- `@S` 帧是**差分集**：前台线程与被管（pinned）条目每帧全量，长尾线程只在
  `u/core/home/pin` 变化时落行，每 30 帧一次全量刷新。**缺失行 = 与上一帧相同**。
  必须跨帧累积重建状态，**不能**把「行数」当线程数。本探测流式累积 tid→(pin,home)，
  不建逐帧对象图（本包 aff_ 单个 ~116MB，占归档 94%）。
- 绑定轨迹：累计 `pin=1 且 home=-1`（组掩码绑定且未钉核）的前台线程数，逐帧输出
  first/last/max + 单调性 + 采样时间戳——用来发现**只绑不解**的泄漏。
- `p` 行不参与差分（每帧全量），但本探针只统计 `t` 行（pin/home 状态在 t 行）。
"""

import argparse
import collections
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dvcommon as dc  # noqa: E402

# t 行：t <pid> <tid> <comm> u=<util> core=<核|-1> home=<核|-1> pin=<0|1> uclamp=<值|-1>
T_LINE = re.compile(r"^t (\d+) (\d+) (\S+) u=(\d+) core=(-?\d+) home=(-?\d+) pin=(\d+) uclamp=(-?\d+)$")
A_LINE = re.compile(r"^@A ts=(\S+) act=(\S+) pid=(-?\d+) tid=(-?\d+) pkg=(\S+) comm=(\S+) "
                    r"dst=(\S+) value=(\S+) result=(\S+) reason=(\S+)$")


def classify(result):
    """result 归类；e0 = errno 不可得，**不是成功**。"""
    if result == "ok":
        return "ok"
    if result == "e3":
        return "e3(ESRCH)"
    if result == "e22":
        return "e22(EINVAL)"
    if result == "e0":
        return "e0(errno 不可得)"
    return "other"


def scan(fn):
    """流式扫一个 aff_ 文件，返回统计字典（不建逐帧对象图）。"""
    acts = collections.Counter()          # (act, reason, result)
    act_ok = collections.Counter()        # reason -> ok
    act_tot = collections.Counter()       # reason -> total
    bulk = collections.Counter()          # reason(_bulk) -> Σvalue
    bulk_frames = collections.Counter()   # reason(_bulk) -> 帧数
    bulk_by_act = collections.Counter()   # act -> 帧数（bulk 帧）
    state = {}                            # tid -> (pin, home, comm)
    frames = 0
    traj = []                             # (frame, ts, bound)
    monotonic = True
    prev_bound = None
    peak = 0
    peak_ts = None
    nfg_hdr = []
    comm_bound = collections.Counter()
    first_bound_frame = None
    with open(fn, encoding="utf-8", errors="replace") as fh:
        for ln in fh:
            if not ln:
                continue
            c = ln[0]
            if c == "@":
                if ln.startswith("@S"):
                    frames += 1
                    m = re.search(r"ts=(\S+) ntop=(\d+) nfg=(\d+)", ln)
                    ts = m.group(1) if m else "?"
                    if m:
                        nfg_hdr.append(int(m.group(3)))
                    bound = 0
                    for pin, home, _cm in state.values():
                        if pin == 1 and home == -1:
                            bound += 1
                    if prev_bound is not None and bound < prev_bound:
                        monotonic = False
                    prev_bound = bound
                    if bound > peak:
                        peak, peak_ts = bound, ts
                    if bound and first_bound_frame is None:
                        first_bound_frame = frames
                    if frames % 200 == 0 or frames <= 3:
                        traj.append((frames, ts, bound))
                else:  # @A
                    m = A_LINE.match(ln.rstrip("\n"))
                    if not m:
                        continue
                    act, reason, result, value = m.group(2), m.group(10), m.group(9), m.group(8)
                    acts[(act, reason, result)] += 1
                    act_tot[reason] += 1
                    if result == "ok":
                        act_ok[reason] += 1
                    if str(reason).endswith("_bulk"):
                        n = dc.num(value)
                        bulk[reason] += int(n) if n is not None else 0
                        bulk_frames[reason] += 1
                        bulk_by_act[act] += 1
            elif c == "t":
                m = T_LINE.match(ln.rstrip("\n"))
                if not m:
                    continue
                tid = int(m.group(2))
                comm = m.group(3)
                pin, home = int(m.group(7)), int(m.group(6))
                state[tid] = (pin, home, comm)
                if pin == 1:
                    comm_bound[comm] += 1
    return dict(acts=acts, act_ok=act_ok, act_tot=act_tot, bulk=bulk, bulk_frames=bulk_frames,
                bulk_by_act=bulk_by_act, state=state, frames=frames, traj=traj,
                monotonic=monotonic, peak=peak, peak_ts=peak_ts, nfg_hdr=nfg_hdr,
                comm_bound=comm_bound, first_bound_frame=first_bound_frame,
                final_bound=prev_bound or 0)


HEADER = """\
# ───────────────────────── 判读校准（务必先读）─────────────────────────
# 1. `@S` 帧是**差分集**：前台线程与被管（pinned）条目每帧全量，长尾线程只在
#    `u/core/home/pin` 变化时落行，每 30 帧一次全量刷新。**缺失行 = 与上一帧相同**。
#    因此「行数 ≠ 线程数」；下表所有状态都是跨帧累积重建的结果。
# 2. `<场景>_bulk` 是**清理规模的正解**（value = 本次条数）；逐条 `bind_release`
#    只代表真有内核动作的条目。
# 3. `result=e0` 不是成功：`io_result_tag` 拿不到 errno 时写 e0。e3=ESRCH（线程已退出，
#    正常）、e22=EINVAL（偶发）。
# 4. 绑定轨迹统计的是累计 `pin=1 且 home=-1` 的线程数（组掩码绑定、未钉核），
#    单调上升到高位不回落 = 只绑不解的疑似泄漏。
# ─────────────────────────────────────────────────────────────────────
"""


def report(fn, st):
    out = [f"## {fn}", HEADER]
    out.append(f"@S 帧数: {st['frames']}   nfg 帧头值: min={min(st['nfg_hdr']) if st['nfg_hdr'] else 0} "
               f"max={max(st['nfg_hdr']) if st['nfg_hdr'] else 0}")
    out.append("")

    out.append("### @A 动作计数 (act, reason, result)  —— result 归类见头部校准")
    out.append(f"  {'act':13s} {'reason':22s} {'result':18s} {'count':>8s}")
    for (act, reason, result), n in st["acts"].most_common():
        out.append(f"  {act:13s} {reason[:22]:22s} {classify(result):18s} {n:8d}")
    out.append("")

    out.append("### @A 按 reason 的成功率（e0 不计入成功）")
    out.append(f"  {'reason':26s} {'total':>8s} {'ok':>8s} {'rate':>7s}")
    for reason, tot in st["act_tot"].most_common():
        ok = st["act_ok"][reason]
        out.append(f"  {reason[:26]:26s} {tot:8d} {ok:8d} {ok / tot * 100:6.1f}%")
    out.append("")

    out.append("### bulk 汇总帧（清理规模正解；单列，不与逐条 bind_release 混算）")
    if st["bulk"]:
        out.append(f"  {'reason':24s} {'Σvalue':>10s} {'帧数':>7s} {'act':>14s}")
        for reason in sorted(st["bulk"], key=lambda r: -st["bulk"][r]):
            acts = ",".join(sorted({a for (a, rr, _), _ in st["acts"].items() if rr == reason}))
            out.append(f"  {reason:24s} {st['bulk'][reason]:10d} {st['bulk_frames'][reason]:7d} {acts:>14s}")
        out.append(f"  {'':>24s} {'合计':>10s} = {sum(st['bulk'].values())}")
    else:
        out.append("  （本文件无 bulk 帧）")
    out.append("")

    out.append("### 绑定线程轨迹：累计 `pin=1 且 home=-1` 的前台线程数（逐帧累积重建）")
    out.append(f"  首帧出现于第 {st['first_bound_frame']} 帧；final={st['final_bound']} "
               f"peak={st['peak']}（@ {st['peak_ts']}）  单调不降={st['monotonic']}")
    if not st["monotonic"]:
        out.append("  [i] 非单调：存在释放（正常亦可能）；重点看 final 是否随会话上行。")
    out.append(f"  {'frame':>6s} {'ts':>14s} {'bound':>7s}")
    for f, ts, b in st["traj"]:
        out.append(f"  {f:6d} {ts:>14s} {b:7d}")
    out.append("")

    out.append("### 末帧 pin / home 直方图（累积状态快照）")
    pin_h = collections.Counter()
    home_h = collections.Counter()
    for _tid, (pin, home, _cm) in st["state"].items():
        pin_h[pin] += 1
        home_h[home] += 1
    out.append(f"  tracked tids = {len(st['state'])}")
    out.append(f"  pin  直方图: {dict(sorted(pin_h.items()))}")
    out.append(f"  home 直方图: {dict(sorted(home_h.items()))}")
    out.append("")
    out.append("### 绑定线程（pin=1）出现次数最多的 comm（累积行数，非并发数）")
    for cm, n in st["comm_bound"].most_common(15):
        out.append(f"  {cm[:40]:40s} {n:8d}")
    out.append("")
    return out


def main(argv=None):
    dc.setup_console()
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--since", default="0000-000000")
    ap.add_argument("--out", default=None)
    a = ap.parse_args(argv)

    root = os.path.abspath(a.dir)
    files = dc.list_files(root, ("aff_",), a.since)
    if not files:
        print(f"[!] {root} 下没有 aff_*.log（--since {a.since}）")
        return 1

    out = ["# dvaff —— aff_*.log 探针", f"# dir: {root}",
           f"# 文件: {len(files)}", ""]
    summary = []
    for fn in files:
        st = scan(fn)
        out += report(fn, st)
        summary.append(f"{os.path.basename(fn)}: @S {st['frames']} 帧, "
                       f"final_bound={st['final_bound']}, peak={st['peak']} (单调={st['monotonic']}), "
                       f"@A 行 {sum(st['acts'].values())}, bulkΣ={sum(st['bulk'].values())}")
    workdir = a.out or root
    path = dc.write_report(workdir, "aff.txt", out)
    dc.announce("dvaff", path, summary)
    return 0


if __name__ == "__main__":
    sys.exit(main())
