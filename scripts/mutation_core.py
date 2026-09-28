#!/usr/bin/env python3
"""Run the fixed targeted core mutations, or audit a cargo-mutants output directory."""
import argparse
from collections import Counter
from datetime import datetime, timezone
import difflib
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parent.parent
CATALOGUE = ROOT / "tests" / "mutation-core.json"


def replace_once(source, mutation):
    before, after = mutation["before"], mutation["after"]
    if not before or before == after or source.count(before) != 1:
        raise ValueError(f"stale or ambiguous mutation: {mutation['id']}")
    return source.replace(before, after, 1)


def test_status(returncode, log):
    """Infrastructure errors and empty runs must never count as assertion kills."""
    if returncode == 0 and re.search(r"test result: ok\. [1-9][0-9]* passed;", log):
        return "missed"
    if returncode == 101 and re.search(r"test result: FAILED\. .*; [1-9][0-9]* failed;", log):
        return "caught"
    return "error"


def audit_generated(inventory, output):
    expected = [item["name"] for item in inventory]
    outcomes = json.loads((output / "outcomes.json").read_text())
    if not outcomes["end_time"] or outcomes["cargo_mutants_version"] != "27.1.0":
        raise ValueError("incomplete campaign or unexpected cargo-mutants version")
    baselines = [row for row in outcomes["outcomes"] if row["scenario"] == "Baseline"]
    if len(baselines) != 1 or baselines[0]["summary"] != "Success":
        raise ValueError("a successful baseline is required")
    rows = [row for row in outcomes["outcomes"] if row["scenario"] != "Baseline"]
    names = [row["scenario"]["Mutant"]["name"] for row in rows]
    if not expected or len(set(expected)) != len(expected) or Counter(names) != Counter(expected):
        raise ValueError("outcomes do not exactly cover the frozen inventory")
    counts = Counter(row["summary"] for row in rows)
    fields = {"CaughtMutant": "caught", "MissedMutant": "missed", "Unviable": "unviable", "Timeout": "timeout"}
    if (set(counts) - set(fields) or outcomes["total_mutants"] != len(rows)
            or any(outcomes[field] != counts[status] for status, field in fields.items())):
        raise ValueError("inconsistent campaign totals or unknown outcome")
    baseline = baselines[0]
    phases = {phase["phase"]: phase["process_status"] for phase in baseline["phase_results"]}
    if (phases != {"Build": "Success", "Test": "Success"}
            or test_status(0, (output / baseline["log_path"]).read_text()) != "missed"):
        raise ValueError("baseline has no passing tests")
    for row in rows:
        phases = {phase["phase"]: phase["process_status"] for phase in row["phase_results"]}
        if row["summary"] in ("CaughtMutant", "MissedMutant"):
            if phases.get("Build") != "Success":
                raise ValueError("test outcome without a successful build")
            test = phases.get("Test")
            code = 0 if test == "Success" else test.get("Failure") if isinstance(test, dict) else None
            status = test_status(code, (output / row["log_path"]).read_text())
            expected_status = "caught" if row["summary"] == "CaughtMutant" else "missed"
            if status != expected_status:
                raise ValueError(f"unverified test outcome: {row['scenario']}")
        elif row["summary"] == "Unviable":
            build = phases.get("Build")
            if not isinstance(build, dict) or not build.get("Failure") or "Test" in phases:
                raise ValueError("unviable outcome without a failed build")
        elif "Timeout" not in phases.values():
            raise ValueError("timeout outcome without a timed out phase")
    return dict(counts)


def execute(command, cwd, log, timeout):
    start = time.monotonic()
    with log.open("w") as stream:
        child = subprocess.Popen(command, cwd=cwd, stdout=stream, stderr=subprocess.STDOUT,
                                 env=dict(os.environ, CARGO_NET_OFFLINE="true"), start_new_session=True)
        try:
            code = child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
            code = None
        except BaseException:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
            child.wait()
            raise
    return {"command": command, "returncode": code, "timeout": code is None,
            "elapsed_seconds": round(time.monotonic() - start, 3), "log": log.name,
            "log_sha256": hashlib.sha256(log.read_bytes()).hexdigest()}


def capture(*command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def verify_exit(execution, counts):
    expected = 3 if counts.get("Timeout", 0) else 2 if counts.get("MissedMutant", 0) else 0
    if execution["timeout"] or execution["returncode"] != expected:
        raise ValueError("cargo-mutants did not exit consistently with its audited results")


def snapshot_campaign(kind):
    if sys.platform != "linux":
        raise ValueError("this campaign runner currently requires Linux")
    output_root = ROOT / "target" / "mutation-core"
    output_root.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix=f"{kind}-", dir=output_root))
    snapshot = output / "source"
    snapshot.mkdir()
    # Git's inventory excludes build products and .git; copy uncommitted tests too.
    names = subprocess.check_output(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], cwd=ROOT)
    fingerprints = {}
    for name in sorted(set(os.fsdecode(name) for name in names.split(b"\0") if name)):
        source = ROOT / name
        if not source.is_file() or source.is_symlink():
            raise ValueError(f"snapshot requires an ordinary file: {name}")
        destination = snapshot / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        fingerprints[name] = hashlib.sha256(destination.read_bytes()).hexdigest()
    record = {"schema": 1, "captured_at": datetime.now(timezone.utc).isoformat(),
              "base_revision": capture("git", "rev-parse", "HEAD"),
              "worktree_status": capture("git", "status", "--porcelain"),
              "input_sha256": fingerprints, "platform": platform.platform(),
              "rustc": capture("rustc", "-Vv"), "cargo": capture("cargo", "-V"),
              "build_timeout_seconds": 180, "test_timeout_seconds": 60,
              "outcomes": [], "complete": False}
    print(f"campaign record: {output / 'summary.json'}", flush=True)
    return output, snapshot, record


def verify_snapshot(snapshot, record):
    for name, digest in record["input_sha256"].items():
        if hashlib.sha256((snapshot / name).read_bytes()).hexdigest() != digest:
            raise ValueError(f"snapshot changed unexpectedly: {name}")


def generated(name=None):
    if capture("cargo", "mutants", "--version") != "cargo-mutants 27.1.0":
        raise ValueError("install cargo-mutants 27.1.0 with --locked")
    output, snapshot, record = snapshot_campaign("generated")
    try:
        inventory = subprocess.check_output(["cargo", "mutants", "--list", "--json"], cwd=snapshot)
        selection = []
        if name is not None:
            full = json.loads(inventory)
            matches = [index for index, item in enumerate(full) if item["name"] == name]
            if len(matches) != 1:
                raise ValueError("replay name must match exactly one generated mutation")
            (output / "full-inventory.json").write_bytes(inventory)
            # Shard after discovery; do not broaden the selection with config regexes.
            selection = ["--shard", f"{matches[0]}/{len(full)}"]
            inventory = subprocess.check_output(["cargo", "mutants", "--list", "--json", *selection], cwd=snapshot)
            if json.loads(inventory) != [full[matches[0]]]:
                raise ValueError("shard did not select exactly the requested mutation")
            record["replay_name"] = name
        (output / "inventory.json").write_bytes(inventory)
        command = ["cargo", "mutants", "--jobs", "2", "--build-timeout", "180", "--timeout", "60",
                   "--output", str(output), "--caught", "--unviable", *selection]
        record["execution"] = execute(command, snapshot, output / "run.log", 3600)
        record["counts"] = audit_generated(json.loads(inventory), output / "mutants.out")
        verify_exit(record["execution"], record["counts"])
        verify_snapshot(snapshot, record)
        record["complete"] = True
        print(json.dumps(record["counts"]), flush=True)
        return (record["counts"].get("CaughtMutant", 0) > 0
                and not (record["counts"].get("MissedMutant", 0) or record["counts"].get("Timeout", 0)))
    finally:
        (output / "summary.json").write_text(json.dumps(record, indent=2) + "\n")


def targeted():
    output, snapshot, record = snapshot_campaign("targeted")
    catalogue = json.loads((snapshot / CATALOGUE.relative_to(ROOT)).read_text())
    mutations = catalogue["mutations"]
    if not mutations or len({item["id"] for item in mutations}) != len(mutations):
        raise ValueError("empty catalogue or duplicate mutation IDs")
    for item in mutations:
        if not re.fullmatch(r"[a-z0-9-]+", item["id"]):
            raise ValueError("mutation ID must be a lowercase filename component")
        path = Path(item["file"])
        if path.is_absolute() or ".." in path.parts or path.parts[0] != "src":
            raise ValueError("mutation path must be inside src")
        replace_once((snapshot / path).read_text(), item)
    config = tomllib.loads((snapshot / ".cargo/mutants.toml").read_text())
    cargo = ["cargo", "test", *config["additional_cargo_args"]]
    record["catalogue_version"] = catalogue["version"]
    try:
        for item in [None, *mutations]:
            name = item["id"] if item else "baseline"
            row = {"id": name, "status": "incomplete"}
            record["outcomes"].append(row)
            path = snapshot / item["file"] if item else None
            original = path.read_text() if path else None
            try:
                if item:
                    changed = replace_once(original, item)
                    path.write_text(changed)
                    diff = "".join(difflib.unified_diff(original.splitlines(True), changed.splitlines(True),
                                                      fromfile=item["file"], tofile=item["file"]))
                    (output / f"{name}.diff").write_text(diff)
                row["build"] = execute([*cargo, "--no-run"], snapshot, output / f"{name}-build.log", 180)
                build = row["build"]
                if build["timeout"]:
                    row["status"] = "build_timeout"
                elif build["returncode"]:
                    row["status"] = "unviable"
                else:
                    row["test"] = execute(cargo, snapshot, output / f"{name}-test.log", 60)
                    test = row["test"]
                    row["status"] = ("timeout" if test["timeout"] else
                                     test_status(test["returncode"], (output / test["log"]).read_text()))
                if item is None:
                    if row["status"] != "missed":
                        raise ValueError("baseline failed; inspect its build/test logs")
                    row["status"] = "passed"
                print(f"{name}: {row['status']}", flush=True)
                if row["status"] == "error":
                    raise ValueError(f"infrastructure failure: {name}")
            finally:
                if path:
                    path.write_text(original)
                (output / "summary.json").write_text(json.dumps(record, indent=2) + "\n")
        verify_snapshot(snapshot, record)
        record["complete"] = True
        return all(row["status"] == "caught" for row in record["outcomes"][1:])
    finally:
        (output / "summary.json").write_text(json.dumps(record, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("targeted", help="test every catalogue entry in an isolated source copy")
    generated_parser = commands.add_parser("generated", help="freeze and test every generated core mutation")
    generated_parser.add_argument("--name", help="replay exactly one name from the frozen inventory")
    audit = commands.add_parser("audit", help="reject missing mutants and unproven test kills")
    audit.add_argument("inventory", type=Path)
    audit.add_argument("output", type=Path, help="the generated mutants.out directory")
    args = parser.parse_args()
    if args.command == "targeted":
        sys.exit(0 if targeted() else 1)
    elif args.command == "generated":
        sys.exit(0 if generated(args.name) else 1)
    else:
        print(json.dumps(audit_generated(json.loads(args.inventory.read_text()), args.output), indent=2))


if __name__ == "__main__":
    main()
