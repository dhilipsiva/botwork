import copy
import json
import os
from pathlib import Path
import platform
import runpy
import shutil
import subprocess
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

    def test_scaled_workloads_multiply_every_size_but_startup(self):
        cases = {case["id"]: case for case in RUNNER["workloads"](scale=2)}
        self.assertEqual({name: (case["units"], case["checksum"]) for name, case in cases.items()},
                         {"cli-startup": (1, 42), "parse": (20_000, 20_000), "calls": (200_000, 200_000),
                          "loop": (2_000_000, 2_000_000), "source-io": (32, 42 * 32), "waiting": (200, 19_900)})
        self.assertEqual(cases["parse"]["source"].count("\n"), 20_000)
        self.assertIn("While |answer < 200000|", cases["calls"]["source"])
        self.assertIn("While |answer < 2000000|", cases["loop"]["source"])
        # Each source load stays 256 KiB; the campaign loads twice as many.
        self.assertEqual(len(cases["source-io"]["source"].encode()), 256 * 1024)
        self.assertEqual(RUNNER["workloads"](scale=4)[3]["units"], 4_000_000)
        self.assertEqual(RUNNER["SCALES"], (1, 2, 4))

    def test_success_requires_identity_size_and_correct_result(self):
        self.assertEqual(RUNNER["verify"](SAMPLE, CASE, json.dumps(RESULT), ""), 100)
        for key, value in [("schema", True), ("schema", 2), ("workload", "loop"), ("units", 1), ("checksum", 1)]:
            result = dict(RESULT, **{key: value})
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                RUNNER["verify"](SAMPLE, CASE, json.dumps(result), "")
        with self.assertRaises(ValueError):
            RUNNER["verify"](SAMPLE, CASE, json.dumps(dict(RESULT, peak_heap_bytes=1)), "")

    def test_cli_output_and_process_failure_cannot_pass(self):
        cli = {"id": "cli-startup"}
        self.assertEqual(RUNNER["verify"](SAMPLE, cli, "42\n", ""), 200)
        for stdout, stderr in [("", ""), ("42\nextra\n", ""), ("42\n", "failure")]:
            with self.subTest(stdout=stdout, stderr=stderr), self.assertRaises(ValueError):
                RUNNER["verify"](SAMPLE, cli, stdout, stderr)
        for change in [{"returncode": 1}, {"timeout": True}, {"peak_rss_kib": 0}]:
            with self.subTest(change=change), self.assertRaises(ValueError):
                RUNNER["verify"](dict(SAMPLE, **change), CASE, json.dumps(RESULT), "")

    def test_the_heap_counter_must_report_a_positive_peak(self):
        # 161,256 bytes round up to 158 KiB.
        self.assertEqual(RUNNER["heap_kib"]("161256\n"), 158)
        self.assertEqual(RUNNER["heap_kib"]("1024\n"), 1)
        for report in [None, "", "0\n", "-1\n", "1.5\n", "12", "12\n13\n", " 12\n"]:
            with self.subTest(report=report), self.assertRaises(ValueError):
                RUNNER["heap_kib"](report)

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
                     workload_elapsed_ns=elapsed, heap_kib=elapsed // 10) for phase, index, elapsed in
                [("warmup", 0, 1000), ("measured", 0, 10), ("measured", 1, 20)]]
        aggregate = RUNNER["aggregate"]
        self.assertEqual(aggregate(rows, [CASE], 2, 1)["parse"]["workload_elapsed_ns"]["max"], 20)
        self.assertEqual(aggregate(rows, [CASE], 2, 1)["parse"]["heap_kib"]["max"], 2)
        # Static builds have no counted runs; a workload counts every observation or none.
        uncounted = copy.deepcopy(rows)
        for row in uncounted:
            del row["heap_kib"]
        self.assertNotIn("heap_kib", aggregate(uncounted, [CASE], 2, 1)["parse"])
        partly = copy.deepcopy(rows)
        del partly[1]["heap_kib"]
        with self.assertRaises(ValueError):
            aggregate(partly, [CASE], 2, 1)
        # Paired baseline runs likewise: all or none.
        paired = [dict(row, paired_elapsed_ns=row["workload_elapsed_ns"]) for row in rows]
        self.assertEqual(aggregate(paired, [CASE], 2, 1)["parse"]["paired_elapsed_ns"]["max"], 20)
        del paired[2]["paired_elapsed_ns"]
        with self.assertRaises(ValueError):
            aggregate(paired, [CASE], 2, 1)
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


    @unittest.skipUnless(sys.platform == "linux" and shutil.which("cc") and platform.libc_ver()[0] == "glibc",
                         "the heap counter preloads into glibc processes")
    def test_heap_counter_observes_the_childs_own_allocations(self):
        with tempfile.TemporaryDirectory() as directory:
            counter, report = Path(directory) / "heap.so", Path(directory) / "heap"
            subprocess.run(["cc", "-shared", "-fPIC", "-O2", "-Wall", "-Wextra", "-Werror", "-o", str(counter),
                            str(ROOT / "benches/runtime/heap.c")], check=True)

            def heap(code, **env):
                report.unlink(missing_ok=True)
                subprocess.run([sys.executable, "-I", "-S", "-c", code], check=True, stdout=subprocess.DEVNULL,
                               env=dict(os.environ, LD_PRELOAD=str(counter), **env))
                return int(report.read_text()) if report.exists() else None

            small = heap("print(42)", BOTWORK_HEAP_REPORT=str(report))
            large = heap("data = bytearray(32 * 1024 * 1024); data = None; print(42)", BOTWORK_HEAP_REPORT=str(report))
            # The allocation alone makes a 32 MiB peak. Startup's transient
            # allocations need not be live alongside it, so the difference
            # from the small child can fall slightly short of 32 MiB.
            self.assertLess(0, small)
            self.assertLessEqual(32 * 1024 * 1024, large)
            self.assertLess(small + 31 * 1024 * 1024, large)
            # Freeing a block releases it: two 32 MiB blocks held one after
            # the other peak at one block, not at the total allocated.
            again = heap("data = bytearray(32 * 1024 * 1024); data = None; data = bytearray(32 * 1024 * 1024); print(42)",
                         BOTWORK_HEAP_REPORT=str(report))
            self.assertLess(abs(again - large), 64 * 1024)
            # An in-place repeat grows one block with realloc, from 1 MiB to 33 MiB.
            grown = heap("data = bytearray(1024 * 1024); data *= 33; print(42)", BOTWORK_HEAP_REPORT=str(report))
            self.assertLessEqual(33 * 1024 * 1024, grown)
            self.assertIsNone(heap("print(42)"))


class RegisteredBudgets(unittest.TestCase):
    """Roadmap decision D4's budgets and the 10% regression gate of item 249."""

    def setUp(self):
        self.baseline = json.loads((ROOT / "docs/performance-baseline-evidence.json").read_text())
        self.budgets = json.loads((ROOT / "benches/runtime/budgets.json").read_text())
        # The baseline as a campaign whose paired runs of the baseline's source
        # measured exactly what it did.
        self.paired = copy.deepcopy(self.baseline)
        self.paired["paired"] = {"inputs_sha256": RUNNER["inputs_digest"](self.baseline["input_sha256"])}
        for statistics in self.paired["statistics"].values():
            statistics["paired_elapsed_ns"] = dict(statistics["workload_elapsed_ns"])

    def changed(self, workload, metric, factor):
        campaign = copy.deepcopy(self.paired)
        if workload == "binary":
            binary = campaign["binaries"]["botwork"]
            binary["bytes"] = round(binary["bytes"] * factor)
            return campaign
        statistics = campaign["statistics"][workload]
        key, statistic = {"p95": ("workload_elapsed_ns", "p95"), "p50": ("workload_elapsed_ns", "p50"),
                          "heap": ("heap_kib", "max"),
                          "rss": ("peak_rss_kib", "max")}[metric]
        statistics[key][statistic] = round(statistics[key][statistic] * factor)
        return campaign

    def test_budgets_are_the_accepted_baseline_times_a_quarter(self):
        self.assertEqual(self.budgets, RUNNER["budgets_from"](self.baseline))
        self.assertEqual((self.budgets["schema"], self.budgets["protocol"]), (3, 2))
        # Paired runs build the committed source of the baseline's fingerprinted inputs.
        self.assertEqual(self.budgets["baseline"]["source_revision"], "b49116c285408aceb80a811bebd30a63fb15eb58")
        self.assertEqual(self.budgets["baseline"]["inputs_sha256"],
                         RUNNER["inputs_digest"](self.baseline["input_sha256"]))
        self.assertEqual(self.budgets["workloads"]["cli-startup"]["p95_budget_ns"], 3_954_744)
        self.assertEqual(self.budgets["workloads"]["parse"]["heap_budget_kib"], 18_135)
        self.assertEqual(self.budgets["binary"]["budget_bytes"], 13_157_170)
        self.assertEqual(set(self.budgets["workloads"]), {case["id"] for case in RUNNER["workloads"]()})
        self.assertEqual(self.budgets["host"], {"cpu": "AMD Ryzen 9 9950X3D 16-Core Processor", "cpus": 8,
                                                "target": "x86_64-unknown-linux-gnu", "profile": "dist"})
        with self.assertRaises(ValueError):
            RUNNER["budgets_from"](dict(self.baseline, schema=1))

    def test_the_baseline_passes_its_own_budgets(self):
        self.assertEqual(RUNNER["check"](self.paired, self.budgets), [])
        # Without paired runs, time regressions cannot be judged under this host's load.
        unpaired = RUNNER["check"](self.baseline, self.budgets)
        self.assertEqual(len(unpaired), 6)
        self.assertTrue(all("median time regression not checked: no paired baseline runs" in problem
                            for problem in unpaired))
        other = copy.deepcopy(self.paired)
        other["paired"]["inputs_sha256"] = "0" * 64
        self.assertIn("paired runs did not build the baseline's source", RUNNER["check"](other, self.budgets))

    @unittest.skipUnless(subprocess.run(["git", "cat-file", "-e", "b49116c285408aceb80a811bebd30a63fb15eb58^{commit}"],
                                        cwd=ROOT, capture_output=True).returncode == 0,
                         "needs the baseline revision in the history (CI checks out one commit)")
    def test_paired_runs_build_the_baseline_revision_checked_by_fingerprint(self):
        # runpy returns a copy of the script's globals; patch the ones its
        # functions use, so the test never touches the real cache.
        script = RUNNER["baseline_tree"].__globals__
        with tempfile.TemporaryDirectory() as directory:
            cache = script["PAIRED_CACHE"]
            script["PAIRED_CACHE"] = Path(directory)
            try:
                tree = RUNNER["baseline_tree"](self.budgets)
                self.assertTrue(str(tree).startswith(directory))  # Before tampering with it.
                self.assertEqual(tree.name, "baseline-" + self.budgets["baseline"]["source_revision"])
                self.assertEqual(RUNNER["fingerprints"](tree), self.baseline["input_sha256"])
                self.assertEqual(RUNNER["baseline_tree"](self.budgets), tree)  # Reused once extracted.
                with (tree / "src/lib.rs").open("a") as source:
                    source.write("\n")
                with self.assertRaises(ValueError):
                    RUNNER["baseline_tree"](self.budgets)
            finally:
                script["PAIRED_CACHE"] = cache

    def test_time_regressions_compare_with_paired_runs_under_the_same_load(self):
        # A busier machine slows both builds: 15% over the recorded baseline, as
        # much as the paired runs, is no regression.
        loaded = copy.deepcopy(self.paired)
        for statistics in loaded["statistics"].values():
            for key in ("workload_elapsed_ns", "paired_elapsed_ns"):
                statistics[key]["p50"] = round(statistics[key]["p50"] * 1.15)
        self.assertEqual(RUNNER["check"](loaded, self.budgets), [])
        # A slower build is a regression even when the recorded baseline was slower still.
        quiet = copy.deepcopy(self.paired)
        statistics = quiet["statistics"]["parse"]
        statistics["paired_elapsed_ns"]["p50"] = round(statistics["paired_elapsed_ns"]["p50"] * 0.8)
        problems = RUNNER["check"](quiet, self.budgets)
        self.assertEqual(len(problems), 1)
        self.assertIn("parse: median time regressed 25.0% against the paired baseline runs", problems[0])

    def test_p95_tail_noise_is_left_to_the_budget(self):
        # Even paired, the p95 of 30 samples swings past 10% for identical code;
        # within its budget, a higher p95 with a matching median passes.
        noisy = self.changed("loop", "p95", 1.15)
        self.assertEqual(RUNNER["check"](noisy, self.budgets), [])
        over = RUNNER["check"](self.changed("loop", "p95", 1.3), self.budgets)
        self.assertEqual(len(over), 1)
        self.assertIn("loop: p95", over[0])
        self.assertIn("exceeds its budget", over[0])

    def test_exceeding_a_budget_fails_even_when_explained(self):
        problems = RUNNER["check"](self.changed("loop", "p95", 1.3), self.budgets, {"loop": "slower"})
        self.assertEqual(len(problems), 1)
        self.assertIn("loop: p95", problems[0])
        self.assertIn("exceeds its budget", problems[0])
        problems = RUNNER["check"](self.changed("parse", "heap", 1.3), self.budgets)
        self.assertTrue(any("parse: peak heap" in problem and "exceeds" in problem for problem in problems))
        problems = RUNNER["check"](self.changed("binary", "size", 1.3), self.budgets, {"binary": "new adapter"})
        self.assertEqual(len(problems), 1)
        self.assertIn("binary: size", problems[0])
        self.assertIn("exceeds its budget", problems[0])

    def test_regressions_beyond_ten_percent_need_an_explanation(self):
        within = self.changed("calls", "p50", 1.09)
        self.assertEqual(RUNNER["check"](within, self.budgets), [])
        regressed = self.changed("calls", "p50", 1.15)
        problems = RUNNER["check"](regressed, self.budgets)
        self.assertEqual(len(problems), 1)
        self.assertIn("calls: median time regressed 15.0%", problems[0])
        self.assertEqual(RUNNER["check"](regressed, self.budgets, {"calls": "reason"}), [])
        memory = RUNNER["check"](self.changed("waiting", "heap", 1.12), self.budgets)
        self.assertTrue(any("waiting: peak heap regressed" in problem for problem in memory))
        binary = RUNNER["check"](self.changed("binary", "size", 1.12), self.budgets)
        self.assertEqual(len(binary), 1)
        self.assertIn("binary: size regressed", binary[0])
        self.assertEqual(RUNNER["check"](self.changed("binary", "size", 1.12), self.budgets, {"binary": "reason"}), [])

    def test_peak_rss_is_recorded_but_not_budgeted(self):
        # Resident memory mixes the binary's pages and the heap, which have their own budgets.
        self.assertEqual(RUNNER["check"](self.changed("parse", "rss", 2), self.budgets), [])

    def test_other_hosts_and_unfinished_campaigns_are_not_compared(self):
        for change in [
            lambda record: record["machine"].update(cpuinfo=record["machine"]["cpuinfo"].replace("9950X3D", "7950X")),
            lambda record: record["machine"].update(cpu_affinity=[0, 1]),
            lambda record: record.update(target="x86_64-unknown-linux-musl"),
            lambda record: record.update(profile="release"),
            lambda record: record.update(schema=1),
        ]:
            campaign = copy.deepcopy(self.baseline)
            change(campaign)
            with self.subTest(host=RUNNER["host"](campaign), schema=campaign["schema"]):
                problems = RUNNER["check"](campaign, self.budgets)
                self.assertEqual(len(problems), 1)
                self.assertIn("not comparable", problems[0])
        # The protocol 1 baseline measured whole-process memory of a release build.
        earlier = json.loads((ROOT / "docs/performance-evidence.json").read_text())
        self.assertEqual(RUNNER["check"](earlier, self.budgets),
                         ["not comparable: protocol 1, budgets use protocol 2"])
        for change in [{"kind": "smoke"}, {"complete": False}]:
            with self.subTest(change=change):
                self.assertEqual(RUNNER["check"](dict(self.baseline, **change), self.budgets),
                                 ["not a complete measurement campaign"])
        missing = copy.deepcopy(self.paired)
        del missing["statistics"]["waiting"]
        self.assertEqual(RUNNER["check"](missing, self.budgets), ["waiting: not measured"])
        uncounted = copy.deepcopy(self.paired)
        del uncounted["statistics"]["cli-startup"]["heap_kib"]
        self.assertEqual(RUNNER["check"](uncounted, self.budgets), ["cli-startup: peak heap not measured"])

    def test_a_campaign_gates_only_what_it_can_be_compared_with(self):
        gate = RUNNER["gate"]
        self.assertEqual(gate(self.paired, self.budgets), ("passed", []))
        verdict, problems = gate(self.changed("loop", "p95", 1.3), self.budgets)
        self.assertEqual(verdict, "failed")
        self.assertTrue(any("loop: p95" in problem for problem in problems))
        for change in [dict(target="x86_64-unknown-linux-musl"), dict(scale=2), dict(profile="release")]:
            with self.subTest(change=change):
                verdict, problems = gate(dict(self.paired, **change), self.budgets)
                self.assertEqual(verdict, "not comparable")
        self.assertEqual(RUNNER["check"](dict(self.paired, scale=2), self.budgets),
                         ["not comparable: scale 2, budgets apply at scale 1"])

    def test_the_command_line_checks_a_summary_and_records_explanations(self):
        script = str(ROOT / "scripts/performance.py")
        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / "summary.json"
            summary.write_text(json.dumps(self.paired))
            passed = subprocess.run([sys.executable, script, "--check", str(summary)], capture_output=True, text=True)
            self.assertEqual((passed.returncode, passed.stdout.strip()), (0, "budget check passed"))
            summary.write_text(json.dumps(self.changed("calls", "p50", 1.15)))
            failed = subprocess.run([sys.executable, script, "--check", str(summary)], capture_output=True, text=True)
            self.assertEqual(failed.returncode, 1)
            self.assertIn("budget check failed: calls: median time regressed", failed.stdout)
            explained = subprocess.run([sys.executable, script, "--check", str(summary),
                                        "--explain", "calls=profiled: new cancellation checkpoint"],
                                       capture_output=True, text=True)
            self.assertEqual(explained.returncode, 0, explained.stdout)
            self.assertIn("explained regression: calls: profiled: new cancellation checkpoint", explained.stdout)
            bare = subprocess.run([sys.executable, script, "--check", str(summary), "--explain", "calls"],
                                  capture_output=True, text=True)
            self.assertEqual(bare.returncode, 2)


class Scaling(unittest.TestCase):
    """Item 249: doubling a workload's size or concurrency."""

    def setUp(self):
        self.base = json.loads((ROOT / "docs/performance-baseline-evidence.json").read_text())

    def doubled(self, time=None, heap=None, rss=None):
        """The baseline as if doubled linearly, with some workloads' ratios replaced."""
        campaign = copy.deepcopy(self.base)
        campaign["scale"] = 2
        grows = {"parse", "waiting"}
        for name, statistics in campaign["statistics"].items():
            for key, statistic, factor in [
                ("workload_elapsed_ns", "p50", (time or {}).get(name, 2)),
                ("heap_kib", "max", (heap or {}).get(name, 2 if name in grows else 1)),
                ("peak_rss_kib", "max", (rss or {}).get(name, 2 if name in grows else 1)),
            ]:
                statistics[key][statistic] = round(statistics[key][statistic] * factor)
        return campaign

    def test_linear_time_and_bounded_memory_pass(self):
        ratios, problems = RUNNER["scaling"](self.base, self.doubled())
        self.assertEqual(problems, [])
        self.assertEqual(set(ratios), {"parse", "calls", "loop", "source-io", "waiting"})
        self.assertEqual(ratios["loop"], {"p50_time": 2.0, "peak_heap": 1.0, "peak_rss": 1.0})
        self.assertEqual(ratios["parse"]["peak_heap"], 2.0)

    def test_superlinear_time_fails(self):
        _, problems = RUNNER["scaling"](self.base, self.doubled(time={"calls": 2.6}))
        self.assertEqual(len(problems), 1)
        self.assertIn("calls: median time grew 2.60 times", problems[0])
        self.assertEqual(RUNNER["scaling"](self.base, self.doubled(time={"calls": 2.45}))[1], [])

    def test_memory_must_stay_bounded_where_live_data_does(self):
        for name, metric, change in [("loop", "peak heap", dict(heap={"loop": 1.15})),
                                     ("source-io", "peak rss", dict(rss={"source-io": 1.15})),
                                     ("parse", "peak heap", dict(heap={"parse": 2.6})),
                                     ("waiting", "peak rss", dict(rss={"waiting": 2.6}))]:
            with self.subTest(name=name, metric=metric):
                _, problems = RUNNER["scaling"](self.base, self.doubled(**change))
                self.assertEqual(len(problems), 1)
                self.assertIn(f"{name}: {metric} grew", problems[0])
        self.assertEqual(RUNNER["scaling"](self.base, self.doubled(heap={"loop": 1.09}))[1], [])

    def test_only_a_doubling_of_comparable_campaigns_is_checked(self):
        quadrupled = dict(self.doubled(), scale=4)
        self.assertEqual(RUNNER["scaling"](self.base, quadrupled), ({}, ["not a doubling: scale 1 to 4"]))
        other = dict(self.doubled(), profile="release")
        self.assertIn("not comparable", RUNNER["scaling"](self.base, other)[1][0])
        self.assertEqual(RUNNER["scaling"](self.base, dict(self.doubled(), complete=False)),
                         ({}, ["not a complete measurement campaign"]))
        uncounted = self.doubled()
        del uncounted["statistics"]["waiting"]["heap_kib"]
        self.assertEqual(RUNNER["scaling"](self.base, uncounted)[1],
                         ["waiting: not measured with peak heap in both campaigns"])

    def test_the_command_line_reports_ratios_and_fails_superlinear_growth(self):
        script = str(ROOT / "scripts/performance.py")
        with tempfile.TemporaryDirectory() as directory:
            base, doubled = Path(directory) / "base.json", Path(directory) / "doubled.json"
            base.write_text(json.dumps(self.base))
            doubled.write_text(json.dumps(self.doubled()))
            passed = subprocess.run([sys.executable, script, "--scaling", str(base), str(doubled)],
                                    capture_output=True, text=True)
            self.assertEqual(passed.returncode, 0, passed.stdout)
            self.assertIn("loop: median time x2.0, peak heap x1.0, peak RSS x1.0", passed.stdout)
            self.assertTrue(passed.stdout.strip().endswith("scaling check passed"))
            doubled.write_text(json.dumps(self.doubled(time={"parse": 4})))
            failed = subprocess.run([sys.executable, script, "--scaling", str(base), str(doubled)],
                                    capture_output=True, text=True)
            self.assertEqual(failed.returncode, 1)
            self.assertIn("scaling check failed: parse: median time grew 4.00 times", failed.stdout)


if __name__ == "__main__":
    unittest.main()
