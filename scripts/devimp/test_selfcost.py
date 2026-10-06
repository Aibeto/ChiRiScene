"""dvselfcost（阶段A 修订版）聚焦回归：只读夹具，不碰设备、不改原始日志。

运行：python3 -m unittest discover -s scripts/devimp -p 'test_selfcost.py'
覆盖：清单校验、跨午夜、ambiguous 回退、重复秒、多帧同秒、计数重置、五类身份边界、
缺口、空文件、缺字段、未知身份、覆盖≠确认、相位与「不按符号断言原因」、screen_prop 主层。
"""
import contextlib
import csv
import io
import json
import re
import unittest
from pathlib import Path
import tempfile

import dvselfcost as ds

STATUS_COLUMNS = ["timestamp", "type", "mode", "package", "charge", "screen_on", "batt_temp",
                  "cpu_temp", "thermal_cap_pct", "thermal_free_pct", "clg_active", "psi_cpu_some",
                  "psi_io_some", "psi_mem_some", "gpu_busy_pct", "batt_voltage_v",
                  "batt_current_ma", "batt_power_w", "wakeups", "migrations", "freq_trans", "fps",
                  "screen_prop", "daemon_utime_ms", "daemon_stime_ms", "cpu_dyn_w", "resid_w"]


def status_row(stamp, utime, stime, mode="default", package="pkg", screen_prop="2",
               screen_on="1", clg="1", rtype="snap"):
    return {"timestamp": stamp, "type": rtype, "mode": mode, "package": package,
            "screen_on": screen_on, "clg_active": clg, "screen_prop": screen_prop,
            "daemon_utime_ms": str(utime), "daemon_stime_ms": str(stime)}


class SelfCostTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=Path(__file__).parent)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def make_status(self, rows, columns=None):
        path = self.root / "x_1000-000000" / "status.csv"
        path.parent.mkdir(parents=True, exist_ok=True)
        columns = columns or STATUS_COLUMNS
        with path.open("w", encoding="utf-8", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=columns)
            writer.writeheader()
            for row in rows:
                full = dict.fromkeys(columns, "-")
                full.update({key: value for key, value in row.items() if key in full})
                writer.writerow(full)
        return path

    def make_case(self, rows=None, aff_lines=(), main_lines=(), case_id="C", version="V1",
                  config="CFG1", session="S1", logical=None, manifest=True, aff=True):
        status = self.make_status(rows or [])
        devimp = self.root / ("x_devimp_%s" % case_id)
        devimp.mkdir(parents=True, exist_ok=True)
        aff_path = devimp / "aff_1000-000000.log"
        if aff:
            aff_path.write_text("\n".join(aff_lines), encoding="utf-8")
        for name, content in main_lines:
            (devimp / name).write_text("\n".join(content), encoding="utf-8")
        case = {"case_id": case_id, "status_path": str(status), "devimp_dir": str(devimp),
                "aff_path": str(aff_path), "version_identity": version,
                "config_identity": config, "session_id": session}
        if logical is not None:
            case["logical_cpus"] = logical
        return case if not manifest else self.write_manifest([case])

    def write_manifest(self, cases):
        path = self.root / "manifest.json"
        path.write_text(json.dumps({"cases": cases}, ensure_ascii=False), encoding="utf-8")
        return path

    def run_cli(self, command, manifest):
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            code = ds.main([command, "--manifest", str(manifest)])
        return code, buffer.getvalue()

    def load(self, case):
        return ds.load_status(case)

    # [清单与输入]
    def test_manifest_requires_seven_fields(self):
        bad = self.root / "bad.json"
        bad.write_text(json.dumps({"cases": [{"case_id": "C"}]}), encoding="utf-8")
        with self.assertRaises(ds.CaseError):
            ds.load_manifest(str(bad), str(self.root))

    def test_manifest_rejects_duplicate_case_id(self):
        case = self.make_case(case_id="DUP", manifest=False)
        path = self.write_manifest([case, case])
        with self.assertRaises(ds.CaseError):
            ds.load_manifest(str(path), str(self.root))

    def test_manifest_top_level_shape(self):
        path = self.root / "shape.json"
        path.write_text(json.dumps({"items": []}), encoding="utf-8")
        with self.assertRaises(ds.CaseError):
            ds.load_manifest(str(path), str(self.root))

    def test_unknown_identity_is_recorded_not_compared(self):
        case = self.make_case(rows=[status_row("00:00:00.000", 0, 0)], version="", config="",
                              manifest=False)
        loaded = ds.load_manifest(str(self.write_manifest([case])), str(self.root))[0]
        self.assertEqual(loaded["version_identity"], ds.UNKNOWN)
        self.assertEqual(loaded["config_identity"], ds.UNKNOWN)
        code, text = self.run_cli("feas", self.write_manifest([case]))
        self.assertEqual(code, 0)
        self.assertIn("未知身份", text)

    def test_missing_file_fails_explicitly(self):
        case = self.make_case(rows=[status_row("00:00:00.000", 0, 0)], aff=False, manifest=False)
        path = self.write_manifest([case])
        buffer = io.StringIO()
        with contextlib.redirect_stderr(buffer):
            code = ds.main(["feas", "--manifest", str(path)])
        self.assertEqual(code, 2)
        self.assertIn("aff_path 文件不存在", buffer.getvalue())

    def test_cli_requires_manifest_and_known_subcommand(self):
        for argv in ([], ["feas"], ["bogus", "--manifest", "x"]):
            with self.assertRaises(SystemExit):
                ds.main(argv)

    def test_missing_required_column_fails(self):
        columns = [name for name in STATUS_COLUMNS if name != "screen_prop"]
        path = self.make_status([status_row("00:00:00.000", 0, 0)], columns=columns)
        case = {"case_id": "C", "status_path": str(path), "devimp_dir": str(self.root),
                "aff_path": str(path), "version_identity": "V1", "config_identity": "C1",
                "session_id": "S1"}
        with self.assertRaises(ds.CaseError):
            ds.load_status(case)

    # [时间轴]
    def test_midnight_expansion_keeps_continuity(self):
        rows = [status_row("23:59:59.000", 100, 0), status_row("00:00:00.000", 120, 0),
                status_row("00:00:01.000", 140, 0)]
        info = self.load(self.make_case(rows=rows, manifest=False))
        layers, stats, _over = ds.diff_layers(info["rows"])
        self.assertEqual(info["timeline"].midnight, 1)
        self.assertEqual(layers["strict"].count, 2)
        self.assertEqual(stats["segment"], 0)

    def test_ambiguous_backward_jump_cuts_segment(self):
        rows = [status_row("12:00:02.000", 100, 0), status_row("12:00:01.000", 200, 0),
                status_row("12:00:02.000", 300, 0)]
        info = self.load(self.make_case(rows=rows, manifest=False))
        layers, stats, _over = ds.diff_layers(info["rows"])
        self.assertEqual(info["timeline"].ambiguous, 1)
        self.assertEqual(layers["strict"].count, 1)
        self.assertEqual(stats["segment"], 1)

    def test_unknown_time_cuts_and_is_not_averaged(self):
        rows = [status_row("00:00:00.000", 0, 0), status_row("", 900, 0),
                status_row("00:00:02.000", 20, 0)]
        info = self.load(self.make_case(rows=rows, manifest=False))
        layers, stats, _over = ds.diff_layers(info["rows"])
        self.assertEqual(info["dropped"]["bad_timestamp"], 1)
        self.assertEqual(layers["strict"].count, 0)
        self.assertEqual(layers["short"].count, 0)
        self.assertEqual(stats["segment"], 1)

    def test_bad_cpu_cuts_and_is_reported(self):
        rows = [status_row("00:00:00.000", 0, 0), status_row("00:00:01.000", "nan", 0),
                status_row("00:00:02.000", 20, 0)]
        info = self.load(self.make_case(rows=rows, manifest=False))
        layers, _stats, _over = ds.diff_layers(info["rows"])
        self.assertEqual(info["dropped"]["bad_cpu"], 1)
        self.assertEqual(layers["strict"].count, 0)

    def test_duplicate_second_counted(self):
        rows = [status_row("00:00:00.100", 0, 0), status_row("00:00:00.900", 10, 0)]
        info = self.load(self.make_case(rows=rows, manifest=False))
        self.assertEqual(info["dup_second"], 1)

    # [差分口径]
    def test_short_interval_rate_uses_actual_duration(self):
        rows = [status_row("00:00:00.000", 0, 0), status_row("00:00:02.000", 20, 0)]
        info = self.load(self.make_case(rows=rows, manifest=False))
        layers, _stats, _over = ds.diff_layers(info["rows"])
        self.assertEqual(layers["short"].count, 1)
        self.assertAlmostEqual(layers["short"].rate, 10.0)

    def test_three_rows_one_second_span_is_two_seconds(self):
        rows = [status_row("00:00:00.000", 0, 0), status_row("00:00:01.000", 10, 0),
                status_row("00:00:02.000", 20, 0)]
        info = self.load(self.make_case(rows=rows, manifest=False))
        layers, _stats, _over = ds.diff_layers(info["rows"])
        self.assertAlmostEqual(layers["strict"].dt_sum, 2.0)
        self.assertEqual(layers["strict"].count, 2)

    def test_gap_counted_without_extrapolation(self):
        rows = [status_row("00:00:00.000", 0, 0), status_row("00:01:40.000", 500, 0)]
        info = self.load(self.make_case(rows=rows, manifest=False))
        layers, stats, _over = ds.diff_layers(info["rows"])
        self.assertEqual(stats["gap"], 1)
        self.assertAlmostEqual(stats["gap_dt"], 100.0)
        self.assertEqual(layers["strict"].count + layers["short"].count, 0)

    def test_count_reset_excluded(self):
        rows = [status_row("00:00:00.000", 500, 0), status_row("00:00:01.000", 100, 0)]
        info = self.load(self.make_case(rows=rows, manifest=False))
        layers, stats, _over = ds.diff_layers(info["rows"])
        self.assertEqual(stats["cpu_reset"], 1)
        self.assertEqual(layers["strict"].count, 0)

    def test_no_unjustified_3000_filter_and_physical_bound(self):
        rows = [status_row("00:00:00.000", 0, 0), status_row("00:00:01.000", 5000, 0)]
        info = self.load(self.make_case(rows=rows, manifest=False))
        layers, _stats, _over = ds.diff_layers(info["rows"])
        self.assertEqual(layers["strict"].count, 1)
        layers_b, _stats_b, over = ds.diff_layers(info["rows"], logical_cpus=2)
        self.assertEqual(layers_b["strict"].count, 0)
        self.assertEqual(len(over), 1)
        self.assertEqual(over[0][1], 5000)

    def test_each_identity_boundary_cuts(self):
        for field, changed in (("type", "tick"), ("mode", "fas"), ("package", "other"),
                               ("screen_prop", "3"), ("clg_active", "0")):
            with self.subTest(field=field):
                first = status_row("00:00:00.000", 0, 0)
                middle = status_row("00:00:01.000", 10, 0)
                last = status_row("00:00:02.000", 20, 0)
                middle[field] = changed
                info = self.load(self.make_case(rows=[first, middle, last], manifest=False))
                layers, stats, _over = ds.diff_layers(info["rows"])
                self.assertEqual(layers["strict"].count, 0, field)
                self.assertEqual(stats["boundary"], 2, field)

    def test_screen_on_change_does_not_cut_but_conflict_is_listed(self):
        rows = [status_row("00:00:00.000", 0, 0, screen_on="1", screen_prop="2"),
                status_row("00:00:01.000", 10, 0, screen_on="0", screen_prop="2")]
        info = self.load(self.make_case(rows=rows, manifest=False))
        layers, stats, _over = ds.diff_layers(info["rows"])
        self.assertEqual(layers["strict"].count, 1)
        self.assertEqual(stats["boundary"], 0)
        self.assertEqual(info["conflicts"], 1)

    def test_placeholder_values_are_empty_not_dropped(self):
        rows = [status_row("00:00:00.000", 0, 0, clg="-"), status_row("00:00:01.000", 10, 0, clg="-")]
        info = self.load(self.make_case(rows=rows, manifest=False))
        self.assertEqual(info["empty"]["clg_active"], 2)
        self.assertEqual(info["rows"][0]["clg_active"], ds.UNKNOWN)

    def test_rate_is_insufficient_not_zero(self):
        self.assertIsNone(ds.Layer().rate)
        self.assertEqual(ds.fmt_rate(None), "insufficient")
        self.assertEqual(ds.fmt_pct(0, 0), "insufficient")

    def test_single_row_reports_insufficient_coverage(self):
        manifest = self.make_case(rows=[status_row("00:00:00.000", 0, 0)])
        code, text = self.run_cli("feas", manifest)
        self.assertEqual(code, 0)
        self.assertIn("覆盖=insufficient", text)

    # [aff 帧]
    def test_frames_keep_duplicate_seconds_without_merging(self):
        lines = ["@S ts=1000-000000 ntop=1 nfg=1",
                 "@S ts=1000-000000 ntop=1 nfg=2",
                 "@S ts=1000-000002 ntop=1 nfg=3",
                 "@S ts=1000-000003 ntop=1 nfg=4"]
        case = self.make_case(rows=[status_row("00:00:00.000", 0, 0)], aff_lines=lines,
                              manifest=False)
        frames = ds.load_frames(case)
        self.assertEqual(len(frames["frames"]), 4)
        self.assertEqual(sorted(frame["nfg"] for frame in frames["frames"]), [1, 2, 3, 4])

    def test_frame_line_without_date_is_unmatched_not_silently_kept(self):
        case = self.make_case(rows=[status_row("00:00:00.000", 0, 0)],
                              aff_lines=["@S ts=000000 ntop=1 nfg=1"], manifest=False)
        frames = ds.load_frames(case)
        self.assertEqual(frames["frames"], [])
        self.assertEqual(frames["unmatched"], 1)

    def test_empty_aff_reports_zero_activity(self):
        manifest = self.make_case(rows=[status_row("00:00:00.000", 0, 0), status_row("00:00:01.000", 10, 0)])
        code, text = self.run_cli("feas", manifest)
        self.assertEqual(code, 0)
        self.assertIn("aff 帧=0", text)

    # [diag 覆盖 ≠ 确认]
    def test_coverage_leaves_hole_unfilled(self):
        main = [("main_pkg_1000-000000.log",
                 ["00:00:00.000,x", "00:00:01.000,x", "00:00:02.000,x",
                  "00:00:10.000,x", "00:00:11.000,x"])]
        case = self.make_case(rows=[status_row("00:00:00.000", 0, 0)], main_lines=main,
                              manifest=False)
        covered, _report = ds.coverage_seconds(case["devimp_dir"])
        self.assertEqual(covered, {0, 1, 2, 10, 11})

    def test_explicit_events_need_markers(self):
        path = self.root / "daemon.log"
        path.write_text("".join(
            "[2026-10-07 10:00:00] [INFO] [chiri] 普通的启动信息\n"
            "[2026-10-07 10:00:05] [INFO] [chiri] 开发记录已关闭\n"
            "[2026-10-07 10:00:09] [INFO] [chiri] 开发记录已开启\n"), encoding="utf-8")
        on, off, matched = ds.explicit_events([str(path)])
        self.assertEqual(len(off), 1)
        self.assertEqual(len(on), 1)
        self.assertEqual(len(matched), 2)

    def test_diag_keeps_state_unknown_without_evidence(self):
        main = [("main_pkg_1000-000000.log", ["00:00:00.000,x", "00:00:01.000,x",
                                              "00:00:02.000,x"])]
        rows = [status_row("00:00:%02d.000" % second, second * 10, 0) for second in range(3)]
        case = self.make_case(rows=rows, main_lines=main, manifest=False)
        code, text = self.run_cli("diag", self.write_manifest([case]))
        self.assertEqual(code, 0)
        self.assertTrue(re.search(r"confirmed_off\s+n=0\b", text))
        self.assertTrue(re.search(r"confirmed_on\s+n=0\b", text))
        self.assertIn("coverage_only", text)
        self.assertIn("不可判定", text)

    # [pair]
    def test_pair_reports_phase_sensitivity_without_causal_claim(self):
        cpu = [0, 5, 25, 30, 50, 55, 75, 80, 100, 105, 125, 150, 170]
        rows = [status_row("00:00:%02d.000" % second, value, 0)
                for second, value in enumerate(cpu)]
        frames = ["@S ts=1000-0000%02d ntop=1 nfg=10" % second
                  for second in (0, 2, 4, 6, 8, 10)]
        case = self.make_case(rows=rows, aff_lines=frames, manifest=False)
        code, text = self.run_cli("pair", self.write_manifest([case]))
        self.assertEqual(code, 0)
        self.assertIn("严格连续亮屏跳帧配对 n=6", text)
        self.assertIn("相位A", text)
        self.assertIn("一律不成立", text)
        self.assertNotIn("说明跳帧样本存在选择偏差", text)

    def test_pair_insufficient_when_no_window(self):
        rows = [status_row("00:00:00.000", 0, 0), status_row("00:00:01.000", 10, 0)]
        case = self.make_case(rows=rows, aff_lines=["@S ts=1000-000000 ntop=1 nfg=1"],
                              manifest=False)
        code, text = self.run_cli("pair", self.write_manifest([case]))
        self.assertEqual(code, 0)
        self.assertIn("insufficient", text)


if __name__ == "__main__":
    unittest.main()