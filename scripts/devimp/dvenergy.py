#!/usr/bin/env python3
"""dvenergy.py —— 功耗三层分解（总口 / CPU 动态 / 残差）+ 外围基线剥离。

解决的老问题：status.csv 的 `batt_power_w` 是电池端总功率，里面装着屏幕、modem、GPU、
相机、静态漏电这些调度层碰不到的外围。直接拿它按包归因，会把「这人开了多久屏幕」
当成「ChiRi 调度得怎么样」。本脚本用 daemon 新增的 `cpu_dyn_w` / `resid_w` 两列把
CPU 侧单独拎出来，再给每个场景算一条外围基线，让增量归因只看 CPU 那一份。

用法：
    python scripts\\devimp\\dvenergy.py <解压目录>
    python scripts\\devimp\\dvenergy.py --selftest

口径（判读铁律）：
  1. `cpu_dyn_w` 是能效表折算的**动态项**，不含漏电/静态，是 CPU 功耗的下界；
     表的电压借自另一批次，绝对值有系统偏差，只保证相对可比。
  2. `resid_w` = batt_power_w − cpu_dyn_w，残差里混着外围与 CPU 静态，**不是纯外围**。
     要拿它当外围基线，必须用 [2] 的「低负载分位」做法把 CPU 静态压到最低。
  3. 只看放电行（charge=discharging）。充电行的 batt_power_w 是充电功率，残差无意义。
  4. 无功耗表的 SoC（当前只有 8550 有）两列恒 `-`，本脚本会明确报「本包无分解数据」。
  5. 本脚本**不做 A/B 对照**：[2][3] 的基线是同一会话内同场景的低负载分位，属观察口径，
     不是跨版本/跨配置的差值实验。

输出：写 UTF-8 结果文件 `energy.txt` 到解压目录（PowerShell 重定向会毁中文，见仓库已知坑 16）。
"""
import os
import sys
import io
import collections

def fnum(s):
    try:
        v = float(s)
    except (TypeError, ValueError):
        return None
    return v if v == v and abs(v) != float("inf") else None


def load(root):
    """读解压目录下所有 x_*/status.csv，返回 (放电行 list[dict], 总行数, 有分解列的行数)"""
    rows = []
    total = 0
    with_energy = 0
    for d in sorted(os.listdir(root)):
        p = os.path.join(root, d, "status.csv")
        if not os.path.isfile(p):
            continue
        with open(p, "r", encoding="utf-8", errors="replace") as f:
            cols = None
            for i, line in enumerate(f):
                parts = line.rstrip("\n").split(",")
                if i == 0:
                    cols = parts
                    continue
                if len(parts) < len(cols):
                    continue
                r = dict(zip(cols, parts))
                total += 1
                if r.get("charge") != "discharging":
                    continue
                if r.get("type", "snap") != "snap":
                    continue
                w = fnum(r.get("batt_power_w"))
                if w is None or w <= 0:
                    continue
                rows.append(r)
                if fnum(r.get("cpu_dyn_w")) is not None:
                    with_energy += 1
    return rows, total, with_energy


def pctile(sorted_vals, q):
    if not sorted_vals:
        return None
    i = int(len(sorted_vals) * q)
    return sorted_vals[min(i, len(sorted_vals) - 1)]


def linreg(xs, ys):
    """最小二乘 y = a + b*x，返回 (a, b, r2, n)"""
    n = len(xs)
    if n < 3:
        return None
    mx = sum(xs) / n
    my = sum(ys) / n
    sxx = sum((x - mx) ** 2 for x in xs)
    sxy = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    syy = sum((y - my) ** 2 for y in ys)
    if sxx <= 0:
        return None
    b = sxy / sxx
    a = my - b * mx
    r2 = (sxy * sxy / (sxx * syy)) if syy > 0 else 0.0
    return a, b, r2, n


def scene(r):
    """场景键：亮/息屏 + 前台包。screen_prop 1=息屏、2=亮屏、3/4=过渡态"""
    sp = r.get("screen_prop", "-")
    return "%s|%s" % (sp if sp in ("1", "2") else "other", r.get("package") or "-")


def build(root):
    """产出分解全文（纯函数，便于 dvrun 复用与自检）"""
    rows, total, with_energy = load(root)
    out = io.StringIO()
    out.write("# dvenergy —— 功耗三层分解（总口 / CPU 动态 / 残差）+ 外围基线剥离\n")
    out.write("# dir: %s\n" % root)
    out.write("# 放电 snap 行 %d / 全部 status 行 %d；含 cpu_dyn_w 的放电行 %d\n"
              % (len(rows), total, with_energy))

    if not rows:
        out.write("\n[!] 无放电行，无法分解。\n")
        return out.getvalue()
    if with_energy == 0:
        out.write("\n[!] 本包 status.csv 没有 cpu_dyn_w / resid_w 两列（老版本产物）。\n")
        out.write("    该分解需要 2026-10-05 之后的 daemon；旧包只能看总口 batt_power_w。\n")
        return out.getvalue()
    if with_energy < len(rows) * 0.5:
        out.write("\n[!] 只有 %d/%d 行带分解列（无功耗表的 SoC 恒 '-'，或本包跨版本）\n"
                  % (with_energy, len(rows)))

    # [1] 三层能量账
    out.write("\n# [1] 三层能量账（放电行，按 场景|包 聚合；只统计两列都有的行）\n")
    out.write("#     总 Wh = Σ batt_power_w/3600；CPU 动态 Wh = Σ cpu_dyn_w/3600；\n")
    out.write("#     残差 Wh = 总 − CPU 动态（含外围 + CPU 静态 + GPU，不是纯外围）。\n")
    out.write("#     CPU 占比低 = 这个场景的钱主要花在调度层碰不到的地方，别拿总口归因调度。\n")
    agg = collections.defaultdict(lambda: [0, 0.0, 0.0])
    for r in rows:
        w = fnum(r.get("batt_power_w"))
        c = fnum(r.get("cpu_dyn_w"))
        if w is None or c is None:
            continue
        e = agg[scene(r)]
        e[0] += 1
        e[1] += w
        e[2] += c
    out.write("  场景|包                                    n  总Wh   CPU动态Wh  残差Wh  CPU占比  均值W\n")
    for k in sorted(agg, key=lambda k: -agg[k][1])[:20]:
        n, sw, sc = agg[k]
        out.write("  %-38s %6d %6.2f %8.2f %8.2f  %5.1f%%  %5.2f\n"
                  % (k[:38], n, sw / 3600, sc / 3600, (sw - sc) / 3600,
                     sc / sw * 100 if sw else 0, sw / n))

    # [2] 外围基线：同场景内 cpu_dyn_w 最低 10% 分位的行，其 batt_power_w 均值
    out.write("\n# [2] 外围基线（同 场景|包 内 cpu_dyn_w 最低 10% 分位行的 batt_power_w 均值）\n")
    out.write("#     低 CPU 负载时的总功耗 ≈ 该场景的外围地板（屏幕+modem+静态），\n")
    out.write("#     同会话同场景的观察口径，不是跨配置对照。样本 <20 行不给出基线。\n")
    by_scene = collections.defaultdict(list)
    for r in rows:
        w = fnum(r.get("batt_power_w"))
        c = fnum(r.get("cpu_dyn_w"))
        if w is None or c is None:
            continue
        by_scene[scene(r)].append((c, w))
    base = {}
    out.write("  场景|包                                    n   基线W  CPU动态W(该分位)  可用\n")
    for k in sorted(by_scene, key=lambda k: -len(by_scene[k]))[:20]:
        vals = sorted(by_scene[k])
        n = len(vals)
        if n < 20:
            out.write("  %-38s %6d       -                -   否(样本不足)\n" % (k[:38], n))
            continue
        cut = pctile([v[0] for v in vals], 0.10)
        low = [w for c, w in vals if c <= cut]
        base[k] = sum(low) / len(low)
        out.write("  %-38s %6d %7.2f %14.2f   是\n" % (k[:38], n, base[k], cut or 0.0))

    # [3] 增量归因：Σ(batt_power_w − 基线)，只算有基线的场景
    out.write("\n# [3] CPU 增量能量（有基线的场景：Σ(batt_power_w − 基线)/3600）\n")
    out.write("#     这一列才是「这个场景里 CPU 侧多花的电」，调度改动该看它而不是总 Wh。\n")
    inc = collections.defaultdict(lambda: [0, 0.0, 0.0])
    for r in rows:
        k = scene(r)
        if k not in base:
            continue
        w = fnum(r.get("batt_power_w"))
        c = fnum(r.get("cpu_dyn_w"))
        if w is None or c is None:
            continue
        e = inc[k]
        e[0] += 1
        e[1] += w - base[k]
        e[2] += c
    out.write("  场景|包                                    n  增量Wh  其中CPU动态Wh  未解释Wh\n")
    for k in sorted(inc, key=lambda k: -inc[k][1])[:20]:
        n, si, sc = inc[k]
        out.write("  %-38s %6d %7.2f %12.2f %11.2f\n"
                  % (k[:38], n, si / 3600, sc / 3600, (si - sc) / 3600))

    # [4] 回归：batt_power_w ~ a + b*cpu_dyn_w（按 screen_prop 分组）
    out.write("\n# [4] 回归 batt_power_w = a + b × cpu_dyn_w（按 screen_prop 分组）\n")
    out.write("#     a = CPU 空载时的整机功耗（外围地板 + 静态），b = 能效表的标定系数，\n")
    out.write("#     R² 低说明该场景下功耗主要由 CPU 之外的东西驱动（屏幕/modem），\n")
    out.write("#     这时拿总口做前后对比必然被外围噪声淹没，必须看 [3] 的增量列。\n")
    grp = collections.defaultdict(lambda: ([], []))
    for r in rows:
        w = fnum(r.get("batt_power_w"))
        c = fnum(r.get("cpu_dyn_w"))
        if w is None or c is None:
            continue
        g = grp[r.get("screen_prop", "-")]
        g[0].append(c)
        g[1].append(w)
    out.write("  screen_prop     n    a(截距W)   b(斜率)     R²\n")
    for k in sorted(grp):
        xs, ys = grp[k]
        r = linreg(xs, ys)
        if not r:
            out.write("  %-12s %6d          -        -       -\n" % (k, len(xs)))
            continue
        a, b, r2, n = r
        out.write("  %-12s %6d %10.2f %9.2f %7.3f\n" % (k, n, a, b, r2))

    return out.getvalue()


def selftest():
    """合成数据自检：构造 batt = 2.0(外围地板) + cpu_dyn 的行，验证回归能还原 a≈2.0、b≈1.0"""
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_dvenergy_selftest")
    os.makedirs(root, exist_ok=True)
    hdr = ("timestamp,type,mode,package,charge,screen_on,batt_temp,cpu_temp,thermal_cap_pct,"
           "thermal_free_pct,clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,"
           "batt_voltage_v,batt_current_ma,batt_power_w,wakeups,migrations,freq_trans,fps,"
           "screen_prop,daemon_utime_ms,daemon_stime_ms,cpu_dyn_w,resid_w")
    lines = [hdr]
    for i in range(120):
        u = (i % 20) / 20.0
        cpu = 0.5 * u
        tot = 2.0 + cpu
        lines.append(",".join([
            "00:00:%02d.000" % (i % 60), "snap", "default", "com.example", "discharging", "1",
            "35", "45", "100", "95", "1", "1.0", "0.1", "0.0", "10",
            "4.0", "-500", "%.4f" % tot, "0", "0", "0", "-", "2", "1000", "500",
            "%.4f" % cpu, "%.4f" % (tot - cpu),
        ]))
    d = os.path.join(root, "x_selftest")
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, "status.csv"), "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    text = build(root)
    ok_a = ok_b = False
    for ln in text.splitlines():
        if ln.strip().startswith("2 "):
            parts = ln.split()
            try:
                a = float(parts[2]); b = float(parts[3])
                ok_a = abs(a - 2.0) < 0.05
                ok_b = abs(b - 1.0) < 0.05
            except (IndexError, ValueError):
                pass
    # 控制台可能是 GBK（已知坑 16），中文全文只落盘、stdout 打 ASCII 摘要
    with open(os.path.join(root, "energy.txt"), "w", encoding="utf-8") as f:
        f.write(text)
    print("selftest: intercept a~2.0 -> %s ; slope b~1.0 -> %s" % (ok_a, ok_b))
    return 0 if (ok_a and ok_b) else 1


def main(argv):
    """统一入口（与 dvpower/dvmain 同形）：argv = [解压目录, --since MMDD-HHMMSS]。

    `--since` 对 status.csv 无意义（无日期列），接受但忽略，仅为与 dvrun 的阶段参数对齐。
    """
    if not argv:
        print(__doc__)
        return 2
    target = argv[0]
    if not os.path.isdir(target):
        print("[x] 不是目录: %s" % target)
        return 2
    text = build(target)
    dst = os.path.join(target, "energy.txt")
    with open(dst, "w", encoding="utf-8") as f:
        f.write(text)
    # 控制台可能是 GBK：stdout 只打 ASCII 摘要（完整中文结果看 energy.txt）
    print("== dvenergy -> %s" % dst)
    for ln in text.splitlines():
        if ln.startswith("# 放电 snap 行") or ln.startswith("[!]"):
            print("   " + ln.encode("gbk", "replace").decode("gbk"))
    return 0


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "--selftest":
        sys.exit(selftest())
    sys.exit(main(sys.argv[1:]))
