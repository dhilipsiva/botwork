#!/usr/bin/env python3
"""Collect separate library-unit and full-suite line coverage baselines."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "target" / "coverage"
TOOL_VERSION = "cargo-llvm-cov 0.9.1"
EXCLUSIONS = (r"(^|[/\\])tests([/\\]|\.rs$)|[/\\]src[/\\]core[/\\]parser\.rs$"
              r"|[/\\]src[/\\]core[/\\]eval[/\\](execution_contract|native_contract)\.rs$")
# Review these lists whenever adding executable source files. Module declarations
# have no executable lines; the generated Pest parser is excluded deliberately.
LIBRARY_FILES = {
    "src/core/eval/builtins/http.rs",
    "src/core/eval/builtins/http/config.rs",
    "src/core/eval/builtins/http/output.rs",
    "src/core/eval/builtins/http/transport.rs",
    "src/core/eval/builtins/processes.rs",
    "src/core/eval/builtins/processes/config.rs",
    "src/core/eval/builtins/processes/output.rs",
    "src/core/worker/waiting.rs",
    "src/core/eval/builtins/operating_system.rs",
    "src/core/eval/builtins/operating_system/environment.rs",
    "src/core/eval/builtins/operating_system/files/read.rs",
    "src/core/eval/builtins/operating_system/files.rs",
    "src/core/eval/builtins/operating_system/paths.rs",
    "src/core/eval/builtins/operating_system/values.rs",
    "src/core/eval/builtins/datetime.rs", "src/core/eval/builtins/datetime/duration.rs", "src/core/eval/builtins/datetime/moment.rs",
    "src/core/eval/builtins.rs", "src/core/eval/builtins/assertions.rs",
    "src/core/eval/builtins/collections.rs",
    "src/core/eval/builtins/collections/build.rs",
    "src/core/eval/builtins/strings.rs",
    "src/core/eval/builtins/strings/build.rs",
    "src/core/eval/builtins/strings/template.rs",
    "src/core/eval/builtins/strings/patterns.rs",
    "src/core/acceptance.rs", "src/core/acceptance/totals.rs", "src/core/acceptance/view.rs",
    "src/core/run/cleanup.rs", "src/core/eval/cleanup.rs", "src/core/run/attempt.rs", "src/core/eval/polling.rs", "src/core/csv.rs", "src/core/report.rs", "src/core/listener.rs", "src/core/secret.rs", "src/core/eval/compiled.rs", "src/core/analysis.rs", "src/core/format.rs", "src/core/language.rs", "src/core/eval/builtins/data.rs", "src/core/eval/builtins/data/csv.rs",
    "src/core/ast/suite.rs", "src/core/ast/suite/dataset.rs",
    "src/core/ast/suite/fixtures.rs", "src/core/eval/fixtures.rs",
    "src/core/ast.rs", "src/core/ast/parse_diagnostic.rs", "src/core/ast_limits.rs", "src/core/diagnostic.rs", "src/core/diagnostic/value.rs", "src/core/diagnostic/ownership.rs", "src/core/diagnostic/rejection.rs", "src/core/diagnostic/construction.rs", "src/core/diagnostic/render.rs", "src/core/eval.rs", "src/core/eval/execution.rs", "src/core/eval/filesystem.rs", "src/core/eval/blocking.rs", "src/core/eval/diagnostics.rs",
    "src/core/eval/output.rs", "src/core/run/output_limits.rs", "src/core/eval/imports.rs", "src/core/eval/snapshots.rs", "src/core/eval/results.rs", "src/core/eval/temporaries.rs", "src/core/grammar.rs", "src/core/input.rs", "src/core/input/limits.rs", "src/core/input/raw.rs",
    "src/core/worker/protocol.rs", "src/core/worker/protocol/values.rs", "src/core/worker/protocol/diagnostics.rs", "src/core/worker/protocol/diagnostics/errors.rs", "src/core/operation/isolated.rs", "src/core/worker.rs", "src/core/worker/linux.rs", "src/core/worker/linux/launch.rs", "src/core/worker/linux/namespace.rs", "src/core/worker/linux/process.rs", "src/core/worker/linux/guardian.rs", "src/core/worker/journal.rs", "src/core/worker/journal/format.rs", "src/core/worker/journal/storage.rs", "src/core/worker/linux/observation.rs", "src/core/worker/linux/owner.rs", "src/core/operation.rs", "src/core/operation/ownership.rs", "src/core/operation/diagnostics.rs", "src/core/run.rs", "src/core/run/asynchronous.rs", "src/core/run/blocking_io.rs", "src/core/run/import_limits.rs", "src/core/run/retained_values.rs", "src/core/run/retained_definitions.rs", "src/core/run/retained_diagnostics.rs", "src/core/run/retained_names.rs", "src/core/run/retained_registry.rs", "src/core/run/snapshot_limits.rs", "src/core/run/result_limits.rs", "src/core/run/temporary_values.rs", "src/core/signature.rs", "src/core/syntax_limits.rs", "src/core/value_limits.rs",
}
EXPECTED_FILES = {"unit": LIBRARY_FILES, "all": LIBRARY_FILES | {"src/main.rs", "src/check.rs", "src/formatting.rs", "src/lsp.rs", "src/assertion_artifacts.rs", "src/atomic_file.rs", "src/listener.rs", "src/report_json.rs", "src/report_json/html.rs", "src/report_json/journal.rs", "src/interrupt.rs", "src/secrets.rs", "src/batch.rs", "src/suites.rs", "src/suites/history.rs", "src/suites/datasets.rs", "src/suites/execution.rs"}}


def capture(*command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def line_summary(report, expected_files):
    """Reject missing/unexpected files instead of silently shrinking the scope."""
    files = {}
    for data in report["data"]:
        for entry in data["files"]:
            name = Path(entry["filename"]).resolve().relative_to(ROOT).as_posix()
            if name in files:
                raise ValueError(f"duplicate coverage file: {name}")
            lines = entry["summary"]["lines"]
            covered, total = lines["covered"], lines["count"]
            if not 0 <= covered <= total or total == 0:
                raise ValueError(f"invalid line counts for {name}: {lines}")
            files[name] = {"covered": covered, "total": total}
    if files.keys() != expected_files:
        raise ValueError(f"coverage scope mismatch: expected {sorted(expected_files)}, got {sorted(files)}")
    covered = sum(value["covered"] for value in files.values())
    total = sum(value["total"] for value in files.values())
    return {"files": files, "covered": covered, "total": total,
            "percent": round(100 * covered / total, 2)}


def collect(scope, offline):
    report_path = OUTPUT / f"{scope}.json"
    log_path = OUTPUT / f"{scope}.log"
    command = ["cargo", "llvm-cov", "--locked", "--json", "--summary-only",
               "--ignore-filename-regex", EXCLUSIONS,
               "--output-path", str(report_path)]
    if scope == "unit":
        command.append("--lib")
    if offline:
        command.append("--offline")
    env = os.environ.copy()
    # Each invocation cleans its own profiles. Separate targets also prevent
    # a preceding full-suite run from contributing to library-unit coverage.
    target = str(OUTPUT / f"{scope}-target")
    env.update(CARGO_LLVM_COV_TARGET_DIR=target, CARGO_LLVM_COV_BUILD_DIR=target,
               LLVM_PROFILE_FILE_NAME="botwork-%p-%m.profraw")
    print(f"{scope}: {shlex.join(command)}", flush=True)
    with log_path.open("w") as log:
        result = subprocess.run(command, cwd=ROOT, env=env, stdout=log,
                                stderr=subprocess.STDOUT, check=False)
    log_text = log_path.read_text()
    if result.returncode:
        print(log_text, file=sys.stderr)
        raise subprocess.CalledProcessError(result.returncode, command)
    counts = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;", log_text)
    if not counts:
        raise ValueError(f"no successful test summaries found in {log_path}")
    tests = dict(zip(("passed", "failed", "ignored"),
                     (sum(int(row[i]) for row in counts) for i in range(3))))
    summary = line_summary(json.loads(report_path.read_text()), EXPECTED_FILES[scope])
    summary.update(command=command, tests=tests,
                   environment={key: env[key] for key in (
                       "CARGO_LLVM_COV_TARGET_DIR", "CARGO_LLVM_COV_BUILD_DIR",
                       "LLVM_PROFILE_FILE_NAME")})
    print(f"{scope}: {summary['covered']}/{summary['total']} lines ({summary['percent']}%); {tests}")
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scope", choices=("unit", "all", "both"), default="both")
    parser.add_argument("--offline", action="store_true", help="use cached Cargo dependencies")
    args = parser.parse_args()
    version = capture("cargo", "llvm-cov", "--version")
    if version != TOOL_VERSION:
        raise ValueError(f"expected {TOOL_VERSION}, got {version}; review tool upgrades explicitly")
    overrides = ("LLVM_COV", "LLVM_PROFDATA", "LLVM_COV_FLAGS", "LLVM_PROFDATA_FLAGS",
                 "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS",
                 "CARGO_BUILD_TARGET", "RUSTC", "CARGO_BUILD_RUSTC", "RUSTC_WRAPPER",
                 "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER",
                 "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
    if any(os.environ.get(name) for name in overrides):
        raise ValueError("unset coverage/compiler overrides before collecting the baseline: "
                         + ", ".join(name for name in overrides if os.environ.get(name)))
    rustc = capture("rustc", "-vV")
    host = next(line.removeprefix("host: ") for line in rustc.splitlines() if line.startswith("host: "))
    llvm_bin = Path(capture("rustc", "--print", "sysroot")) / "lib" / "rustlib" / host / "bin"
    executable_suffix = ".exe" if os.name == "nt" else ""
    OUTPUT.mkdir(parents=True, exist_ok=True)
    inputs = [ROOT / name for name in ("Cargo.toml", "Cargo.lock", "scripts/coverage.py")]
    for directory in ("src", "tests", "examples"):
        inputs.extend(path for path in (ROOT / directory).rglob("*")
                      if path.is_file() and "__pycache__" not in path.parts)
    summary = {
        "schema": 1,
        "captured_at": datetime.now(timezone.utc).isoformat(),
        "base_revision": capture("git", "rev-parse", "HEAD"),
        "worktree_status": capture("git", "status", "--porcelain"),
        "input_sha256": {path.relative_to(ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
                         for path in sorted(inputs)},
        "platform": platform.platform(),
        "rustc": rustc,
        "cargo": capture("cargo", "--version"),
        "cargo_llvm_cov": version,
        "llvm_cov": capture(str(llvm_bin / f"llvm-cov{executable_suffix}"), "--version"),
        "llvm_profdata": capture(str(llvm_bin / f"llvm-profdata{executable_suffix}"), "--version"),
        "profile": "debug",
        "metric": "executable line coverage of maintained Rust source files",
        "excluded": ["tests/**", "src/**/tests.rs", "src/core/eval/execution_contract.rs",
                     "src/core/eval/native_contract.rs", "src/core/parser.rs (generated Pest parser)"],
        "unmeasured": ["branch coverage", "grammar-rule coverage", "ignored regressions", "doctests"],
        "scopes": {},
    }
    scopes = ("unit", "all") if args.scope == "both" else (args.scope,)
    for scope in scopes:
        summary["scopes"][scope] = collect(scope, args.offline)
    path = OUTPUT / f"{args.scope}-summary.json"
    path.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    print(f"Saved {path.relative_to(ROOT)}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        sys.exit(f"coverage: {error}")
