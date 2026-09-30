import copy
import json
import os
from pathlib import Path
import runpy
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parent.parent
RUNNER = runpy.run_path(ROOT / "scripts/performance.py")
CASE = {"id": "parse", "units": 10_000, "checksum": 10_000}
SAMPLE = {"timeout": False, "returncode": 0, "process_elapsed_ns": 200, "peak_rss_kib": 4096}
RESULT = {"schema": 1, "workload": "parse", "units": 10_000, "checksum": 10_000, "elapsed_ns": 100}


class PerformanceEvidence(unittest.TestCase):
    def test_registered_full_sizes_and_generated_payload_are_exact(self):
        cases = {case["id"]: case for case in RUNNER["workloads"]()}
        self.assertEqual({name: case["units"] for name, case in cases.items()},
                         {"cli-startup": 1, "parse": 10_000, "calls": 100_000,
                          "loop": 1_000_000, "source-io": 16, "waiting": 100})
        self.assertEqual(cases["parse"]["source"].count("\n"), 10_000)
        self.assertEqual(len(cases["source-io"]["source"].encode()), 256 * 1024)
        self.assertEqual(cases["waiting"]["checksum"], 4950)
        self.assertEqual({case["id"] for case in RUNNER["workloads"](True)}, set(cases))

    def test_success_requires_identity_size_and_correct_result(self):
        self.assertEqual(RUNNER["verify"](SAMPLE, CASE, json.dumps(RESULT), ""), 100)
        for key, value in [("schema", True), ("workload", "loop"), ("units", 1), ("checksum", 1)]:
            result = dict(RESULT, **{key: value})
            with self.subTest(key=key), self.assertRaises(ValueError):
                RUNNER["verify"](SAMPLE, CASE, json.dumps(result), "")

    def test_cli_output_and_process_failure_cannot_pass(self):
        cli = {"id": "cli-startup"}
        self.assertEqual(RUNNER["verify"](SAMPLE, cli, "42\n", ""), 200)
        for stdout, stderr in [("", ""), ("42\nextra\n", ""), ("42\n", "failure")]:
            with self.subTest(stdout=stdout, stderr=stderr), self.assertRaises(ValueError):
                RUNNER["verify"](SAMPLE, cli, stdout, stderr)
        for change in [{"returncode": 1}, {"timeout": True}, {"peak_rss_kib": 0}]:
            with self.subTest(change=change), self.assertRaises(ValueError):
                RUNNER["verify"](dict(SAMPLE, **change), CASE, json.dumps(RESULT), "")

    def test_invalid_or_impossible_workload_durations_are_rejected(self):
        for elapsed in [0, -1, 201, True, 1.5, float("nan")]:
            with self.subTest(elapsed=elapsed), self.assertRaises(ValueError):
                RUNNER["verify"](SAMPLE, CASE, json.dumps(dict(RESULT, elapsed_ns=elapsed)), "")

    def test_nearest_rank_percentiles_keep_outliers(self):
        values = list(range(30, 0, -1))
        self.assertEqual(RUNNER["statistics"](values),
                         {"count": 30, "min": 1, "p50": 15, "p95": 29, "max": 30})
        for invalid in [[], [0], [-1], [float("inf")], [float("nan")], [True]]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                RUNNER["statistics"](invalid)

    def test_aggregation_requires_all_verified_samples_and_keeps_warmups_separate(self):
        rows = [dict(SAMPLE, phase=phase, index=index, workload="parse", verified=True,
                     workload_elapsed_ns=elapsed) for phase, index, elapsed in
                [("warmup", 0, 1000), ("measured", 0, 10), ("measured", 1, 20)]]
        aggregate = RUNNER["aggregate"]
        self.assertEqual(aggregate(rows, [CASE], 2, 1)["parse"]["workload_elapsed_ns"]["max"], 20)
        bad = copy.deepcopy(rows)
        bad[0]["verified"] = False
        for invalid in [rows[:-1], [*rows, rows[0]], bad]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                aggregate(invalid, [CASE], 2, 1)

    @unittest.skipUnless(sys.platform == "linux", "runner uses Linux wait4/pidfds")
    def test_watchdog_reaps_a_stalled_child_and_does_not_report_success(self):
        with tempfile.TemporaryDirectory() as directory:
            sample = RUNNER["execute"]([sys.executable, "-I", "-S", "-c", "import time; time.sleep(30)"],
                                       Path(directory), Path(directory) / "timeout", 0.05)
        self.assertTrue(sample["timeout"])
        self.assertNotEqual(sample["returncode"], 0)

    @unittest.skipUnless(sys.platform == "linux", "runner uses Linux process groups")
    def test_watchdog_also_stops_build_style_descendants(self):
        with tempfile.TemporaryDirectory() as directory:
            code = "import subprocess, sys, time; child = subprocess.Popen([sys.executable, '-I', '-S', '-c', 'import time; time.sleep(30)']); print(child.pid, flush=True); time.sleep(30)"
            sample = RUNNER["execute"]([sys.executable, "-I", "-S", "-c", code],
                                       Path(directory), Path(directory) / "tree", 1)
            self.assertTrue(sample["timeout"])
            child = int(Path(sample["stdout"]).read_text().strip())
            deadline = time.monotonic() + 5
            while True:
                try:
                    state = Path(f"/proc/{child}/stat").read_text().rsplit(")", 1)[1].split()[0]
                except FileNotFoundError:
                    break
                if state == "Z":
                    break  # Dead; the system reaper owns this orphan's wait status.
                if time.monotonic() > deadline:
                    os.kill(child, 9)
                    self.fail("descendant survived the command watchdog")
                time.sleep(0.01)



class RegisteredBudgets(unittest.TestCase):
    """Roadmap decision D4's budgets and the 10% regression gate of item 249."""

    def setUp(self):
        self.baseline = json.loads((ROOT / "docs/performance-evidence.json").read_text())
        self.budgets = json.loads((ROOT / "benches/runtime/budgets.json").read_text())

    def changed(self, workload, metric, factor):
        campaign = copy.deepcopy(self.baseline)
        statistics = campaign["statistics"][workload]
        if metric == "p95":
            statistics["workload_elapsed_ns"]["p95"] = round(statistics["workload_elapsed_ns"]["p95"] * factor)
        else:
            statistics["peak_rss_kib"]["max"] = round(statistics["peak_rss_kib"]["max"] * factor)
        return campaign

    def test_budgets_are_the_accepted_baseline_times_a_quarter(self):
        self.assertEqual(self.budgets, RUNNER["budgets_from"](self.baseline))
        self.assertEqual(self.budgets["workloads"]["cli-startup"]["p95_budget_ns"], 2_025_325)
        self.assertEqual(self.budgets["workloads"]["parse"]["peak_rss_budget_kib"], 16_180)
        self.assertEqual(set(self.budgets["workloads"]), {case["id"] for case in RUNNER["workloads"]()})
        self.assertEqual(self.budgets["host"]["cpus"], 8)

    def test_the_baseline_passes_its_own_budgets(self):
        self.assertEqual(RUNNER["check"](self.baseline, self.budgets), [])

    def test_exceeding_a_budget_fails_even_when_explained(self):
        problems = RUNNER["check"](self.changed("loop", "p95", 1.3), self.budgets, {"loop": "slower"})
        self.assertEqual(len(problems), 1)
        self.assertIn("loop: p95", problems[0])
        self.assertIn("exceeds its budget", problems[0])
        problems = RUNNER["check"](self.changed("parse", "memory", 1.3), self.budgets)
        self.assertTrue(any("parse: peak memory" in problem and "exceeds" in problem for problem in problems))

    def test_regressions_beyond_ten_percent_need_an_explanation(self):
        within = self.changed("calls", "p95", 1.09)
        self.assertEqual(RUNNER["check"](within, self.budgets), [])
        regressed = self.changed("calls", "p95", 1.15)
        problems = RUNNER["check"](regressed, self.budgets)
        self.assertEqual(len(problems), 1)
        self.assertIn("calls: p95 regressed 15.0%", problems[0])
        self.assertEqual(RUNNER["check"](regressed, self.budgets, {"calls": "reason"}), [])
        memory = RUNNER["check"](self.changed("waiting", "memory", 1.12), self.budgets)
        self.assertTrue(any("waiting: peak memory regressed" in problem for problem in memory))

    def test_other_hosts_and_unfinished_campaigns_are_not_compared(self):
        for change in [
            lambda record: record["machine"].update(cpuinfo=record["machine"]["cpuinfo"].replace("9950X3D", "7950X")),
            lambda record: record["machine"].update(cpu_affinity=[0, 1]),
            lambda record: record.update(target="x86_64-unknown-linux-musl"),
            lambda record: record.update(profile="debug"),
        ]:
            campaign = copy.deepcopy(self.baseline)
            change(campaign)
            with self.subTest(host=RUNNER["host"](campaign)):
                problems = RUNNER["check"](campaign, self.budgets)
                self.assertEqual(len(problems), 1)
                self.assertIn("not comparable", problems[0])
        for change in [{"kind": "smoke"}, {"complete": False}]:
            with self.subTest(change=change):
                self.assertEqual(RUNNER["check"](dict(self.baseline, **change), self.budgets),
                                 ["not a complete measurement campaign"])
        missing = copy.deepcopy(self.baseline)
        del missing["statistics"]["waiting"]
        self.assertEqual(RUNNER["check"](missing, self.budgets), ["waiting: not measured"])

    def test_the_command_line_checks_a_summary_and_records_explanations(self):
        import subprocess
        script = str(ROOT / "scripts/performance.py")
        passed = subprocess.run([sys.executable, script, "--check", str(ROOT / "docs/performance-evidence.json")],
                                capture_output=True, text=True)
        self.assertEqual((passed.returncode, passed.stdout.strip()), (0, "budget check passed"))
        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / "summary.json"
            summary.write_text(json.dumps(self.changed("calls", "p95", 1.15)))
            failed = subprocess.run([sys.executable, script, "--check", str(summary)], capture_output=True, text=True)
            self.assertEqual(failed.returncode, 1)
            self.assertIn("budget check failed: calls: p95 regressed", failed.stdout)
            explained = subprocess.run([sys.executable, script, "--check", str(summary),
                                        "--explain", "calls=profiled: new cancellation checkpoint"],
                                       capture_output=True, text=True)
            self.assertEqual(explained.returncode, 0, explained.stdout)
            self.assertIn("explained regression: calls: profiled: new cancellation checkpoint", explained.stdout)
            bare = subprocess.run([sys.executable, script, "--check", str(summary), "--explain", "calls"],
                                  capture_output=True, text=True)
            self.assertEqual(bare.returncode, 2)


if __name__ == "__main__":
    unittest.main()
