#!/usr/bin/env python3
"""Fail closed when mutation runs are incomplete or infrastructure fails."""
from copy import deepcopy
import json
from pathlib import Path
import runpy
import sys
import tempfile
import unittest

TOOLS = runpy.run_path(str(Path(__file__).resolve().parents[1] / "scripts/mutation_core.py"))
PASSED = "test result: ok. 2 passed; 0 failed; 0 ignored;\n"
FAILED = "test result: FAILED. 1 passed; 1 failed; 0 ignored;\n"


class MutationTools(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.output = Path(self.temp.name)
        (self.output / "baseline.log").write_text(PASSED)
        (self.output / "mutant.log").write_text(FAILED)
        self.inventory = [{"name": "mutation-one"}]
        self.result = {
            "end_time": "completed", "cargo_mutants_version": "27.1.0", "total_mutants": 1,
            "caught": 1, "missed": 0, "unviable": 0, "timeout": 0,
            "outcomes": [
                {"scenario": "Baseline", "summary": "Success", "log_path": "baseline.log",
                 "phase_results": [{"phase": "Build", "process_status": "Success"},
                                   {"phase": "Test", "process_status": "Success"}]},
                {"scenario": {"Mutant": {"name": "mutation-one"}}, "summary": "CaughtMutant",
                 "log_path": "mutant.log", "phase_results": [
                     {"phase": "Build", "process_status": "Success"},
                     {"phase": "Test", "process_status": {"Failure": 101}},
                 ]},
            ],
        }

    def audit(self, result=None):
        (self.output / "outcomes.json").write_text(json.dumps(self.result if result is None else result))
        return TOOLS["audit_generated"](self.inventory, self.output)

    def test_completed_campaign_has_verified_assertion_kills(self):
        self.assertEqual(self.audit(), {"CaughtMutant": 1})

    def test_missing_duplicate_and_foreign_results_are_rejected(self):
        mutant = self.result["outcomes"][1]
        for rows in ([], [mutant, mutant], [{**mutant, "scenario": {"Mutant": {"name": "other"}}}]):
            result = deepcopy(self.result)
            result["outcomes"] = [result["outcomes"][0], *rows]
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                self.audit(result)

    def test_incomplete_campaigns_and_failed_baselines_are_rejected(self):
        for field, value in (("end_time", None), ("cargo_mutants_version", "unknown")):
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.audit({**self.result, field: value})
        self.result["outcomes"][0]["summary"] = "Failure"
        with self.assertRaises(ValueError):
            self.audit()

    def test_empty_baseline_does_not_pass(self):
        (self.output / "baseline.log").write_text("test result: ok. 0 passed; 0 failed;\n")
        with self.assertRaises(ValueError):
            self.audit()

    def test_kills_require_a_successful_build_and_real_failed_tests(self):
        result = deepcopy(self.result)
        result["outcomes"][1]["phase_results"][0]["process_status"] = {"Failure": 101}
        with self.assertRaises(ValueError):
            self.audit(result)
        for log in ("", "error: failed to compile", PASSED):
            (self.output / "mutant.log").write_text(log)
            with self.subTest(log=log), self.assertRaises(ValueError):
                self.audit()

    def test_inconsistent_totals_and_unknown_outcomes_are_rejected(self):
        for field in ("total_mutants", "caught"):
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.audit({**self.result, field: 7})
        self.result["outcomes"][1]["summary"] = "Unknown"
        with self.assertRaises(ValueError):
            self.audit()

    def test_build_failures_and_timeouts_are_separate_from_test_kills(self):
        for status, field, phases in [
            ("Unviable", "unviable", [{"phase": "Build", "process_status": {"Failure": 101}}]),
            ("Timeout", "timeout", [{"phase": "Build", "process_status": "Success"},
                                    {"phase": "Test", "process_status": "Timeout"}]),
        ]:
            result = deepcopy(self.result)
            result.update(caught=0, **{field: 1})
            result["outcomes"][1].update(summary=status, phase_results=phases)
            with self.subTest(status=status):
                self.assertEqual(self.audit(result), {status: 1})
            result["outcomes"][1]["phase_results"] = []
            with self.assertRaises(ValueError):
                self.audit(result)

    def test_empty_tests_crashes_and_other_failures_never_count_as_kills(self):
        for code, log in [(0, ""), (0, "test result: ok. 0 passed; 0 failed;"),
                          (101, "error: could not compile"), (-9, FAILED), (None, FAILED)]:
            with self.subTest(code=code, log=log):
                self.assertEqual(TOOLS["test_status"](code, log), "error")
        self.assertEqual(TOOLS["test_status"](0, PASSED), "missed")
        self.assertEqual(TOOLS["test_status"](101, FAILED), "caught")

    def test_stale_ambiguous_and_empty_substitutions_are_rejected(self):
        for source, before, after in [("abc", "x", "y"), ("aaa", "a", "b"),
                                       ("abc", "", "x"), ("abc", "a", "a")]:
            with self.subTest(source=source), self.assertRaises(ValueError):
                TOOLS["replace_once"](source, {"id": "example", "before": before, "after": after})

    def test_process_deadline_is_recorded_as_timeout(self):
        outcome = TOOLS["execute"]([sys.executable, "-c", "import time; time.sleep(60)"],
                                   self.output, self.output / "timeout.log", 0.05)
        self.assertTrue(outcome["timeout"])
        self.assertIsNone(outcome["returncode"])

    def test_outer_failures_cannot_be_hidden_by_complete_outcome_files(self):
        for counts, code in [({"CaughtMutant": 1}, 0), ({"MissedMutant": 1}, 2), ({"Timeout": 1}, 3)]:
            TOOLS["verify_exit"]({"timeout": False, "returncode": code}, counts)
            for execution in ({"timeout": True, "returncode": code},
                              {"timeout": False, "returncode": 70}):
                with self.subTest(counts=counts, execution=execution), self.assertRaises(ValueError):
                    TOOLS["verify_exit"](execution, counts)


if __name__ == "__main__":
    unittest.main()
