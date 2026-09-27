#!/usr/bin/env python3
"""dvaff.py —— `aff_*.log` 预设探针（@A 动作、@S 差分帧、绑定线程轨迹）。

用法:
    python scripts/devimp/dvaff.py <解压目录> [--since MMDD-HHMMSS] [--out 目录]
                                   [--groups "0-2,3-6,7"] [--core-top 8]

口径（命令文档「三、判定要点」+ 已知坑 11/13/14）：
- `@A` 按 `(act, reason, result)` 计数；result 归类 ok / e3(ESRCH) / e22(EINVAL) /
  e0(errno 不可得) / other。**`result=e0` 不是成功**（`affinity::io_result_tag` 用
  `raw_os_error().unwrap_or(0)`，拿不到 errno 时写 e0）。
- `<场景>_bulk` 汇总帧**单独统计**：它们才是清理规模的正解；逐条 `bind_release`
  只代表「真有内核动作」的条目（pin/move_group/restore 等）。
- `@S` 帧的 `t` 行是**槽级差分**（2026-09-28 起；旧包是每帧全量六槽行，同一套位置解析）：
  行内只写本次变化的槽，**未变槽写 `-`、尾部连续未变的槽整段省略**，整行全未变则不落行
  →「缺失 tid = 与上次落盘相同」；槽真值为 `-` 时写入端写 `NaN`（只有 comm 取得到），
  本脚本还原为 `-`。**必须逐槽合并，不能把 `-` 当字段值**，也不能把「行数」当线程数。
  帧头带 `full=1` 的是刷新帧（首帧 + 每 30 帧，其 t 行全量），存活集合以它为锚。
  本探测流式累积 tid→槽值，不建逐帧对象图（每个 aff_ 可达 100MB+，是归档的绝对主体）。
- 绑定轨迹：累计 `pin=1 且 home=-1`（组掩码绑定且未钉核）的前台线程数，逐帧输出
  first/last/max + 单调性 + 采样时间戳——用来发现**只绑不解**的泄漏。
- `p` 行不参与差分（每帧全量），但本探针只统计 `t` 行（pin/home 状态在 t 行）。
- 核分布段（`t` 行的 `core` → comm 行数 / `u` 行均值）用来回答「某个簇上活跃的是谁」。
  行数是采样**行数**不是并发线程数（差分集）；`u` 均值里长尾行是 ≤30s 窗口均值。
  簇分界不猜：按机型 DT 用 `--groups` 显式给（8550 little=0-2 / big=3-6 / prime=7）。
"""

import argparse
import collections
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dvcommon as dc  # noqa: E402

# t 行：`t <pid> <tid>[ comm][ u=][ core=][ home=][ pin=][ uclamp=]`，槽序固定（T_SLOTS）。
# 2026-09-28 起为**字段级差分**：只写本次变化的槽，未变槽为 `-`、尾部未变槽整段省略；
# 旧包（全量 t 行）用同一套位置解析即可覆盖——旧格式六槽全有值。
T_PREFIX = re.compile(r"^t (\d+) (\d+)(?: (.*))?$")
T_SLOTS = ("comm", "u", "core", "home", "pin", "uclamp")
T_KEYED = {"u": "u=", "core": "core=", "home": "home=", "pin": "pin=", "uclamp": "uclamp="}
T_NUM = re.compile(r"-?\d+")
# 槽真值为 `-` 时写入端改写这个 token（只有 comm 取得到 `-`），读端还原为 `-`
DASH_VALUE = "NaN"
A_LINE = re.compile(r"^@A ts=(\S+) act=(\S+) pid=(-?\d+) tid=(-?\d+) pkg=(\S+) comm=(\S+) "
                    r"dst=(\S+) value=(\S+) result=(\S+) reason=(\S+)$")


def parse_t_line(line):
    """`t` 行 → (pid, tid, {槽: 值})；槽值为 None = 本次未变（`-` 或尾部省略）。

    位置解析对新旧格式通用：旧格式六槽全有值，新格式未变槽写 `-`、尾部未变槽直接省略。
    真值是 `-` 的槽由写入端写成 `DASH_VALUE`（`NaN`），这里还原成 `-`——不还原就会被
    当成「未变」而把这次变化吞掉。数值槽非整数、或槽位错位的行判为格式不符，返回 None
    由调用方丢行（旧实现靠整行正则兜住这层校验，换成位置解析后必须自己做，否则后面的
    `int()` 会直接抛）。
    """
    m = T_PREFIX.match(line)
    if not m:
        return None
    raw = m.group(3).split() if m.group(3) else []
    vals = {}
    for i, name in enumerate(T_SLOTS):
        if i >= len(raw) or raw[i] == "-":
            vals[name] = None
            continue
        tok = raw[i]
        if name == "comm":
            vals[name] = "-" if tok == DASH_VALUE else tok
            continue
        key = T_KEYED[name]
        if not tok.startswith(key):
            return None
        tok = tok[len(key):]
        if not T_NUM.fullmatch(tok):
            return None
        vals[name] = tok
    return int(m.group(1)), int(m.group(2)), vals


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
    # 计数容器：acts=(act,reason,result) 计数；act_ok/act_tot=reason→ok/总数；
    # bulk/bulk_frames=bulk 帧（reason 以 _bulk 结尾）的 Σvalue/帧数；bulk_by_act=act→帧数（bulk 帧）；
    # state=tid→(pin,home,comm) 已合并态；last=tid→槽级合并态（含 core/u，供末帧直方图）；
    # traj=(frame,ts,bound) 绑定轨迹采样
    acts = collections.Counter()
    act_ok = collections.Counter()
    act_tot = collections.Counter()
    bulk = collections.Counter()
    bulk_frames = collections.Counter()
    bulk_by_act = collections.Counter()
    state = {}
    last = {}                                  # tid → {槽: 值}（槽级差分合并结果）
    tpid = {}                                  # tid → 最近一次的进程 id（tid 复用判定）
    last_seen_full = {}                        # tid → 最近出现过的刷新帧序号（存活集合收敛用）
    full_index = 0                             # 刷新帧（帧头 full=1）序号，从 1 开始
    dropped_t = 0                              # 无法解析的 t 行数（槽名/槽序与写入端不一致时整行丢弃）
    frames = 0
    full_frames = 0                            # 帧头带 full=1 的刷新帧数
    partial_rows = 0                           # 含 `-`/尾部省略的 t 行数（>0 = 新格式）
    traj = []
    monotonic = True
    prev_bound = None
    peak = 0
    peak_ts = None
    nfg_hdr = []
    comm_bound = collections.Counter()
    core_rows = collections.Counter()          # core 号 -> t 行数（core=-1 单列）
    core_comm = collections.Counter()          # (core, comm) -> t 行数
    core_util = collections.Counter()          # (core, comm) -> Σu（除行数得行均值）
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
                    if " full=1" in ln:
                        full_frames += 1
                        # 存活集合收敛（口径见 HEADER）：写入端刷新帧对**所有存活候选**全量落行，
                        # 故「连续两次刷新帧都没出现」= 已退出 → 从 state/last 删除，否则已退出线程
                        # 会被永久计入绑定轨迹与合并态统计（旧包无 full 帧则 full_index=0，不收敛）
                        full_index += 1
                        for t in [x for x, f in last_seen_full.items() if f <= full_index - 3]:
                            last_seen_full.pop(t, None)
                            state.pop(t, None)
                            last.pop(t, None)
                            tpid.pop(t, None)
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
                else:
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
                parsed = parse_t_line(ln.rstrip("\n"))
                if parsed is None:
                    dropped_t += 1
                    continue
                tid = parsed[1]
                pid = parsed[0]
                # pid 变化 = tid 被其它进程复用：写入端只为该情形落一条零槽标识行，本行只作归属更新，
                # 必须作废该 tid 的合并态（否则新线程沿用旧进程的 comm/core/pin，统计张冠李戴）
                if tpid.get(tid, pid) != pid:
                    tpid[tid] = pid
                    last.pop(tid, None)
                    state.pop(tid, None)
                    last_seen_full[tid] = full_index
                    continue
                tpid[tid] = pid
                last_seen_full[tid] = full_index
                # 槽级合并：本次有值的槽覆盖旧值；None（`-` 或尾部省略）= 沿用上次值。
                # 不做合并就取槽值会把 `-` 当字段值，正是新格式最容易读错的地方。
                merged = last.setdefault(tid, {})
                dash = 0
                for k, v in parsed[2].items():
                    if v is None:
                        dash += 1
                        continue
                    merged[k] = v
                if dash:
                    partial_rows += 1
                # 身份齐备（pin/home/comm 都见过）才进 state：与旧口径一致，截断首帧的残缺行不入
                if all(k in merged for k in ("pin", "home", "comm")):
                    state[tid] = (int(merged["pin"]), int(merged["home"]), merged["comm"])
                if int(merged.get("pin", 0)) == 1 and "comm" in merged:
                    comm_bound[merged["comm"]] += 1
                # 核分布按**合并后**的 core/comm/u 计数：行数 = 该线程本次有变化的采样行
                if "core" in merged and "comm" in merged:
                    core = int(merged["core"])
                    comm = merged["comm"]
                    core_rows[core] += 1
                    core_comm[(core, comm)] += 1
                    core_util[(core, comm)] += int(merged.get("u", 0))
    return dict(acts=acts, act_ok=act_ok, act_tot=act_tot, bulk=bulk, bulk_frames=bulk_frames,
                bulk_by_act=bulk_by_act, state=state, last=last, frames=frames,
                full_frames=full_frames, partial_rows=partial_rows, dropped_t=dropped_t, traj=traj,
                monotonic=monotonic, peak=peak, peak_ts=peak_ts, nfg_hdr=nfg_hdr,
                comm_bound=comm_bound, first_bound_frame=first_bound_frame,
                final_bound=prev_bound or 0,
                core_rows=core_rows, core_comm=core_comm, core_util=core_util)


HEADER = """\
# ───────────────────────── 判读校准（务必先读）─────────────────────────
# 1. `@S` 帧是**槽级差分**（2026-09-28 起）：`t` 行只写本次变化的槽，未变槽写 `-`、
#    尾部未变槽整段省略（**缺省槽 = 沿用上次值，不能把 `-` 当字段值**）；整行全未变
#    则不落行。槽真值本身就是 `-` 时写入端写 `NaN`（只有 comm 取得到），本脚本还原成
#    `-`——不还原就会被当成「未变」而吞掉这次变化。因此「行数 ≠ 线程数」，下表所有
#    状态都是跨帧**逐槽合并**重建的结果。存活集合以帧头带 `full=1` 的刷新帧为锚
#    （首帧 + 每 30 帧一次，其 t 行是全量）；连续两次刷新帧都没出现的 tid 才算已退出
#    （粒度 = 刷新间隔）。旧包（2026-09-27 及以前）是每帧全量六槽行，同一套位置解析，
#    本脚本自动兼容。
# 2. `<场景>_bulk` 是**清理规模的正解**（value = 本次条数）；逐条 `bind_release`
#    只代表真有内核动作的条目。
# 3. `result=e0` 不是成功：`io_result_tag` 拿不到 errno 时写 e0。e3=ESRCH（线程已退出，
#    正常）、e22=EINVAL（偶发）。
# 4. 绑定轨迹统计的是存活集合里 `pin=1 且 home=-1` 的线程数（组掩码绑定、未钉核），
#    单调上升到高位不回落 = 只绑不解的疑似泄漏。
# ─────────────────────────────────────────────────────────────────────
"""


def report(fn, st, groups=None, top=8):
    out = [f"## {fn}", HEADER]
    out.append(f"@S 帧数: {st['frames']}   nfg 帧头值: min={min(st['nfg_hdr']) if st['nfg_hdr'] else 0} "
               f"max={max(st['nfg_hdr']) if st['nfg_hdr'] else 0}")
    # 新旧判别：`full=1` 帧头是新格式独有的，故即便一行 `-` 都没出现（极端情况下每行都恰好是全量行）
    # 也要按新格式报——否则会把新包说成旧包，读者按「每行全量」去解释会漏掉整个合并语义
    fmt = ("槽级差分（2026-09-28 起）"
           if (st["partial_rows"] or st["full_frames"]) else "全量 t 行（旧格式）")
    out.append(f"t 行格式: {fmt}   含 `-` 或尾部省略的行 {st['partial_rows']}   "
               f"刷新帧(full=1) {st['full_frames']}   合并态 tid 数 {len(st['last'])}   "
               f"无法解析丢弃 {st['dropped_t']}")
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

    out.append("### 绑定线程轨迹：存活集合里 `pin=1 且 home=-1` 的前台线程数（逐帧重建）")
    out.append(f"  首次出现于第 {st['first_bound_frame'] or '-'} 帧；final={st['final_bound']} "
               f"peak={st['peak']}（@ {st['peak_ts']}）  单调不降={st['monotonic']}")
    if not st["monotonic"]:
        out.append("  [i] 非单调：存在释放（正常亦可能）；重点看 final 是否随会话上行。")
    out.append(f"  {'frame':>6s} {'ts':>14s} {'bound':>7s}")
    for f, ts, b in st["traj"]:
        out.append(f"  {f:6d} {ts:>14s} {b:7d}")
    out.append("")

    out.append("### pin / home 直方图（当前存活集合的合并态）")
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

    out.append("### t 行核分布（core → 出现最多的 comm）")
    out.append("  行数 = 该核上的采样**行数**（新格式下 = 本次有变化的行 + 每 30 帧的刷新行，")
    out.append("  不是并发线程数，也**不可跨新旧格式直接比**）；u_avg 是这些行的 `u` 均值——")
    out.append("  长尾行的 u 是 ≤30s 窗口均值，短促占用会被抹平。想要的「某一刻谁在哪核」看下一节。")
    out.append("  簇归属按机型 DT 定，勿照搬：8550 little=0-2 / big=3-6 / prime=7（8475 分界不同）；")
    out.append("  `--groups \"0-2,3-6,7\"` 可让脚本按当前机型分簇汇总。")
    out.append(f"  逐核 t 行数: {dict(sorted(st['core_rows'].items()))}")
    for core in sorted(st["core_rows"]):
        keys = sorted((k for k in st["core_comm"] if k[0] == core),
                      key=lambda k: -st["core_comm"][k])
        out.append(f"  core={core:<3d} t 行 {st['core_rows'][core]:8d}")
        for k in keys[:top]:
            n = st["core_comm"][k]
            out.append(f"      {k[1][:36]:36s} {n:8d}  u_avg={st['core_util'][k] / n:6.1f}")
    for gi, grp in enumerate(groups or [], 1):
        com, util = collections.Counter(), collections.Counter()
        for k in st["core_comm"]:
            if k[0] in grp:
                com[k[1]] += st["core_comm"][k]
                util[k[1]] += st["core_util"][k]
        out.append(f"  group{gi} cores={sorted(grp)}  t 行 {sum(com.values())}")
        for cm, n in com.most_common(top):
            out.append(f"      {cm[:36]:36s} {n:8d}  u_avg={util[cm] / n:6.1f}")
    out.append("")

    # 槽级差分下「行数」不再等于「驻留时长」，用合并态补一份「谁在哪核」——
    # 只反映存活集合的**最后已知**位置（已退出 tid 在两次刷新帧缺席后移出），
    # 不是并发峰值，也不能当驻留时长用
    out.append("### 存活集合合并态：core → 唯一 tid 数（逐槽合并结果，不受差分省略影响）")
    end_core = collections.Counter()
    end_core_comm = collections.Counter()
    for cm in st["last"].values():
        if "core" not in cm:
            continue
        core = int(cm["core"])
        end_core[core] += 1
        if "comm" in cm:
            end_core_comm[(core, cm["comm"])] += 1
    out.append(f"  逐核唯一 tid 数: {dict(sorted(end_core.items()))}")
    for core in sorted(end_core):
        keys = sorted((k for k in end_core_comm if k[0] == core),
                      key=lambda k: -end_core_comm[k])
        out.append(f"  core={core:<3d} tid {end_core[core]:6d}")
        for k in keys[:top]:
            out.append(f"      {k[1][:36]:36s} {end_core_comm[k]:6d}")
    out.append("")
    return out


def positive_int(s):
    """`--core-top` 取值校验：1..64（<=0 会让切片语义反转/失去意义）"""
    v = int(s)
    if v < 1 or v > 64:
        raise argparse.ArgumentTypeError(f"需在 1..64，收到 {s}")
    return v


def parse_groups(s):
    """`"0-2,3-6,7"` → [{0,1,2},{3,4,5,6},{7}]（簇分界按机型 DT 给，脚本不猜拓扑）。

    非法输入（非数字 / 端点倒置）抛 ValueError，由调用方转 argparse 错误：静默产出空簇
    会让报告显示成「该簇没数据」，与真实的解析失败无法区分。
    """
    out = []
    for part in (s or "").split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            lo, hi = part.split("-", 1)
            if not (lo.isdigit() and hi.isdigit()):
                raise ValueError(f"非法的簇区间 '{part}'（应为 a-b）")
            a, b = int(lo), int(hi)
            if a > b:
                raise ValueError(f"簇区间端点倒置 '{part}'")
            out.append(set(range(a, b + 1)))
        else:
            if not part.isdigit():
                raise ValueError(f"非法的核号 '{part}'")
            out.append({int(part)})
    return out


def main(argv=None):
    dc.setup_console()
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--since", default="0000-000000")
    ap.add_argument("--out", default=None)
    ap.add_argument("--groups", default=None, help='簇分界，如 "0-2,3-6,7"（8550 用这个）')
    ap.add_argument("--core-top", type=positive_int, default=8,
                    help="每核/每簇列出的 comm 条数（1..64）")
    a = ap.parse_args(argv)
    try:
        groups = parse_groups(a.groups)
    except ValueError as e:
        ap.error(str(e))

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
        out += report(fn, st, groups, a.core_top)
        hot = sorted(st["core_rows"].items(), key=lambda x: -x[1])[:3]
        summary.append(f"{os.path.basename(fn)}: @S {st['frames']} 帧, "
                       f"final_bound={st['final_bound']}, peak={st['peak']} (单调={st['monotonic']}), "
                       f"@A 行 {sum(st['acts'].values())}, bulkΣ={sum(st['bulk'].values())}, "
                       f"丢弃 t 行 {st['dropped_t']}, core 行数 top3 {hot}")
    workdir = a.out or root
    path = dc.write_report(workdir, "aff.txt", out)
    dc.announce("dvaff", path, summary)
    return 0


if __name__ == "__main__":
    sys.exit(main())
