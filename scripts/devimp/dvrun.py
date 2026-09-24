#!/usr/bin/env python3
"""dvrun.py —— devimp 日志包分析的单命令驱动（一次 python 进程跑完整条链）。

用法:
    python scripts/devimp/dvrun.py <logd_*.tar.gz | 已解压目录>
        [--tag T] [--since MMDD-HHMMSS] [--min-n 30] [--only extract,analyze,main,aff,status]

或者用一条命令入口（Windows）：
    scripts\\devimp-run.cmd devimpbin\\logd_0925-045336.tar.gz

产出（写进 `devimpbin/<tag>/`）：
    inventory.txt  解压清单（每个文件 + 大小 + 嵌套层 + 检出压缩）
    analyze.txt    `scripts/devimp-analyze.py` 的输出（它文件名带连字符，不能 import，
                   用 subprocess 捕获 stdout —— 其 stdout 是纯 ASCII 数字表，无中文乱码风险）
    main.txt       dvmain 探针
    aff.txt       dvaff 探针
    status.txt    dvstatus 探针
    report.md    步骤清单 + 各探针头条数字 + 单独重跑任一阶段的完整命令

失败隔离：任一阶段失败不中断整条链（依赖允许时继续），`report.md` 里明确标注失败阶段。
stdout 只打短摘要（输出目录 / 一行定版 / 头条数字），绝不回放探针全文。
"""

import argparse
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import dvcommon as dc  # noqa: E402

STAGES = ("extract", "analyze", "main", "aff", "status")
PY = sys.executable or "python"


def run_stage(name, func):
    """跑一个阶段，返回 (ok, 摘要行 list, 报错文本 or None)。"""
    t0 = time.time()
    try:
        rc, lines = func()
        ok = rc == 0
        return ok, lines, None if ok else f"退出码 {rc}", time.time() - t0
    except Exception as e:  # 单阶段失败不拖垮整条链
        return False, [], f"{type(e).__name__}: {e}", time.time() - t0


def main(argv=None):
    dc.setup_console()
    ap = argparse.ArgumentParser()
    ap.add_argument("input", help="logd_*.tar.gz 或已解压目录")
    ap.add_argument("--tag", default=None)
    ap.add_argument("--since", default="0000-000000")
    ap.add_argument("--min-n", type=int, default=30)
    ap.add_argument("--only", default=",".join(STAGES),
                    help="逗号分隔的阶段子集（默认全部）")
    a = ap.parse_args(argv)

    want = [s.strip() for s in a.only.split(",") if s.strip()]
    for s in want:
        if s not in STAGES:
            print(f"[!] 未知阶段 {s}（可选 {','.join(STAGES)}）")
            return 2

    report = ["# devimp 分析报告", "",
              f"- 输入: `{os.path.abspath(a.input)}`",
              f"- 生成: {time.strftime('%Y-%m-%d %H:%M:%S')}",
              f"- 阶段: {', '.join(want)}",
              f"- 参数: --since {a.since} --min-n {a.min_n}", ""]
    headline = []
    failed = []

    # ── 1. extract（后续所有阶段都依赖它）──
    outdir = None
    if "extract" in want:
        import dvextract
        recs = []
        def _extract():
            nonlocal outdir
            outdir, was_dir = dvextract.run(a.input, a.tag, "devimpbin", recs)
            warns, nd, nl = dvextract.verify(outdir)
            inv = ["# dvextract inventory",
                   f"# input   : {os.path.abspath(a.input)}",
                   f"# outdir  : {outdir}", f"# files   : {len(recs)}", "#",
                   f"{'size':>12s}  {'compression':<9s} {'layer':<38s} path"]
            for rel, size, layer, comp in sorted(recs, key=lambda r: (r[2], r[0])):
                inv.append(f"{size:12d}  {comp:<9s} {layer:<38s} {rel}")
            inv.append("#")
            inv.append(f"# devimp 侧批次: {nd}   logd 侧批次: {nl}")
            inv.extend(warns)
            dc.write_report(outdir, "inventory.txt", inv)
            return 0, [f"输出目录 {outdir}", f"文件 {len(recs)}，devimp {nd} / logd {nl}"]
        ok, lines, err, dt = run_stage("extract", _extract)
        report.append(f"## 1. extract {'✅' if ok else '❌ 失败'}")
        report += [f"- {l}" for l in lines]
        if err:
            report.append(f"- **失败**: {err}")
            failed.append(("extract", err))
        report.append("")
        headline += lines[:1]
    else:
        outdir = os.path.abspath(a.input) if os.path.isdir(a.input) else None
        if outdir is None:
            print("[!] 跳过 extract 时必须传入已解压目录")
            return 2

    if outdir is None:
        print("[!] extract 失败且无法定位输出目录，后续阶段中止")
        return 1

    # ── 2. analyze：文件名带连字符，只能走 subprocess（其 stdout 纯 ASCII 表）。
    #    该脚本只 glob 传入目录**本级**（非递归），而解压结果是
    #    `x_devimp_<ts>/` 子目录，故按 devimp 批次逐个跑，输出合并进 analyze.txt ──
    if "analyze" in want:
        def _analyze():
            script = os.path.join(dc.repo_root(), "scripts", "devimp-analyze.py")
            import glob as _glob
            batch_dirs = sorted({os.path.dirname(p) for p in
                                 _glob.glob(os.path.join(outdir, "**", "main_*.log"), recursive=True)})
            if not batch_dirs:
                batch_dirs = [outdir]
            chunks, heads, rc = [], [], 0
            for bd in batch_dirs:
                cmd = [PY, script, bd, "--since", a.since, "--min-n", str(a.min_n)]
                pr = subprocess.run(cmd, capture_output=True, text=True,
                                    encoding="utf-8", errors="replace")
                chunks.append(f"# ── batch {os.path.basename(bd)} ──")
                chunks.append(f"# cmd: {' '.join(cmd)}")
                chunks.append(pr.stdout or "")
                if pr.stderr:
                    chunks.append("STDERR:\n" + pr.stderr)
                chunks.append("")
                if pr.returncode != 0:
                    rc = pr.returncode
                heads += [l for l in (pr.stdout or "").splitlines()
                          if l.strip() and not l.startswith(" ")]
            dc.write_report(outdir, "analyze.txt",
                            ["# 由 scripts/devimp-analyze.py 生成（subprocess 捕获，按批次）", ""] + chunks)
            return rc, [f"批次目录 {len(batch_dirs)} 个"] + heads[:6]
        ok, lines, err, dt = run_stage("analyze", _analyze)
        report.append(f"## 2. analyze {'✅' if ok else '❌ 失败'}（scripts/devimp-analyze.py）")
        report += [f"- {l}" for l in lines]
        if err:
            report.append(f"- **失败**: {err}")
            failed.append(("analyze", err))
        report.append("")
        headline += lines[:1]

    # ── 3. main ──
    if "main" in want:
        import dvmain
        def _main():
            import io
            import contextlib
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                rc = dvmain.main([outdir, "--since", a.since, "--min-n", str(a.min_n)])
            head = [l.strip() for l in buf.getvalue().splitlines() if l.strip()]
            return rc, head
        ok, lines, err, dt = run_stage("main", _main)
        report.append(f"## 3. main（dvmain.py）{'✅' if ok else '❌ 失败'}")
        report += [f"- {l}" for l in lines]
        if err:
            report.append(f"- **失败**: {err}")
            failed.append(("main", err))
        report.append("")
        headline += lines[:2]

    # ── 4. aff ──
    if "aff" in want:
        import dvaff
        def _aff():
            import io
            import contextlib
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                rc = dvaff.main([outdir, "--since", a.since])
            head = [l.strip() for l in buf.getvalue().splitlines() if l.strip()]
            return rc, head
        ok, lines, err, dt = run_stage("aff", _aff)
        report.append(f"## 4. aff（dvaff.py）{'✅' if ok else '❌ 失败'}")
        report += [f"- {l}" for l in lines]
        if err:
            report.append(f"- **失败**: {err}")
            failed.append(("aff", err))
        report.append("")
        headline += lines[1:][:2]

    # ── 5. status ──
    if "status" in want:
        import dvstatus
        def _status():
            import io
            import contextlib
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                rc = dvstatus.main([outdir])
            head = [l.strip() for l in buf.getvalue().splitlines() if l.strip()]
            return rc, head
        ok, lines, err, dt = run_stage("status", _status)
        report.append(f"## 5. status（dvstatus.py）{'✅' if ok else '❌ 失败'}")
        report += [f"- {l}" for l in lines]
        if err:
            report.append(f"- **失败**: {err}")
            failed.append(("status", err))
        report.append("")
        headline += lines[1:][:2]

    # ── 收尾：重跑命令 + 失败清单 ──
    tag = os.path.basename(outdir)
    report += ["## 单独重跑任一阶段", "",
               "```",
               f"# 解压（重建 {tag}/）",
               f"python scripts/devimp/dvextract.py {a.input} --tag {tag}",
               "# 聚合表（原脚本）",
               f"python scripts/devimp-analyze.py devimpbin/{tag} --since {a.since} --min-n {a.min_n}",
               "# 三个探针",
               f"python scripts/devimp/dvmain.py devimpbin/{tag} --since {a.since} --min-n {a.min_n}",
               f"python scripts/devimp/dvaff.py devimpbin/{tag} --since {a.since}",
               f"python scripts/devimp/dvstatus.py devimpbin/{tag}",
               "# 或一次性全跑",
               f"python scripts/devimp/dvrun.py {a.input} --tag {tag}",
               "```", ""]
    if failed:
        report += ["## 失败阶段", ""]
        report += [f"- **{n}**: {e}" for n, e in failed]
        report.append("")
    else:
        report += ["## 失败阶段", "", "- 无，全部阶段成功", ""]

    path = dc.write_report(outdir, "report.md", report)

    print(f"== dvrun 完成 -> {outdir}")
    for l in headline[:6]:
        print(f"   {l}")
    print(f"   报告: {path}")
    if failed:
        print(f"   [!] 失败阶段: {', '.join(n for n, _ in failed)}（见 report.md）")
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())
