import runpy
from pathlib import Path
import tomllib
import unittest

ROOT = Path(__file__).resolve().parent.parent
RUNNER = runpy.run_path(ROOT / "scripts/fuzz_smoke.py")
VERIFY = RUNNER["verify"]
COMPLETE = "#10000 DONE cov: 100\nstat::number_of_executed_units: 10000\nstat::peak_rss_mb: 81\n"


class CampaignEvidence(unittest.TestCase):
    def test_campaign_covers_every_declared_target_with_curated_seeds(self):
        manifest = tomllib.loads((ROOT / "fuzz/Cargo.toml").read_text())
        self.assertEqual(set(RUNNER["TARGETS"]), {target["name"] for target in manifest["bin"]})
        for target in RUNNER["TARGETS"]:
            self.assertTrue(any((ROOT / "fuzz/seeds" / target).iterdir()), target)

    def test_complete_iterations_preserve_statistics(self):
        self.assertEqual(VERIFY(COMPLETE, 0, 10000), {"number_of_executed_units": 10000, "peak_rss_mb": 81})

    def test_crashes_never_pass_even_with_completion_text(self):
        with self.assertRaises(ValueError):
            VERIFY(COMPLETE, 77, 10000)

    def test_early_time_limit_cannot_shrink_the_promised_budget(self):
        with self.assertRaises(ValueError):
            VERIFY(COMPLETE.replace("10000", "9999"), 0, 10000)

    def test_missing_statistics_or_completion_never_pass(self):
        for log in ("", "stat::number_of_executed_units: 10000\n", "#10000 DONE\n"):
            with self.subTest(log=log), self.assertRaises(ValueError):
                VERIFY(log, 0, 10000)


if __name__ == "__main__":
    unittest.main()
