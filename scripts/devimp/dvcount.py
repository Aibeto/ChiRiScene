#!/usr/bin/env python3
"""dvcount.py —— 按 SoC 汇总 devimp 批次的推理日志行数（mdocs/socList.md 累计推理量的口径来源）。

用法:
    python scripts/devimp/dvcount.py [root] [--json 输出路径] [--per-batch]

口径（三条，缺一不可；改动即等于改历史数字）:
1. 计入行数 = 批次目录里 main_*.log + aff_*.log 的整文件行数。
   daemon.log / status.csv / inventory.txt 不计（不属于推理输入）。
2. 去重 = 按「main 总字节数 + aff 总字节数」完全相同判为同一次解包的副本，只计一次。
   同一批次常被多个外层包重复解包（例如 x_devimp_1025-195103 在 4 个包里各一份）。
   不要按目录 basename 去重：devimp_X 与 x_devimp_X 是同一批次的两种命名，
   basename 不同会被重复计入（2026-11-18 复算即发现历史累计值混入了这类重复，sm8550 多算 183,641 行）。
3. SoC 归属 = 该批次任一 main_*.log 第 3 行 "# soc=<soc> board=<board> model=<model>"。
   soc=- 时回退 board：pineapple -> sm8650、sun -> sm8750、zumapro -> zumapro；
   两者都没有再看同级 daemon.log 首行的 SoC 字段。4 位纯数字（8550/8650）补 sm 前缀。

注意:
- 只有 aff_*.log 没有 main_*.log 的目录会被跳过并在末尾报数（本仓目前为 0）。
- 行数用字节流数换行符，比 wc -l 少最后一个无换行结尾的残行（10^6 量级可忽略）。
- 基准点（字节去重口径，2026-11-18）：sm8250 116,128 / sm8550 368,450,164 /
  sm8650 47,820,937 / sm8750 16,665,188 / zumapro 27,731,978，合计 460,784,395。
"""

import argparse
import json
import os
import re
from collections import defaultdict

SOC_HEADER = re.compile(r"#\s*soc=(\S+)\s+board=(\S+)\s+model=(\S+)")
SOC_DAEMON = re.compile(r"SoC[\s\u2068]*([A-Za-z0-9_]+)")
BOARD_ALIAS = {"pineapple": "sm8650", "sun": "sm8750", "kalama": "sm8550", "taro": "sm8450"}
BUF = 1 << 20


def count_lines_and_bytes(path):
    lines = 0
    size = 0
    with open(path, "rb") as f:
        while True:
            chunk = f.read(BUF)
            if not chunk:
                break
            lines += chunk.count(b"\n")
            size += len(chunk)
    return lines, size


def header_of(dirpath, filenames):
    for name in sorted(n for n in filenames if n.startswith("main_") and n.endswith(".log")):
        try:
            with open(os.path.join(dirpath, name), "rb") as f:
                for i, raw in enumerate(f):
                    if i > 4:
                        break
                    m = SOC_HEADER.search(raw.decode("utf-8", "replace"))
                    if m:
                        return m.group(1), m.group(2), m.group(3)
        except OSError:
            continue
    return None, None, None


def soc_from_daemon(dirpath):
    ts = os.path.basename(dirpath).split("_")[-1]
    parent = os.path.dirname(dirpath)
    for cand in (os.path.join(parent, "x_" + ts, "daemon.log"), os.path.join(parent, ts, "daemon.log")):
        if os.path.isfile(cand):
            try:
                with open(cand, "rb") as f:
                    m = SOC_DAEMON.search(f.readline().decode("utf-8", "replace"))
            except OSError:
                continue
            if m:
                return m.group(1)
    return None


def normalize(soc, board):
    if soc and soc != "-":
        return "sm" + soc if (soc.isdigit() and len(soc) == 4) else soc
    if board and board != "-":
        return BOARD_ALIAS.get(board, board)
    return None


def scan(root):
    rows = []
    skipped = []
    for dirpath, _dirnames, filenames in os.walk(root):
        mains = [f for f in filenames if f.startswith("main_") and f.endswith(".log")]
        if not mains:
            if any(f.startswith("aff_") and f.endswith(".log") for f in filenames):
                skipped.append(dirpath)
            continue
        main_l = main_b = aff_l = aff_b = 0
        for name in filenames:
            if name.startswith("main_") and name.endswith(".log"):
                l, b = count_lines_and_bytes(os.path.join(dirpath, name))
                main_l += l
                main_b += b
            elif name.startswith("aff_") and name.endswith(".log"):
                l, b = count_lines_and_bytes(os.path.join(dirpath, name))
                aff_l += l
                aff_b += b
        soc, board, model = header_of(dirpath, filenames)
        if normalize(soc, board) is None:
            soc = soc_from_daemon(dirpath)
        rows.append(dict(dir=os.path.relpath(dirpath, root), soc=normalize(soc, board), raw_soc=soc,
                         board=board, model=model, main=main_l, aff=aff_l, total=main_l + aff_l,
                         main_b=main_b, aff_b=aff_b))
    return rows, skipped


def aggregate(rows):
    seen = {}
    kept = []
    dropped = []
    for row in rows:
        key = (row["main_b"], row["aff_b"])
        if key in seen:
            dropped.append((row["dir"], seen[key]))
            continue
        seen[key] = row["dir"]
        kept.append(row)
    agg = defaultdict(lambda: dict(batches=0, main=0, aff=0, total=0))
    for row in kept:
        a = agg[row["soc"] or "UNKNOWN"]
        a["batches"] += 1
        a["main"] += row["main"]
        a["aff"] += row["aff"]
        a["total"] += row["total"]
    return agg, kept, dropped


def main():
    ap = argparse.ArgumentParser(description="按 SoC 汇总 devimp 推理日志行数")
    ap.add_argument("root", nargs="?", default="devimpbin", help="日志根目录（默认 devimpbin）")
    ap.add_argument("--json", help="逐批次明细写到该路径")
    ap.add_argument("--per-batch", action="store_true", help="额外打印逐批次明细")
    args = ap.parse_args()

    rows, skipped = scan(args.root)
    agg, kept, dropped = aggregate(rows)

    total = sum(a["total"] for a in agg.values())
    print("# 批次目录 %d 个，去重后 %d 个（丢弃重复解包 %d 个）" % (len(rows), len(kept), len(dropped)))
    print("%-10s %5s %13s %14s %15s" % ("SoC", "批次", "main", "aff", "合计"))
    for soc, a in sorted(agg.items(), key=lambda kv: -kv[1]["total"]):
        print("%-10s %5d %13s %14s %15s" % (soc, a["batches"], format(a["main"], ","),
                                            format(a["aff"], ","), format(a["total"], ",")))
    print("%-10s %5d %13s %14s %15s" % ("合计", len(kept), "", "", format(total, ",")))

    if args.per_batch:
        print("")
        print("# 逐批次（去重后）")
        for row in sorted(kept, key=lambda r: -r["total"]):
            print("%-10s %12s  %s" % (row["soc"], format(row["total"], ","), row["dir"]))

    if args.json:
        with open(args.json, "w", encoding="utf-8") as f:
            json.dump(dict(aggregate=agg, kept=kept, dropped=dropped), f, ensure_ascii=False, indent=1)
        print("")
        print("明细已写入 " + args.json)

    if skipped:
        print("")
        print("警告: %d 个目录只有 aff_*.log 没有 main_*.log（未计入）: %s" % (len(skipped), skipped[:5]))


if __name__ == "__main__":
    main()
