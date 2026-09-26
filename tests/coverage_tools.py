"""Tests for the coverage report's scope and collection safeguards."""

import contextlib
import io
import json
from pathlib import Path
import runpy
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch


COVERAGE = runpy.run_path(str(Path(__file__).resolve().parents[1] / "scripts/coverage.py"))
ROOT = COVERAGE["ROOT"]
SUMMARIZE = COVERAGE["line_summary"]
COLLECT = COVERAGE["collect"]


def report(*entries):
    return {"data": [{"files": [
        {"filename": str(ROOT / name), "summary": {"lines": {"covered": covered, "count": total}}}
        for name, covered, total in entries
    ]}]}


class CoverageTests(unittest.TestCase):
    def test_aggregate_uses_line_counts_not_average_of_percentages(self):
        result = SUMMARIZE(report(("a.rs", 1, 2), ("b.rs", 1, 8)), {"a.rs", "b.rs"})
        self.assertEqual((result["covered"], result["total"], result["percent"]), (2, 10, 20.0))

    def test_missing_file_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "scope mismatch"):
            SUMMARIZE(report(("a.rs", 1, 2)), {"a.rs", "b.rs"})

    def test_unexpected_file_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "scope mismatch"):
            SUMMARIZE(report(("tests/a.rs", 1, 2)), {"a.rs"})

    def test_duplicate_file_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "duplicate"):
            SUMMARIZE(report(("a.rs", 1, 2), ("a.rs", 1, 2)), {"a.rs"})

    def test_invalid_counts_are_rejected(self):
        for covered, total in ((0, 0), (-1, 2), (3, 2)):
            with self.subTest(covered=covered, total=total), self.assertRaises(ValueError):
                SUMMARIZE(report(("a.rs", covered, total)), {"a.rs"})

    def test_exclusions_retain_maintained_source_files(self):
        for path in ("tests/cli.rs", "src/core/eval/tests.rs", "src/core/ast/tests.rs", "src/core/parser.rs",
                     "src/core/eval/execution_contract.rs", "src/core/eval/native_contract.rs"):
            for separator in ("/", "\\"):
                with self.subTest(path=path, separator=separator):
                    self.assertIsNotNone(re.search(COVERAGE["EXCLUSIONS"],
                                                  ("root/" + path).replace("/", separator)))
        for path in ("src/main.rs", "src/core/ast.rs", "src/core/eval.rs", "src/core/grammar.rs"):
            self.assertIsNone(re.search(COVERAGE["EXCLUSIONS"], "root/" + path))

    def test_collection_separates_scopes_and_counts_ignored_tests(self):
        commands = []

        def fake_run(command, **kwargs):
            scope = "unit" if "--lib" in command else "all"
            commands.append((command, kwargs["env"]))
            files = COVERAGE["EXPECTED_FILES"][scope]
            output = Path(command[command.index("--output-path") + 1])
            output.write_text(json.dumps(report(*((name, 1, 2) for name in sorted(files)))))
            kwargs["stdout"].write("test result: ok. 35 passed; 0 failed; 0 ignored;\n")
            if scope == "all":
                kwargs["stdout"].write("test result: ok. 19 passed; 0 failed; 13 ignored;\n")
            return subprocess.CompletedProcess(command, 0)

        with tempfile.TemporaryDirectory() as directory, \
                patch.dict(COLLECT.__globals__, OUTPUT=Path(directory)), \
                patch("subprocess.run", side_effect=fake_run), \
                contextlib.redirect_stdout(io.StringIO()):
            unit = COLLECT("unit", True)
            full = COLLECT("all", True)
        self.assertEqual(unit["tests"], {"passed": 35, "failed": 0, "ignored": 0})
        self.assertEqual(full["tests"], {"passed": 54, "failed": 0, "ignored": 13})
        self.assertNotEqual(commands[0][1]["CARGO_LLVM_COV_TARGET_DIR"],
                            commands[1][1]["CARGO_LLVM_COV_TARGET_DIR"])
        for command, env in commands:
            self.assertIn("--offline", command)
            self.assertIn("--locked", command)
            self.assertNotIn("--no-clean", command)
            self.assertIn("%p", env["LLVM_PROFILE_FILE_NAME"])
            self.assertIn("%m", env["LLVM_PROFILE_FILE_NAME"])

    def test_failed_test_command_never_produces_a_summary(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.dict(COLLECT.__globals__, OUTPUT=Path(directory)), \
                patch("subprocess.run", return_value=subprocess.CompletedProcess([], 1)), \
                contextlib.redirect_stdout(io.StringIO()), \
                contextlib.redirect_stderr(io.StringIO()), \
                self.assertRaises(subprocess.CalledProcessError):
            COLLECT("all", False)


if __name__ == "__main__":
    unittest.main()
