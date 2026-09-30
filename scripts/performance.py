#!/usr/bin/env python3
"""Measure correctness-checked runtime workloads in fresh Linux processes."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
VERSION = 1
TARGETS = ("x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl")
BASELINE = ROOT / "docs/performance-evidence.json"
BUDGETS = ROOT / "benches/runtime/budgets.json"
# Roadmap decision D4: budgets are the accepted baseline's p95 workload time and
# maximum peak memory, each times 1.25; item 249 blocks unexplained regressions
# of more than 10% against the baseline.
BUDGET_FACTOR = 1.25
REGRESSION_LIMIT = 0.10


def workloads(smoke=False):
    parse, calls, loops, reads, size, waiting = (
        (100, 100, 1000, 1, 4096, 4) if smoke else
        (10_000, 100_000, 1_000_000, 16, 256 * 1024, 100)
    )
    definitions = [
        ("cli-startup", 1, "Log |42|\n", 42),
        ("parse", parse, "".join(f"|value| = |{i}|\n" for i in range(parse)), parse),
        ("calls", calls, f"Next |n| {{ Return |n + 1| }}\n|answer| = |0|\nWhile |answer < {calls}| {{ |answer| = Next |answer| }}\n", calls),
        ("loop", loops, f"|answer| = |0|\nWhile |answer < {loops}| {{ |answer| = |answer + 1| }}\n", loops),
        ("source-io", reads, "|answer| = |42|\n#" + "x" * (size - len("|answer| = |42|\n#\n")) + "\n", 42 * reads),
        ("waiting", waiting, "|answer| = Await |input|\n", waiting * (waiting - 1) // 2),
    ]
    return [{"id": name, "units": units, "source": source, "checksum": checksum}
            for name, units, source, checksum in definitions]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def capture(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def execute(command, directory, prefix, timeout):
    """Retain the leader's PID through watchdog cleanup, then reap with wait4."""
    stdout, stderr = prefix.with_suffix(".stdout"), prefix.with_suffix(".stderr")
    timed_out = threading.Event()
    with stdout.open("w") as output, stderr.open("w") as errors:
        start = time.perf_counter_ns()
        child = subprocess.Popen(command, cwd=directory, stdin=subprocess.DEVNULL,
                                 stdout=output, stderr=errors, start_new_session=True)
        try:
            pidfd = os.pidfd_open(child.pid)
        except BaseException:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait()
            raise
        lifecycle = threading.Lock()
        finished = False

        def stop():
            with lifecycle:
                if finished:
                    return
                timed_out.set()
                try:
                    signal.pidfd_send_signal(pidfd, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                # Cargo builds have descendants too. WNOWAIT below retains the
                # owned leader PID until this callback can no longer run.
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass

        timer = threading.Timer(timeout, stop)
        timer.start()
        try:
            os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOWAIT)
            with lifecycle:
                finished = True
                _, status, usage = os.wait4(child.pid, 0)
            elapsed = time.perf_counter_ns() - start
            child.returncode = os.waitstatus_to_exitcode(status)
        except BaseException:
            stop()
            with lifecycle:
                finished = True
                try:
                    _, status, _ = os.wait4(child.pid, 0)
                    child.returncode = os.waitstatus_to_exitcode(status)
                except ChildProcessError:
                    pass  # An interrupt can arrive just after the successful reap.
            raise
        finally:
            timer.cancel()
            timer.join()
            os.close(pidfd)
    return {
        "command": command, "cwd": str(directory), "returncode": child.returncode,
        "timeout": timed_out.is_set(), "process_elapsed_ns": elapsed,
        "peak_rss_kib": usage.ru_maxrss, "user_seconds": usage.ru_utime,
        "system_seconds": usage.ru_stime, "voluntary_switches": usage.ru_nvcsw,
        "involuntary_switches": usage.ru_nivcsw,
        "stdout": str(stdout), "stderr": str(stderr),
        "stdout_sha256": digest(stdout), "stderr_sha256": digest(stderr),
    }


def verify(sample, workload, stdout, stderr):
    if sample["timeout"] or sample["returncode"] != 0:
        raise ValueError("workload failed or exceeded the process watchdog")
    if stderr or sample["peak_rss_kib"] <= 0 or sample["process_elapsed_ns"] <= 0:
        raise ValueError("unexpected stderr or missing process measurements")
    if workload["id"] == "cli-startup":
        if stdout != "42\n":
            raise ValueError("CLI did not produce the expected result")
        return sample["process_elapsed_ns"]
    result = json.loads(stdout)
    expected = {"schema": VERSION, "workload": workload["id"],
                "units": workload["units"], "checksum": workload["checksum"]}
    if (not isinstance(result, dict) or set(result) != {*expected, "elapsed_ns"}
            or any(type(result[k]) is not type(v) or result[k] != v for k, v in expected.items())):
        raise ValueError("workload identity, size, or correctness check changed")
    elapsed = result["elapsed_ns"]
    if type(elapsed) is not int or not 0 < elapsed <= sample["process_elapsed_ns"]:
        raise ValueError("invalid workload duration")
    return elapsed


def statistics(values):
    if not values or any(type(v) not in (int, float) or not math.isfinite(v) or v <= 0 for v in values):
        raise ValueError("statistics require positive, finite observations")
    ordered = sorted(values)
    return {"count": len(ordered), "min": ordered[0],
            "p50": ordered[math.ceil(0.50 * len(ordered)) - 1],
            "p95": ordered[math.ceil(0.95 * len(ordered)) - 1], "max": ordered[-1]}


def measured(command, directory, prefix, supervisor):
    report = prefix.with_suffix(".probe.json")
    sample = execute([supervisor, "--probe", str(report), *command], directory, prefix, 120)
    if sample["timeout"] or sample["returncode"]:
        return sample
    probe = json.loads(report.read_text())
    if probe.get("schema") != VERSION or probe.get("returncode") != 0:
        raise ValueError("native measurement probe failed")
    # Keep the Python-observed lifetime separately; its RSS includes Python's
    # inherited high-water mark and must not enter workload memory statistics.
    sample["supervisor_elapsed_ns"] = sample["process_elapsed_ns"]
    sample["probe_report"] = str(report)
    sample["probe_report_sha256"] = digest(report)
    for key in ("process_elapsed_ns", "peak_rss_kib", "user_seconds", "system_seconds",
                "voluntary_switches", "involuntary_switches"):
        sample[key] = probe[key]
    return sample


def calibrate(directory, supervisor):
    # Touch every 4 KiB so the Python parent actually owns 64 MiB of resident
    # pages. A fork/exec measurement taken by Python alone inherits that floor.
    padding = bytearray(64 * 1024 * 1024)
    padding[::4096] = b"x" * (len(padding) // 4096)
    small = measured([sys.executable, "-I", "-S", "-c", "print(42)"], directory,
                     directory / "calibration-small", supervisor)
    source = "data = bytearray(32 * 1024 * 1024); data[::4096] = b'x' * 8192; print(sum(data))"
    large = measured([sys.executable, "-I", "-S", "-c", source], directory,
                     directory / "calibration-large", supervisor)
    for row, expected in [(small, "42\n"), (large, "983040\n")]:
        if (row["timeout"] or row["returncode"] or Path(row["stdout"]).read_text() != expected
                or Path(row["stderr"]).read_text()):
            raise ValueError("memory calibration child failed")
    if not 0 < small["peak_rss_kib"] < 64 * 1024:
        raise ValueError("small child inherited the Python parent's memory high-water mark")
    if large["peak_rss_kib"] < max(32 * 1024, small["peak_rss_kib"] + 16 * 1024):
        raise ValueError("memory probe did not observe the child's 32 MiB allocation")
    del padding
    return {"parent_padding_bytes": 64 * 1024 * 1024, "large_child_padding_bytes": 32 * 1024 * 1024,
            "small": small, "large": large, "verified": True}


def aggregate(rows, definitions, samples, warmups):
    expected = {(phase, index, case["id"]) for phase, count in
                [("warmup", warmups), ("measured", samples)]
                for index in range(count) for case in definitions}
    keys = [(row["phase"], row["index"], row["workload"]) for row in rows]
    if len(keys) != len(set(keys)) or set(keys) != expected:
        raise ValueError("incomplete or duplicate sample inventory")
    if any(not row.get("verified") or row["timeout"] or row["returncode"] for row in rows):
        raise ValueError("failed observations cannot be excluded from a campaign")
    return {
        case["id"]: {metric: statistics([row[metric] for row in rows
                                        if row["phase"] == "measured" and row["workload"] == case["id"]])
                     for metric in ("workload_elapsed_ns", "process_elapsed_ns", "peak_rss_kib")}
        for case in definitions
    }


def host(record):
    """What makes two campaigns comparable: the CPU, visible CPUs, target, and profile."""
    model = re.search(r"^model name\s*:\s*(.+)$", record["machine"]["cpuinfo"], re.M)
    return {"cpu": model.group(1).strip() if model else "unknown",
            "cpus": len(record["machine"]["cpu_affinity"]),
            "target": record["target"], "profile": record["profile"]}


def budgets_from(baseline):
    """The budgets roadmap decision D4 registers from an accepted baseline campaign."""
    if baseline.get("kind") != "measurement" or not baseline.get("complete"):
        raise ValueError("budgets need a complete measurement campaign")
    return {
        "schema": 1, "decision": "D4",
        "baseline": {"record": str(BASELINE.relative_to(ROOT)),
                     "base_revision": baseline["base_revision"],
                     "captured_at": baseline["captured_at"]},
        "host": host(baseline), "budget_factor": BUDGET_FACTOR,
        "regression_limit": REGRESSION_LIMIT,
        "workloads": {
            name: {"baseline_p95_ns": stats["workload_elapsed_ns"]["p95"],
                   "p95_budget_ns": round(stats["workload_elapsed_ns"]["p95"] * BUDGET_FACTOR),
                   "baseline_peak_rss_kib": stats["peak_rss_kib"]["max"],
                   "peak_rss_budget_kib": math.ceil(stats["peak_rss_kib"]["max"] * BUDGET_FACTOR)}
            for name, stats in baseline["statistics"].items()},
    }


def check(campaign, budgets, explained=()):
    """Why a campaign fails the registered budgets, or an empty list.

    A campaign counts only when it is a complete measurement on the budgets'
    host, target, and profile. Each workload's p95 time and peak memory must be
    within budget, and neither may regress more than the limit against the
    baseline unless the workload's regression is explained."""
    if campaign.get("kind") != "measurement" or not campaign.get("complete"):
        return ["not a complete measurement campaign"]
    if host(campaign) != budgets["host"]:
        return [f"not comparable: measured on {host(campaign)}, budgets are for {budgets['host']}"]
    problems = []
    limit = budgets["regression_limit"]
    for name, budget in budgets["workloads"].items():
        stats = campaign["statistics"].get(name)
        if stats is None:
            problems.append(f"{name}: not measured")
            continue
        p95, peak = stats["workload_elapsed_ns"]["p95"], stats["peak_rss_kib"]["max"]
        if p95 > budget["p95_budget_ns"]:
            problems.append(f"{name}: p95 {p95} ns exceeds its budget of {budget['p95_budget_ns']} ns")
        if peak > budget["peak_rss_budget_kib"]:
            problems.append(f"{name}: peak memory {peak} KiB exceeds its budget of {budget['peak_rss_budget_kib']} KiB")
        for metric, value, base in [("p95", p95, budget["baseline_p95_ns"]),
                                    ("peak memory", peak, budget["baseline_peak_rss_kib"])]:
            if value > base * (1 + limit) and name not in explained:
                problems.append(f"{name}: {metric} regressed {100 * (value / base - 1):.1f}% against the baseline, more than {100 * limit:.0f}%, without an explanation")
    return problems


def fingerprints():
    paths = [ROOT / name for name in ("Cargo.toml", "Cargo.lock", "scripts/performance.py")]
    paths.extend(path for path in (ROOT / "benches").rglob("*.rs"))
    paths.extend(path for path in (ROOT / "src").rglob("*") if path.is_file())
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(paths)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--smoke", action="store_true", help="small correctness check; never a performance baseline")
    parser.add_argument("--target", choices=TARGETS, default=TARGETS[0])
    parser.add_argument("--profile", choices=("debug", "release"), default="release")
    parser.add_argument("--check", metavar="SUMMARY", type=Path,
                        help="check a completed campaign's summary.json against the registered budgets")
    parser.add_argument("--explain", metavar="WORKLOAD=REASON", action="append", default=[],
                        help="accept a workload's regression beyond the limit, with the reason (repeatable)")
    parser.add_argument("--write-budgets", action="store_true",
                        help="register budgets from the accepted baseline in docs/performance-evidence.json")
    args = parser.parse_args()
    if args.write_budgets:
        BUDGETS.write_text(json.dumps(budgets_from(json.loads(BASELINE.read_text())), indent=2) + "\n")
        print(f"wrote {BUDGETS.relative_to(ROOT)}")
        return
    if args.check:
        explained = {}
        for item in args.explain:
            name, _, reason = item.partition("=")
            if not reason.strip():
                parser.error(f"--explain {item!r} needs WORKLOAD=REASON")
            explained[name] = reason.strip()
        campaign = json.loads(args.check.read_text())
        problems = check(campaign, json.loads(BUDGETS.read_text()), explained)
        for name, reason in explained.items():
            print(f"explained regression: {name}: {reason}")
        for problem in problems:
            print(f"budget check failed: {problem}")
        if problems:
            sys.exit(1)
        print("budget check passed")
        return
    if sys.platform != "linux" or not hasattr(signal, "pidfd_send_signal"):
        parser.error("measurements require Linux wait4/pidfd support and Python 3.9+")
    if not args.smoke and args.profile != "release":
        parser.error("full performance campaigns require the release profile")
    definitions = workloads(args.smoke)
    samples, warmups, wait_ms = (1, 0, 1) if args.smoke else (30, 3, 10)
    output_root = ROOT / "target/performance"
    output_root.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix="smoke-" if args.smoke else "campaign-", dir=output_root))
    sources = output / "inputs"
    sources.mkdir()
    for case in definitions:
        path = sources / (case["id"] + ".botwork")
        path.write_text(case["source"])
        case.update(source_path=str(path), source_bytes=path.stat().st_size, source_sha256=digest(path))
    record = {
        "schema": VERSION, "captured_at": datetime.now(timezone.utc).isoformat(),
        "kind": "smoke" if args.smoke else "measurement", "complete": False,
        "base_revision": capture("git", "rev-parse", "HEAD"),
        "worktree_status": capture("git", "status", "--porcelain"),
        "input_sha256": fingerprints(), "platform": platform.platform(),
        "rustc": capture("rustc", "-Vv"), "cargo": capture("cargo", "-V"),
        "python": sys.version, "target": args.target, "profile": args.profile,
        "machine": {"cpuinfo": Path("/proc/cpuinfo").read_text(),
                    "meminfo": Path("/proc/meminfo").read_text(),
                    "cpu_affinity": sorted(os.sched_getaffinity(0)),
                    "load_average_before": os.getloadavg(),
                    "libc": platform.libc_ver(),
                    "filesystem": capture("stat", "-f", "-c", "%T", str(output))},
        "build_environment": {key: os.environ[key] for key in
                              ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS",
                               "CARGO_INCREMENTAL", "CARGO_PROFILE_RELEASE_OPT_LEVEL",
                               "CARGO_PROFILE_RELEASE_LTO", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS")
                              if key in os.environ},
        "protocol": {"samples": samples, "warmups": warmups, "wait_ms": wait_ms,
                     "process_watchdog_seconds": 120, "steps_per_run": 64_000_000,
                     "other_run_limits": "RunLimits::default() at the recorded revision",
                     "percentiles": "nearest rank: sorted[ceil(p * n) - 1]",
                     "peak_memory": "Native parent wait4 ru_maxrss in KiB, whole workload child lifetime including setup and cleanup; excludes Python's inherited high-water mark",
                     "cache": "warm filesystem caches; inputs created before samples and pre-read by driver; no cache eviction",
                     "order": "one child at a time; fixed workload order within each round; all warmup rounds precede measured rounds",
                     "acceptance_budgets": str(BUDGETS.relative_to(ROOT)), "seed": None, "external_adapters": None},
        "workloads": [{k: v for k, v in case.items() if k != "source"} for case in definitions],
        "observations": [],
    }
    summary = output / "summary.json"
    summary.write_text(json.dumps(record, indent=2) + "\n")
    print(f"campaign record: {summary}", flush=True)
    try:
        command = ["cargo", "build", "--locked", "--bin", "botwork", "--bench", "runtime",
                   "--target", args.target, "--message-format=json-render-diagnostics"]
        if args.profile == "release":
            command.append("--release")
        build = execute(command, ROOT, output / "build", 600)
        record["build"] = build
        if build["returncode"] or build["timeout"]:
            raise ValueError("benchmark build failed; see build.stderr")
        artifacts = [json.loads(line) for line in Path(build["stdout"]).read_text().splitlines()]
        executables = {item["target"]["name"]: item["executable"] for item in artifacts
                       if item.get("reason") == "compiler-artifact" and item.get("executable")}
        if set(executables) != {"botwork", "runtime"}:
            raise ValueError("build did not produce exactly the CLI and workload driver")
        record["binaries"] = {name: {"path": path, "sha256": digest(Path(path))}
                              for name, path in executables.items()}
        record["calibration"] = calibrate(output, executables["runtime"])
        for phase, count in [("warmup", warmups), ("measured", samples)]:
            for index in range(count):
                for case in definitions:
                    command = ([executables["botwork"], "--file", case["source_path"]]
                               if case["id"] == "cli-startup" else
                               [executables["runtime"], case["id"], case["source_path"],
                                str(case["units"]), "--wait-ms", str(wait_ms)])
                    prefix = output / f"{phase}-{index:02}-{case['id']}"
                    row = measured(command, sources, prefix, executables["runtime"])
                    row.update(phase=phase, index=index, workload=case["id"], verified=False)
                    record["observations"].append(row)
                    row["workload_elapsed_ns"] = verify(row, case, Path(row["stdout"]).read_text(),
                                                        Path(row["stderr"]).read_text())
                    row["verified"] = True
                    summary.write_text(json.dumps(record, indent=2) + "\n")
                print(f"{phase} round {index + 1}/{count} verified", flush=True)
        record["statistics"] = aggregate(record["observations"], definitions, samples, warmups)
        if fingerprints() != record["input_sha256"]:
            raise ValueError("source changed during the campaign")
        for case in definitions:
            if digest(Path(case["source_path"])) != case["source_sha256"]:
                raise ValueError("workload input changed during the campaign")
        for binary in record["binaries"].values():
            if digest(Path(binary["path"])) != binary["sha256"]:
                raise ValueError("measured binary changed during the campaign")
        record["complete"] = True
    except Exception as error:
        record["error"] = str(error)
        raise
    finally:
        record["machine"]["load_average_after"] = os.getloadavg()
        summary.write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps(record["statistics"], indent=2), flush=True)


if __name__ == "__main__":
    main()
