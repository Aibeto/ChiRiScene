#!/usr/bin/env python3
"""dvstatus.py —— `status.csv` + `daemon.log` 预设探针。

用法:
    python scripts/devimp/dvstatus.py <解压目录> [--out 目录]

daemon.log：
- **先剥 U+2068/U+2069 隔离符**（`P⁨0⁩`、`⁨playback⁩`），否则 `P(\\d+)`、`mode=…` 全匹配不到
  （已知坑 10）；daemon.log 的文本带这两类字符。
- 模块版本行（`[Main] 模块版本:`，A06 起有）；启动序列界标（`统一启动中`），
  重启会把热状态重置为 cap=100 → **cap 序列不可跨重启连读**。
- FAS 证据（`fas-gear-switch` / `fas-low-perf-upgrade` / `gear_state` / `policy_controller`）。
- 看门狗状态（watchdog.pid 是否被 logger 报缺失）、`logger-log-restart-suppressed` 与打包告警。

status.csv（权威的充放电信号）：
- `charge` 列分布（**不要**用电流符号判方向）。
- 仅放电行的 `batt_power_w` 统计（avg / p50 / p95）。
- `fps` 列：非空行数与统计。fps 是 eBPF uprobe（`Surface::queueBuffer`），**只在 FAS
  活跃时**才有值 → 「0 个非空行」正面说明 FAS 从未接管（照此结论书写）。

logd 侧整体缺失时优雅降级，并明确列出因此不可得的结论（无 charge/fps/FAS/PowerAVG）。
"""

import argparse
import collections
import glob
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dvcommon as dc  # noqa: E402

STATUS_COLS = ("timestamp,type,mode,package,charge,screen_on,batt_temp,cpu_temp,thermal_cap_pct,"
               "thermal_free_pct,clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,"
               "batt_voltage_v,batt_current_ma,batt_power_w,wakeups,migrations,freq_trans,fps").split(",")
SI = dc.index_map(STATUS_COLS)

FAS_MARKERS = ("fas-gear-switch", "fas-low-perf-upgrade", "gear_state", "policy_controller",
               "fas_activate", "mode=fas")
DEGRADED = ("charge 方向（充放电判定）", "fps（FAS 帧率证据）", "FAS 接管证据（daemon.log）",
            "PowerAVG", "热压制事件（daemon.log 的启动/重启界标）")


def daemon_report(paths):
    out = ["# ── daemon.log ──"]
    if not paths:
        out.append("  （无 daemon.log）")
        return out
    versions, landmarks, fas_hits, warn = [], [], collections.Counter(), collections.Counter()
    restarts = 0
    for fn in paths:
        with open(fn, encoding="utf-8", errors="replace") as fh:
            for ln in fh:
                s = dc.strip_isolates(ln)
                if "模块版本:" in s:
                    versions.append(s.strip())
                if "统一启动中" in s:
                    restarts += 1
                    landmarks.append(s.strip())
                if "已归档，后台打包至" in s:
                    landmarks.append(s.strip())
                for k in FAS_MARKERS:
                    if k in s:
                        fas_hits[k] += 1
                for k in ("logger-log-restart-suppressed", "Permission denied",
                          "meta.yaml fields invalid", "was not archived", "未归档"):
                    if k in s:
                        warn[k] += 1
    out.append(f"  文件: {', '.join(os.path.basename(p) for p in paths)}")
    out.append(f"  模块版本行 ({len(versions)}):")
    for v in sorted(set(versions)):
        out.append(f"    {v[:200]}")
    if len(set(versions)) > 1:
        out.append("    [!] 多版本行 —— 包内混装了不同版本的日志")
    out.append(f"  重启界标：`统一启动中` x{restarts}"
               + ("  [!] 多段重启，热状态在重启处归 100，cap 序列不可跨重启连读" if restarts > 1 else ""))
    for l in landmarks[:12]:
        out.append(f"    - {l[:180]}")
    out.append("  启动归档界标（[Main] 上一轮…归档）:")
    for l in [x for x in landmarks if "归档，后台打包至" in x][:8]:
        out.append(f"    - {l[:180]}")
    out.append("  FAS 证据:")
    if fas_hits:
        for k, n in fas_hits.most_common():
            out.append(f"    {k}: {n}")
    else:
        out.append("    **无** —— 本包 daemon.log 里没有任何 FAS 接管痕迹")
    out.append("  看门狗 / 打包告警:")
    if warn:
        for k, n in warn.most_common():
            out.append(f"    {k}: {n}")
    else:
        out.append("    （无 logger-log-restart-suppressed、无权限/打包告警）")
    out.append("")
    return out


def status_report(paths):
    out = ["# ── status.csv ──",
           "#     charge 列是**权威**的充放电信号；不要用电流符号判方向。",
           "#     fps 是 eBPF uprobe（Surface::queueBuffer），**只在 FAS 活跃时**有值 →",
           "#     「0 个非空行」= 正面证据：FAS 从未接管。"]
    if not paths:
        out.append("  （无 status.csv）")
        return out, None
    charge = collections.Counter()
    power_dis = collections.defaultdict(list)
    power_chg = collections.defaultdict(list)
    fps_rows = []
    rows = 0
    cap_hist = collections.Counter()
    for fn in paths:
        with open(fn, encoding="utf-8", errors="replace") as fh:
            for ln in fh:
                p = ln.rstrip("\n").split(",")
                if len(p) < len(STATUS_COLS) or p[0] == STATUS_COLS[0]:
                    continue
                if not dc.TS_RE.match(ln):
                    continue
                rows += 1
                ch = p[SI["charge"]] or "-"
                mode = p[SI["mode"]] or "-"
                charge[ch] += 1
                pw = dc.num(p[SI["batt_power_w"]])
                if pw is not None:
                    if ch == "discharging":
                        power_dis[mode].append(pw)
                    elif ch == "charging":
                        power_chg[mode].append(pw)
                cap_hist[p[SI["thermal_cap_pct"]]] += 1
                fv = p[SI["fps"]]
                if fv not in ("-", "", None):
                    fps_rows.append(dc.num(fv))
    out.append(f"  文件: {', '.join(os.path.basename(p) for p in paths)}")
    out.append(f"  数据行: {rows}")
    out.append(f"  charge 分布: {dict(charge)}")
    out.append(f"  thermal_cap_pct 分布（前 8）: {dict(cap_hist.most_common(8))}")
    out.append("")
    out.append("  仅放电行的 batt_power_w（W）统计:")
    out.append(f"    {'mode':10s} {'n':>5s} {'avg':>7s} {'p50':>7s} {'p95':>7s}")
    for mode in sorted(power_dis, key=lambda m: -len(power_dis[m])):
        v = power_dis[mode]
        out.append(f"    {mode:10s} {len(v):5d} {dc.avg(v):7.3f} {dc.pct(v, .5):7.3f} {dc.pct(v, .95):7.3f}")
    if power_chg:
        allc = [x for v in power_chg.values() for x in v]
        out.append(f"  （充电行 {len(allc)} 条，batt_power_w avg={dc.avg(allc):.3f}，仅供方向核对）")
    out.append("")
    fps_clean = [x for x in fps_rows if x is not None]
    out.append(f"  fps 列: 非空行 {len(fps_clean)} / 总 {rows}")
    if fps_clean:
        out.append(f"    avg={dc.avg(fps_clean):.2f} p50={dc.pct(fps_clean, .5):.2f} "
                   f"p95={dc.pct(fps_clean, .95):.2f} min={min(fps_clean):.2f} max={max(fps_clean):.2f}")
        out.append("    [!] 有值 → FAS 至少活跃过一段，需结合 daemon.log 的 FAS 界标定位该时段")
    else:
        out.append("    **0 个非空行** = FAS 从未接管（fps 探针仅在 FAS 活跃时写入）")
    out.append("")
    return out, dict(rows=rows, charge=charge, fps=len(fps_clean))


def main(argv=None):
    dc.setup_console()
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--out", default=None)
    a = ap.parse_args(argv)

    root = os.path.abspath(a.dir)
    stat = sorted(glob.glob(os.path.join(root, "**", "status.csv"), recursive=True))
    daem = sorted(glob.glob(os.path.join(root, "**", "daemon.log"), recursive=True))

    out = ["# dvstatus —— status.csv + daemon.log 探针", f"# dir: {root}", ""]
    out += daemon_report(daem)
    so, summary = status_report(stat)
    out += so
    if not stat and not daem:
        out.append("## [!] logd 侧整体缺失 —— 以下结论不可得（优雅降级）")
        for d in DEGRADED:
            out.append(f"  - {d}")
    elif not stat:
        out.append("## [!] 无 status.csv —— charge/fps/PowerAVG 结论不可得（已知坑 12 的降级口径）")

    workdir = a.out or root
    path = dc.write_report(workdir, "status.txt", out)
    lines = [f"daemon.log {len(daem)} 个, status.csv {len(stat)} 个"]
    if summary:
        lines.append(f"status 行 {summary['rows']}, charge={dict(summary['charge'])}, "
                     f"fps 非空 {summary['fps']}")
    if not stat:
        lines.append("[!] 缺 status.csv：charge/fps/FAS/PowerAVG 判读不可得")
    dc.announce("dvstatus", path, lines)
    return 0


if __name__ == "__main__":
    sys.exit(main())
