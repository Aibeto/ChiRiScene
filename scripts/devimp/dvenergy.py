#!/usr/bin/env python3
"""功耗模型观察账；不作物理下界、调度归因或功耗 A/B。

Usage: dvenergy.py <status.csv|batch|root> [--since MMDD-HHMMSS] [--out directory]
--since selects archive batch stamps, not date-free CSV timestamps.
API: load(target, since) keeps the (rows, total, with_energy) tuple;
build(target, since) returns report text for dvrun and offline callers.
"""
import argparse
import collections
import os
import sys

import dvcommon as dc

fnum = dc.num


def pctile(sorted_vals, quantile):
    return sorted_vals[min(int(len(sorted_vals) * quantile), len(sorted_vals) - 1)] if sorted_vals else None


def linreg(model_values, total_values):
    """Ordinary least squares; the intercept is an extrapolation, not measured peripherals."""
    count = len(model_values)
    if count < 3:
        return None
    model_mean = sum(model_values) / count
    total_mean = sum(total_values) / count
    model_variance = sum((value - model_mean) ** 2 for value in model_values)
    total_variance = sum((value - total_mean) ** 2 for value in total_values)
    covariance = sum((model - model_mean) * (total - total_mean)
                     for model, total in zip(model_values, total_values))
    if model_variance <= 0:
        return None
    slope = covariance / model_variance
    return (total_mean - slope * model_mean, slope,
            covariance ** 2 / (model_variance * total_variance) if total_variance else 0.0, count)


def scene(record):
    """Control baseline and regression by continuous file segment, screen, mode, package."""
    return "|".join(str(record.get(field) or "-") for field in
                    ("_segment", "screen_prop", "mode", "package"))


def window_key(record):
    """行功率样本只在同一连续窗口内可比：类型/充放电/模式/包名/屏幕状态任一变化即边界。"""
    return tuple(record.get(field) for field in
                 ("type", "charge", "mode", "package", "screen_prop"))


def load(target, since="0000-000000"):
    rows = []
    total = 0
    with_energy = 0
    for path in dc.status_files(target, since):
        _, records = dc.read_status_file(path)
        # 跨类型/充放电/模式/包名/屏幕边界的区间归零，避免整段吞进旧场景。
        intervals, _ = dc.sampling_intervals(records, boundary_key=window_key)
        segment = 0
        previous_window = None
        for index, record in enumerate(records):
            total += 1
            current_window = window_key(record)
            # 窗口变化（含充放电、无效行中断）必须切段，baseline/regression 不跨段合并。
            if previous_window is not None and current_window != previous_window:
                segment += 1
            previous_window = current_window
            record["_segment"] = f"{os.path.relpath(path, target if os.path.isdir(target) else os.path.dirname(target))}#{segment}"
            record["_dt"] = intervals[index]
            # 缺口/边界处区间为 0，同样切开段，防止把缺口两侧并入同一段。
            if intervals[index] == 0:
                segment += 1
            power = fnum(record.get("batt_power_w"))
            if (record.get("charge") != "discharging" or record.get("type", "snap") != "snap"
                    or power is None or not 0 < power <= 20):
                continue
            rows.append(record)
            if fnum(record.get("cpu_dyn_w")) is not None and fnum(record.get("resid_w")) is not None:
                with_energy += 1
    return rows, total, with_energy


def account_scene(records):
    """Both increments subtract means from the identical low-model subset; never clamp residuals."""
    paired = [record for record in records if fnum(record.get("cpu_dyn_w")) is not None
              and fnum(record.get("resid_w")) is not None]
    model_values = sorted(fnum(record["cpu_dyn_w"]) for record in paired)
    baseline = None
    if len(paired) >= 20:
        threshold = pctile(model_values, .10)
        low = [record for record in paired if fnum(record["cpu_dyn_w"]) <= threshold]
        baseline = (dc.avg([fnum(record["batt_power_w"]) for record in low]),
                    dc.avg([fnum(record["cpu_dyn_w"]) for record in low]))
    result = dict(n=len(paired), baseline=baseline, negative=0,
                  sample_total=0.0, sample_model=0.0, integrated_total=0.0,
                  integrated_model=0.0, valid_dt=0.0, sample_increment=0.0,
                  sample_model_increment=0.0, integrated_increment=0.0,
                  integrated_model_increment=0.0)
    for record in paired:
        power = fnum(record["batt_power_w"])
        model = fnum(record["cpu_dyn_w"])
        elapsed = record["_dt"]
        result["negative"] += power - model < 0
        result["sample_total"] += power / 3600
        result["sample_model"] += model / 3600
        result["integrated_total"] += power * elapsed / 3600
        result["integrated_model"] += model * elapsed / 3600
        result["valid_dt"] += elapsed
        if baseline:
            total_increment = power - baseline[0]
            model_increment = model - baseline[1]
            result["sample_increment"] += total_increment / 3600
            result["sample_model_increment"] += model_increment / 3600
            result["integrated_increment"] += total_increment * elapsed / 3600
            result["integrated_model_increment"] += model_increment * elapsed / 3600
    return result


def build(target, since="0000-000000"):
    paths = dc.status_files(target, since)
    rows, total, with_energy = load(target, since)
    out = ["# dvenergy —— 动态模型 / 总口 / 残差观察账", f"# dir: {target}",
           f"# 放电 snap 行 {len(rows)} / 全部 status 行 {total}；有效分解行 {with_energy}",
           f"# scope: 所选文件汇总（非最新批次自动选择）；--since {since} 按归档批次戳过滤。",
           "# cpu_dyn_w 是未标定动态估计，不是物理下界；电压表来源有系统偏差。",
           "# 电量计刷新、频率与 util 的时间窗未在离线侧校准；残差可负且不夹零。",
           "# sample-equivalent Wh = ΣW/3600（每行假设1秒），不是整个会话实耗。",
           "# integrated Wh = 左端功率 × 实际 dt，仅 0<dt<10s；末行无区间。",
           "# gaps 是未知覆盖，不能直接解释为 suspend；长缺口不外推。",
           "# 无累计电量计账，无法恢复缺口能量；跨版本/设备不可作功耗对照。",
           "\n# [0] 采样覆盖与字段可用性"]
    for path in paths:
        columns, records = dc.read_status_file(path)
        out.append(dc.sampling_report(path, records))
        missing = [field for field in ("cpu_dyn_w", "resid_w") if field not in columns]
        available = sum(fnum(record.get("cpu_dyn_w")) is not None
                        and fnum(record.get("resid_w")) is not None for record in records)
        out.append(f"    分解有效比例={available}/{len(records)} "
                   + (f"缺列: {missing}" if missing else
                      "两列存在但全缺值（无表/采集缺失，不能判定老版本）" if not available else "两列存在"))
    if not rows:
        out.append("[!] 无有效放电 snap 行，无法分解。")
    if not with_energy:
        out.append("[!] 无有效分解数据；原因见逐文件缺列/全缺值，不能把无表读成老包。")
        return "\n".join(out) + "\n"
    grouped = collections.defaultdict(list)
    for record in rows:
        if fnum(record.get("cpu_dyn_w")) is not None and fnum(record.get("resid_w")) is not None:
            grouped[scene(record)].append(record)
    negative = sum(fnum(record["batt_power_w"]) < fnum(record["cpu_dyn_w"])
                   for values in grouped.values() for record in values)
    out.append(f"# negative resid 比例（P_total-P_model<0）: {negative}/{with_energy} = {negative / with_energy:.2%}")
    out += ["\n# [1] 同基准三层账与增量：batch|screen|mode|package（batch含文件及连续segment）",
            "# baseline_total / baseline_cpu_model = 同组 CPU 模型最低10%分位子集的各自均值；n<20不给基线。",
            "# 低分位是观察代理，不是纯外围常数；总增量不等于 CPU 增量。",
            "# 未解释 = 总增量 - 模型动态增量，包含 GPU、外围及模型误差。"]
    for key, records in sorted(grouped.items()):
        result = account_scene(records)
        out.append(f"  {key} n={result['n']} negative_resid={result['negative'] / result['n']:.2%}")
        for label, prefix in (("sample-equivalent Wh", "sample"), ("integrated Wh", "integrated")):
            total_energy = result[prefix + "_total"]
            model_energy = result[prefix + "_model"]
            out.append(f"    {label}: total={total_energy:.6f} model={model_energy:.6f} resid={total_energy - model_energy:.6f}")
        out.append(f"    paired valid_dt={result['valid_dt']:.3f}s")
        if result["baseline"] is None:
            out.append("    baseline: 样本不足")
        else:
            out.append("    baseline_total=%.6fW baseline_cpu_model=%.6fW" % result["baseline"])
            for label, prefix in (("sample-equivalent Wh", "sample"), ("integrated Wh", "integrated")):
                total_increment = result[prefix + "_increment"]
                model_increment = result[prefix + "_model_increment"]
                out.append(f"    {label} 增量: total={total_increment:.6f} model={model_increment:.6f} 未解释={total_increment - model_increment:.6f}")
    out += ["\n# [2] 回归：同 batch|screen|mode|package 连续组，总口 = a + b × 动态模型",
            "# a 是外推截距，b 不是标定常数；R2低不能单独证明量纲错误或外围主导。",
            "# 内容、亮度及热状态未完整控制；不能从回归声称模型已校准。"]
    for key, records in sorted(grouped.items()):
        regression = linreg([fnum(record["cpu_dyn_w"]) for record in records],
                            [fnum(record["batt_power_w"]) for record in records])
        if regression:
            intercept, slope, correlation, count = regression
            out.append(f"  {key} n={count} a={intercept:.6f} b={slope:.6f} R2={correlation:.6f}")
        else:
            out.append(f"  {key}: 回归不可用（样本不足/模型无方差）")
    return "\n".join(out) + "\n"


def selftest():
    regression = linreg([0, 1, 2], [2, 3, 4])
    passed = regression is not None and abs(regression[0] - 2) < 1e-9 and abs(regression[1] - 1) < 1e-9
    print(f"selftest: intercept and slope -> {passed}")
    return 0 if passed else 1


def main(argv=None):
    dc.setup_console()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target")
    parser.add_argument("--since", default="0000-000000")
    parser.add_argument("--out")
    arguments = parser.parse_args(argv)
    if not os.path.exists(arguments.target):
        parser.error("target does not exist")
    try:
        text = build(arguments.target, arguments.since)
    except ValueError as error:
        parser.error(str(error))
    directory = arguments.out or (arguments.target if os.path.isdir(arguments.target)
                                  else os.path.dirname(arguments.target))
    destination = dc.write_report(directory, "energy.txt", text.splitlines())
    dc.announce("dvenergy", destination, [line for line in text.splitlines()
                                         if line.startswith(("# 放电", "[!]", "# negative"))])
    return 0


if __name__ == "__main__":
    sys.exit(selftest() if sys.argv[1:] == ["--selftest"] else main())
