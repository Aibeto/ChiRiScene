"""Focused offline accounting regressions; no device or archive writes."""
import csv
import contextlib
import io
import os
from pathlib import Path
import tempfile
import unittest

import dvcommon as dc
import dvenergy
import dvstatus
import dvmain
import dvpower
import dvrun


class AccountingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=Path(__file__).parent)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def write_status(self, batch="x_1006-180717", count=30, energy=True):
        directory = self.root / batch
        directory.mkdir(exist_ok=True)
        path = directory / "status.csv"
        columns = dvstatus.STATUS_COLS + ["screen_prop"]
        if energy:
            columns += ["cpu_dyn_w", "resid_w"]
        with path.open("w", encoding="utf-8", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=columns)
            writer.writeheader()
            for index in range(count):
                row = dict.fromkeys(columns, "-")
                row.update(timestamp=f"00:00:{index:02d}.000", type="snap",
                           mode="fas", package="game", charge="discharging",
                           screen_prop="2", batt_power_w=str(2 + index / 10))
                if energy:
                    row.update(cpu_dyn_w=str(1 + index / 10), resid_w="1")
                writer.writerow(row)
        return path

    def write_rows(self, batch, rows):
        directory = self.root / batch
        directory.mkdir(exist_ok=True)
        path = directory / "status.csv"
        columns = dvstatus.STATUS_COLS + ["screen_prop", "cpu_dyn_w", "resid_w"]
        with path.open("w", encoding="utf-8", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=columns)
            writer.writeheader()
            for row in rows:
                full = dict.fromkeys(columns, "-")
                full.update(row)
                writer.writerow(full)
        return path

    def test_direct_file_load(self):
        path = self.write_status()
        self.assertEqual(len(dvenergy.load(str(path))[0]), 30)

    def test_batch_load(self):
        path = self.write_status()
        self.assertEqual(len(dvenergy.load(str(path.parent))[0]), 30)

    def test_since_selection(self):
        self.write_status("x_1005-000000")
        self.write_status("x_1006-180717")
        self.assertEqual(len(dvenergy.load(str(self.root), since="1006-000000")[0]), 30)

    def test_missing_values_not_missing_columns(self):
        path = self.write_status()
        with path.open(newline="") as stream:
            rows = list(csv.DictReader(stream))
        for row in rows:
            row["cpu_dyn_w"] = "-"
            row["resid_w"] = "-"
        with path.open("w", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=rows[0])
            writer.writeheader()
            writer.writerows(rows)
        report = dvenergy.build(str(self.root))
        self.assertIn("全缺值", report)
        self.assertNotIn("老版本产物", report)

    def test_absent_columns(self):
        self.write_status(energy=False)
        self.assertIn("缺列", dvenergy.build(str(self.root)))

    def test_no_fps_does_not_disprove_fas(self):
        path = self.write_status()
        lines, summary = dvstatus.status_report([str(path)])
        self.assertNotIn("FAS 从未接管", "\n".join(lines))
        self.assertIn("mode=fas", "\n".join(lines))

    def test_restart_dedup(self):
        path = self.root / "daemon.log"
        path.write_text("\n".join(
            f"[2026-10-06 15:36:0{index}] 模块版本: ChiRi A01-08 (versionCode 30108) | SoC 8550 | kernel 5.15"
            for index in range(2)))
        self.assertNotIn("混装", "\n".join(dvstatus.daemon_report([str(path)])))

    def test_real_version_change(self):
        path = self.root / "daemon.log"
        path.write_text("模块版本: ChiRi A01-06 (versionCode 30106) | SoC 8550\n"
                        "模块版本: ChiRi A01-08 (versionCode 30108) | SoC 8550\n")
        self.assertIn("多值", "\n".join(dvstatus.daemon_report([str(path)])))

    def test_same_baseline_for_model_increment(self):
        self.write_status()
        report = dvenergy.build(str(self.root))
        self.assertIn("baseline_cpu_model", report)
        self.assertIn("未解释", report)
        self.assertNotIn("CPU 功耗的下界", report)

    def test_regression_controls_and_scope(self):
        self.write_status()
        report = dvenergy.build(str(self.root))
        self.assertIn("batch|screen|mode|package", report)
        self.assertIn("非最新批次", report)
        self.assertIn("sample-equivalent Wh", report)

    def test_sampling_midnight_and_gaps(self):
        rows = [{"timestamp": stamp} for stamp in
                ("23:59:59.000", "00:00:01.000", "00:00:21.000")]
        self.assertTrue(hasattr(dc, "sampling_intervals"))
        intervals, summary = dc.sampling_intervals(rows)
        self.assertEqual(intervals, [2.0, 0.0, 0.0])
        self.assertEqual(summary["valid_dt"], 2)
        self.assertEqual(summary["gaps"], 1)
        self.assertEqual(summary["max_gap"], 20)
        self.assertAlmostEqual(summary["coverage"], 2 / 22)

    def test_timestamp_reversal_not_midnight(self):
        self.assertTrue(hasattr(dc, "sampling_intervals"))
        intervals, summary = dc.sampling_intervals([
            {"timestamp": "12:00:02.000"}, {"timestamp": "12:00:01.000"}])
        self.assertEqual(sum(intervals), 0)
        self.assertEqual(summary["invalid_dt"], 1)

    def test_thermal_does_not_claim_unconditional_exemption(self):
        report = "\n".join(dvmain.thermal([], dc.C44))
        self.assertIn("clamp_heavy", report)
        self.assertNotIn("`>= free_above` 不钳制", report)

    def test_numeric_baseline_closes_constant_residual(self):
        path = self.write_status()
        records = dvenergy.load(str(path))[0]
        result = dvenergy.account_scene(records)
        self.assertAlmostEqual(result["baseline"][0], 2.15)
        self.assertAlmostEqual(result["baseline"][1], 1.15)
        self.assertAlmostEqual(result["sample_increment"], result["sample_model_increment"])
        self.assertAlmostEqual(result["integrated_increment"], result["integrated_model_increment"])
        self.assertAlmostEqual(result["integrated_total"], sum(2 + index / 10 for index in range(29)) / 3600)
        self.assertEqual(result["valid_dt"], 29)

    def test_negative_residual_is_not_clamped(self):
        result = dvenergy.account_scene([
            dict(batt_power_w="2", cpu_dyn_w="3", resid_w="-1", _dt=2)])
        self.assertEqual(result["negative"], 1)
        self.assertAlmostEqual(result["integrated_total"] - result["integrated_model"], -2 / 3600)

    def test_short_intervals_keep_actual_duration(self):
        intervals, summary = dc.sampling_intervals([
            {"timestamp": stamp} for stamp in ("12:00:00.000", "12:00:00.500", "12:00:03.000")])
        self.assertEqual(intervals, [.5, 2.5, 0])
        self.assertEqual(summary["coverage"], 1)

    def test_ten_second_boundary_is_a_gap(self):
        intervals, summary = dc.sampling_intervals([
            {"timestamp": "12:00:00.000"}, {"timestamp": "12:00:10.000"}])
        self.assertEqual(intervals, [0, 0])
        self.assertEqual(summary["gaps"], 1)

    def test_all_identity_fields_are_parsed(self):
        identity = dc.parse_daemon_version(
            "模块版本: ChiRi A01-08 (versionCode 30108) | SoC 8550 | model PHB110 | Android 16 | kernel 5.15")
        self.assertEqual(identity, dict(module="ChiRi A01-08", versionCode="30108", soc="8550",
                                        model="PHB110", android="16", kernel="5.15"))

    def test_missing_identity_does_not_create_conflict(self):
        path = self.root / "daemon.log"
        path.write_text("模块版本: ChiRi A01-08 (versionCode 30108) | SoC 8550\n"
                        "模块版本: ChiRi A01-08 (versionCode 30108) | SoC 8550 | kernel 5.15\n")
        self.assertNotIn("结构化身份多值", "\n".join(dvstatus.daemon_report([str(path)])))

    def test_nonfinite_values_are_missing(self):
        for value in ("nan", "inf", "-inf", "-"):
            self.assertIsNone(dc.num(value))

    def test_since_rejects_unstamped_file(self):
        path = self.write_status("unstamped")
        with self.assertRaises(ValueError):
            dc.status_files(str(path), "1006-000000")

    def test_since_rejects_invalid_format(self):
        with self.assertRaises(ValueError):
            dc.status_files(str(self.root), "yesterday")

    def test_direct_cli_out_does_not_modify_source(self):
        path = self.write_status()
        original = path.read_bytes()
        destination = self.root / "output"
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(dvenergy.main([str(path), "--out", str(destination)]), 0)
            self.assertEqual(dvstatus.main([str(path), "--out", str(destination)]), 0)
            self.assertEqual(dvpower.main([str(path), "--out", str(destination)]), 0)
        self.assertEqual(original, path.read_bytes())
        self.assertTrue((destination / "energy.txt").is_file())

    def test_dvrun_status_energy_since(self):
        self.write_status("x_1005-000000")
        self.write_status("x_1006-180717")
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(dvrun.main([str(self.root), "--only", "status,energy",
                                        "--since", "1006-000000"]), 0)
        self.assertNotIn("x_1005-000000", (self.root / "energy.txt").read_text())
        self.assertIn("数据行: 30", (self.root / "status.txt").read_text())

    def test_power_sample_and_integrated_units(self):
        path = self.write_status()
        lines, summary = dvpower.status_report([str(path)], dvpower.read_status([str(path)]))
        self.assertIn("sample-equivalent Wh", "\n".join(lines))
        self.assertAlmostEqual(summary["total_wh"], sum(2 + index / 10 for index in range(30)) / 3600)
        self.assertAlmostEqual(summary["integrated_wh"], sum(2 + index / 10 for index in range(29)) / 3600)
        self.assertEqual(summary["valid_power_dt"], 29)

    def test_sampling_boundary_clears_left_dt(self):
        def window(record):
            return (record.get("mode"), record.get("charge"))

        records = [
            {"timestamp": "00:00:00.000", "mode": "fas", "charge": "discharging"},
            {"timestamp": "00:00:02.000", "mode": "fas", "charge": "discharging"},
            {"timestamp": "00:00:04.000", "mode": "default", "charge": "discharging"},
            {"timestamp": "00:00:06.000", "mode": "default", "charge": "discharging"},
        ]
        intervals, summary = dc.sampling_intervals(records, boundary_key=window)
        self.assertEqual(intervals, [2.0, 0.0, 2.0, 0.0])
        self.assertEqual(summary["valid_dt"], 4.0)
        self.assertEqual(summary["boundary_dt"], 2.0)
        self.assertEqual(summary["span"], 6.0)
        self.assertAlmostEqual(summary["coverage"], 1.0)

    def split_rows(self, middle):
        rows = [
            dict(timestamp="00:00:00.000", type="snap", mode="fas", package="game",
                 charge="discharging", screen_prop="2", batt_power_w="2", cpu_dyn_w="1", resid_w="1"),
            dict(timestamp="00:00:01.000", type="snap", mode="fas", package="game",
                 charge="discharging", screen_prop="2", batt_power_w="2", cpu_dyn_w="1", resid_w="1"),
        ]
        rows.append(middle)
        rows.append(dict(timestamp="00:00:03.000", type="snap", mode="fas", package="game",
                         charge="discharging", screen_prop="2", batt_power_w="2",
                         cpu_dyn_w="1", resid_w="1"))
        rows.append(dict(timestamp="00:00:04.000", type="snap", mode="fas", package="game",
                         charge="discharging", screen_prop="2", batt_power_w="2",
                         cpu_dyn_w="1", resid_w="1"))
        return rows

    def test_energy_integration_not_swallowed_across_charge(self):
        charging = dict(timestamp="00:00:02.000", type="snap", mode="fas", package="game",
                        charge="charging", screen_prop="2", batt_power_w="2",
                        cpu_dyn_w="1", resid_w="1")
        path = self.write_rows("x_1006-180717", self.split_rows(charging))
        rows = dvenergy.load(str(path))[0]
        self.assertAlmostEqual(sum(record["_dt"] for record in rows), 2.0)
        self.assertAlmostEqual(
            sum(dc.num(record["batt_power_w"]) * record["_dt"] for record in rows) / 3600, 4 / 3600)

    def test_charge_transition_splits_segments(self):
        charging = dict(timestamp="00:00:02.000", type="snap", mode="fas", package="game",
                        charge="charging", screen_prop="2", batt_power_w="2",
                        cpu_dyn_w="1", resid_w="1")
        path = self.write_rows("x_1006-180717", self.split_rows(charging))
        rows = dvenergy.load(str(path))[0]
        self.assertEqual(len({record["_segment"] for record in rows}), 2)

    def test_invalid_data_transition_splits_segments(self):
        invalid = dict(timestamp="00:00:02.000", type="tick", mode="fas", package="game",
                       charge="discharging", screen_prop="2", batt_power_w="2",
                       cpu_dyn_w="1", resid_w="1")
        path = self.write_rows("x_1006-180717", self.split_rows(invalid))
        rows = dvenergy.load(str(path))[0]
        self.assertEqual(len({record["_segment"] for record in rows}), 2)

    def test_continuous_run_stays_one_segment(self):
        middle = dict(timestamp="00:00:02.000", type="snap", mode="fas", package="game",
                      charge="discharging", screen_prop="2", batt_power_w="2",
                      cpu_dyn_w="1", resid_w="1")
        path = self.write_rows("x_1006-180717", self.split_rows(middle))
        rows = dvenergy.load(str(path))[0]
        self.assertEqual(len({record["_segment"] for record in rows}), 1)

    def test_placeholder_identity_is_not_a_value(self):
        identity = dc.parse_daemon_version(
            "模块版本: ChiRi A01-08 (versionCode 30108) | SoC - | kernel unknown")
        self.assertEqual(identity, {"module": "ChiRi A01-08", "versionCode": "30108"})

    def test_placeholder_identity_does_not_conflict(self):
        path = self.root / "daemon.log"
        path.write_text("模块版本: ChiRi A01-08 (versionCode 30108) | SoC -\n"
                        "模块版本: ChiRi A01-08 (versionCode 30108) | SoC 8550\n")
        self.assertNotIn("结构化身份多值", "\n".join(dvstatus.daemon_report([str(path)])))

    def test_run_stage_isolates_system_exit(self):
        def aborting():
            raise SystemExit(2)

        ok, lines, error, duration = dvrun.run_stage("stage", aborting)
        self.assertFalse(ok)
        self.assertIn("2", error)

    def test_run_stage_propagates_keyboard_interrupt(self):
        def interrupted():
            raise KeyboardInterrupt

        with self.assertRaises(KeyboardInterrupt):
            dvrun.run_stage("stage", interrupted)


class ExistingArchiveChecks(unittest.TestCase):
    @unittest.skipUnless(os.environ.get("DEVIMP_VERIFY_EXISTING"), "optional read-only archive verification")
    def test_four_existing_status_energy_reports(self):
        repository = Path(__file__).resolve().parents[2]
        for tag in ("1005-170643", "1006-123730", "1006-131110", "1006-180755"):
            root = repository / "devimpbin" / tag
            paths = dc.status_files(str(root))
            self.assertTrue(paths, tag)
            daemon_paths = [str(Path(path).with_name("daemon.log")) for path in paths
                            if Path(path).with_name("daemon.log").is_file()]
            daemon_report = "\n".join(dvstatus.daemon_report(daemon_paths))
            mixed = "结构化身份多值" in daemon_report
            self.assertEqual(mixed, tag == "1006-123730")
            energy = dvenergy.build(str(root))
            self.assertIn("sample-equivalent Wh", energy)
            latest = paths[-1]
            _, records = dc.read_status_file(latest)
            print(f"VERIFY {tag} batches={len(paths)} mixed={mixed}")
            print(dc.sampling_report(latest, records))
            latest_energy = dvenergy.build(latest)
            for line in latest_energy.splitlines():
                if line.startswith("# negative") or "分解有效比例" in line:
                    print(line)
            latest_rows = dvenergy.load(latest)[0]
            print("total sample-equivalent Wh=%.6f integrated Wh=%.6f" % (
                sum(dc.num(record["batt_power_w"]) for record in latest_rows) / 3600,
                sum(dc.num(record["batt_power_w"]) * record["_dt"] for record in latest_rows) / 3600))
            lines, summary = dvstatus.status_report([latest])
            self.assertEqual(summary["rows"], len(records))


if __name__ == "__main__":
    unittest.main()
