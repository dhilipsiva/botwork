#!/usr/bin/env python3
"""Run and record the fixed, seeded libFuzzer smoke campaign on Linux."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
TARGETS = ("parse", "expression", "literals")
NIGHTLY = "nightly-2026-08-14"
RUSTC = "rustc 1.99.0-nightly (ba28ff76f 2026-08-13)"
FUZZ_VERSION = "cargo-fuzz 0.13.2"


def capture(*command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def verify(log, returncode, requested):
    if returncode != 0:
        raise ValueError(f"fuzzer exited with status {returncode}")
    fields = {key: int(value) for key, value in re.findall(r"^stat::(\w+):\s+(\d+)\s*$", log, re.M)}
    if fields.get("number_of_executed_units", 0) < requested:
        raise ValueError("fuzzer did not finish the requested iteration budget")
    if not re.search(r"^#\d+\s+DONE\b", log, re.M):
        raise ValueError("missing final libFuzzer completion marker")
    return fields


def execute(command, log, env, timeout):
    with log.open("w") as output:
        child = subprocess.Popen(command, cwd=ROOT, env=env, stdout=output,
                                 stderr=subprocess.STDOUT, start_new_session=True)
        try:
            return child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
            raise


def positive(value):
    value = int(value)
    if value < 1:
        raise argparse.ArgumentTypeError("must be positive")
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toolchain", default=NIGHTLY, help="an installed alias for the pinned nightly")
    parser.add_argument("--target", choices=(*TARGETS, "all"), default="all")
    parser.add_argument("--runs", type=positive, default=10000)
    parser.add_argument("--seed", type=positive, default=314159)
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("the recorded campaign runner currently requires Linux")
    if args.seed > 0xffffffff:
        parser.error("seed must fit libFuzzer's 32-bit unsigned seed")
    if capture("cargo", "fuzz", "--version") != FUZZ_VERSION:
        parser.error(f"install {FUZZ_VERSION} with --locked")
    rustc = capture("rustc", f"+{args.toolchain}", "-Vv")
    if rustc.splitlines()[0] != RUSTC:
        parser.error(f"install {NIGHTLY}; got {rustc.splitlines()[0]}")
    output_root = ROOT / "target" / "fuzz-smoke"
    output_root.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix="campaign-", dir=output_root))
    inputs = [ROOT / path for path in ("Cargo.toml", "Cargo.lock", "scripts/fuzz_smoke.py", "tests/generated_properties.rs", "tests/support/generated.rs")]
    for directory in ("src", "fuzz/fuzz_targets", "fuzz/seeds"):
        inputs.extend(path for path in (ROOT / directory).rglob("*") if path.is_file())
    inputs.extend(ROOT / name for name in ("fuzz/Cargo.toml", "fuzz/Cargo.lock", "fuzz/botwork.dict"))
    fingerprints = {path.relative_to(ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(inputs)}
    summary = {
        "schema": 1, "captured_at": datetime.now(timezone.utc).isoformat(),
        "base_revision": capture("git", "rev-parse", "HEAD"),
        "worktree_status": capture("git", "status", "--porcelain"),
        "input_sha256": fingerprints, "platform": platform.platform(),
        "rustc": rustc, "cargo": capture("cargo", f"+{args.toolchain}", "-V"),
        "cargo_fuzz": FUZZ_VERSION, "seed": args.seed,
        "limits": {"runs_per_target": args.runs, "input_bytes": 8192, "source_bytes": 4096,
                   "input_timeout_seconds": 2, "fuzzer_timeout_seconds": 60, "process_timeout_seconds": 300,
                   "rss_limit_mib": 512, "evaluation_steps": 256, "evaluation_depth": 64, "call_depth": 8},
        "sanitizer": "address; optimized build with debug assertions and overflow checks",
        "external_adapters": "none; raw input is parsed only; evaluation uses generated pure expressions/values",
        "targets": [], "result": "incomplete",
    }
    print(f"campaign record: {output / 'summary.json'}", flush=True)
    try:
        fetch = ["cargo", f"+{args.toolchain}", "fetch", "--locked", "--manifest-path", "fuzz/Cargo.toml"]
        if execute(fetch, output / "fetch.log", os.environ.copy(), 300):
            raise ValueError("locked dependency fetch failed; see fetch.log")
        env = dict(os.environ, CARGO_NET_OFFLINE="true")
        selected = TARGETS if args.target == "all" else (args.target,)
        for target in selected:
            directory = output / target
            corpus, artifacts = directory / "corpus", directory / "artifacts"
            corpus.mkdir(parents=True)
            artifacts.mkdir()
            command = ["cargo", f"+{args.toolchain}", "fuzz", "run", target, str(corpus),
                       str(ROOT / "fuzz" / "seeds" / target), "--", f"-seed={args.seed}", f"-runs={args.runs}",
                       "-max_len=8192", "-timeout=2", "-max_total_time=60", "-rss_limit_mb=512",
                       "-print_final_stats=1", f"-artifact_prefix={artifacts}/"]
            if target == "parse":
                command.append(f"-dict={ROOT / 'fuzz' / 'botwork.dict'}")
            record = {"target": target, "command": command, "result": "incomplete"}
            summary["targets"].append(record)
            start = time.monotonic()
            try:
                status = execute(command, directory / "run.log", env, 300)
                log = (directory / "run.log").read_text()
                record.update(returncode=status, log_sha256=hashlib.sha256(log.encode()).hexdigest())
                record["statistics"] = verify(log, status, args.runs)
                record["result"] = "passed"
                print(f"{target}: {record['statistics']['number_of_executed_units']} executions passed", flush=True)
            finally:
                record["elapsed_seconds"] = round(time.monotonic() - start, 3)
                record["artifacts"] = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(artifacts.iterdir()) if path.is_file()}
        for name, digest in fingerprints.items():
            if hashlib.sha256((ROOT / name).read_bytes()).hexdigest() != digest:
                raise ValueError(f"campaign input changed during execution: {name}")
        summary["result"] = "passed"
    except Exception as error:
        summary["result"] = "failed"
        summary["error"] = str(error)
        raise
    finally:
        (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
    main()
