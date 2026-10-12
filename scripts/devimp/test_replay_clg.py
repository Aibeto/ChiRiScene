"""test_replay_clg.py: [parse] [source] -- standard-library tests, no A/B."""
import io
import unittest
import replay_clg as replay

HEADER = "ts,type,mode,screen_on,cluster,max_util,cur_perf,decision,cur_freq_khz,thermal_cap_pct,touch\n"

class ReplayTests(unittest.TestCase):
    # [parse]
    def test_tick_unknown_screen_is_not_filtered_or_imputed(self):
        rows, counts = replay.parse_rows(io.StringIO(HEADER + "01:00:00.000,tick,default,-,big,0.75,0.4,up,1401600,100,0\n"))
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["screen_on"], "-")
        self.assertEqual(counts["unknown_screen"], 1)

    def test_only_default_valid_ticks_without_fas_invention(self):
        text = HEADER + "01:00:00.000,snap,fas,1,-,-,-,-,-,100,-\n" + "01:00:00.000,tick,reduce,-,big,0.5,0.4,hold,1401600,100,0\n" + "01:00:00.000,tick,default,-,big,NaN,0.4,up,1401600,100,0\n"
        rows, counts = replay.parse_rows(io.StringIO(text))
        self.assertEqual(rows, [])
        self.assertEqual(counts["invalid"], 1)
        self.assertEqual(counts["nondefault_ticks"], 1)
        self.assertEqual(counts["fas_ticks"], 0)

    def test_tick_duplicates_removed_but_distinct_clusters_retained(self):
        line = "01:00:00.000,tick,default,-,big,0.75,0.4,up,1401600,100,0\n"
        rows, counts = replay.parse_rows(io.StringIO(HEADER + line + line + line.replace(",big,", ",prime,")))
        self.assertEqual(len(rows), 2)
        self.assertEqual(counts["duplicates"], 1)

    def test_default_state_never_crosses_mode_or_long_gap(self):
        line = "01:00:00.000,tick,default,-,big,0.75,0.4,up,1401600,100,0\n"
        text = HEADER + line + line.replace(",default,", ",reduce,") + line.replace("01:00:00.000", "01:00:00.160") + line.replace("01:00:00.000", "01:00:10.000")
        rows, _ = replay.parse_rows(io.StringIO(text))
        self.assertEqual([row.get("_segment") for row in rows], [0, 1, 2])

    # [source]
    def test_source_uses_current_production_decision_verbatim(self):
        source = replay.build_source()
        self.assertIn("fn on_load_update(&mut self, core_utils: &[f32])", source)
        self.assertIn("let target = self.cfg.up_threshold * self.cfg.steady_decay_target_ratio;", source)
        self.assertIn("self.cluster.fast_down = util < self.cfg.down_fast_threshold;", source)
        self.assertNotIn("fn flush(", source)

if __name__ == "__main__":
    unittest.main()
