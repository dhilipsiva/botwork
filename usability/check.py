#!/usr/bin/env python3
"""Check a participant's result for one registered usability task.

    python3 usability/check.py TASK FOLDER [--botwork PATH]

TASK is U1 to U6 and FOLDER is the participant's folder for it, as
`prepare.py` created it. Prints a JSON verdict and exits 0 when the task's
success criteria hold, 1 when they do not. U1's explanations are scored by the
facilitator; see docs/usability-study.md.
"""
import argparse
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile

KIT = Path(__file__).resolve().parent
TASKS = ("U1", "U2", "U3", "U4", "U5", "U6")
TIMEOUT_SECONDS = 60
U1_OUTPUT = ["12", "low", "14", "high", "0", "low", "99"]
CALL_MARKER = "usability-check: pricing Total of called"


def code(text):
    """The source without comments and string contents, for counting operators."""
    out, index, in_string = [], 0, False
    while index < len(text):
        character = text[index]
        if in_string:
            if character == "\\":
                index += 2
                continue
            in_string = character != '"'
        elif character == '"':
            in_string = True
            out.append('""')
        elif character == "#":
            while index < len(text) and text[index] != "\n":
                index += 1
            continue
        else:
            out.append(character)
        index += 1
    return "".join(out)


def normalized(text):
    return " ".join(text.split())


def run(botwork, arguments, folder):
    try:
        result = subprocess.run([botwork, *arguments], cwd=folder, capture_output=True, text=True,
                                timeout=TIMEOUT_SECONDS, stdin=subprocess.DEVNULL)
    except subprocess.TimeoutExpired:
        return None, "", f"timed out after {TIMEOUT_SECONDS} seconds"
    return result.returncode, result.stdout, result.stderr


class Verdict:
    def __init__(self, task):
        self.task, self.reasons, self.details = task, [], {}

    def require(self, condition, reason):
        if not condition:
            self.reasons.append(reason)
        return condition

    def ran(self, label, outcome, status, stdout):
        """Require an exit status and exact standard output from a run."""
        actual, out, err = outcome
        self.details[label] = {"status": actual, "stdout": out, "stderr": err[-2000:]}
        self.require(actual == status, f"{label} exited with {actual}, expected {status}")
        self.require(out == stdout, f"{label} printed {out!r}, expected {stdout!r}")

    def result(self):
        return {"task": self.task, "passed": not self.reasons, "reasons": self.reasons, "details": self.details}


def read(verdict, path):
    if not verdict.require(path.is_file(), f"{path.name} is missing"):
        return None
    return path.read_text()


def u1(folder, botwork, verdict):
    answer = read(verdict, folder / "answer.txt")
    if answer is not None:
        predicted = [line.strip() for line in answer.splitlines() if line.strip()]
        verdict.details["predicted"] = predicted
        verdict.require(predicted == U1_OUTPUT, f"predicted {predicted}, but the script prints {U1_OUTPUT}")


def u2(folder, botwork, verdict):
    text = read(verdict, folder / "first.botwork")
    if text is None:
        return
    source = code(text)
    verdict.require(re.search(r"\|\s*quantity\s*\|\s*=\s*\|\s*3\s*\|", source), "quantity is not set to 3")
    verdict.require(re.search(r"\|\s*price\s*\|\s*=\s*\|\s*4\s*\|", source), "price is not set to 4")
    verdict.require(re.search(r"\bquantity\s*\*\s*price\b|\bprice\s*\*\s*quantity\b", source),
                    "the product is not calculated from quantity and price")
    verdict.ran("run", run(botwork, ["--file", "first.botwork"], folder), 0, "12\n")


def u3(folder, botwork, verdict):
    text = read(verdict, folder / "reuse.botwork")
    if text is None:
        return
    source = code(text)
    verdict.require(re.search(r"\|\s*quantity\s*\|\s*=\s*\|\s*99\s*\|", source), "the caller's quantity is no longer set to 99")
    verdict.require(source.count("*") == 1, f"the script multiplies {source.count('*')} times; one shared statement should do it once")
    verdict.ran("run", run(botwork, ["--file", "reuse.botwork"], folder), 0, "12\n14\n99\n")


def u4(folder, botwork, verdict):
    text = read(verdict, folder / "compose.botwork")
    helper = read(verdict, folder / "pricing.botwork")
    if text is None or helper is None:
        return
    original = (KIT / "tasks/U4/pricing.botwork").read_text()
    verdict.require(normalized(helper) == normalized(original), "pricing.botwork was changed")
    verdict.require("*" not in code(text), "compose.botwork multiplies itself instead of using the supplied statement")
    verdict.ran("run", run(botwork, ["--file", "compose.botwork"], folder), 0, "true\n")
    # Count calls with a copy of the helper that logs each one.
    counted = original.replace("    Return |quantity * price|", f'    Log |"{CALL_MARKER}"|\n    Return |quantity * price|')
    with tempfile.TemporaryDirectory() as directory:
        Path(directory, "compose.botwork").write_text(text)
        Path(directory, "pricing.botwork").write_text(counted)
        status, out, err = run(botwork, ["--file", "compose.botwork"], directory)
    lines = out.splitlines()
    calls = lines.count(CALL_MARKER)
    verdict.details["counted_run"] = {"status": status, "calls": calls}
    verdict.require(calls == 2, f"the supplied statement ran {calls} times, expected 2")
    verdict.require([line for line in lines if line != CALL_MARKER] == ["true"] and status == 0,
                    "the counted run did not print only true")


def report(botwork, arguments, folder, label, verdict):
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory, "report.json")
        status, out, err = run(botwork, [*arguments, "--report-json", str(path)], folder)
        verdict.details[label] = {"arguments": arguments, "status": status, "stderr": err[-2000:]}
        if not verdict.require(path.is_file(), f"{label} wrote no report"):
            return status, []
        runs = json.loads(path.read_text())["runs"]
    verdict.details[label]["cases"] = {entry["identity"]["id"]: entry["status"] for entry in runs}
    return status, runs


def u5(folder, botwork, verdict):
    if read(verdict, folder / "totals.suite.botwork") is None:
        return
    status, runs = report(botwork, ["--suite", "totals.suite.botwork", "--jobs", "1"], folder, "first_run", verdict)
    verdict.require(status == 1, f"the first run exited with {status}, expected 1 for a failure")
    rows = [entry for entry in runs if entry["identity"]["id"].startswith("pricing/total/")]
    verdict.require(len(rows) == 3 and len(runs) == 3, f"the first run had {len(runs)} cases, expected the three rows of pricing/total")
    failed = [entry for entry in rows if entry["status"] == "failed"]
    passed = [entry for entry in rows if entry["status"] == "succeeded"]
    if not verdict.require(len(failed) == 1 and len(passed) == 2, "expected two rows to pass and one to fail"):
        return
    # Either operand order: the assertion compares 14 with 99.
    message = (failed[0].get("error") or {}).get("message", "")
    compared = re.search(r"Expected (-?\d+) \(Int\), got (-?\d+) \(Int\)", message)
    verdict.require(compared is not None and set(compared.groups()) == {"14", "99"},
                    "the failing row is not the 2 x 7 row expected to total 99")
    arguments = read(verdict, folder / "rerun.args")
    if arguments is None:
        return
    words = shlex.split(arguments)
    if words[:1] == ["botwork"]:
        words = words[1:]
    # The check writes its own report; drop one the participant asked for.
    for option in ("--report-json", "--report-html", "--failures"):
        while option in words and words.index(option) + 1 < len(words):
            index = words.index(option)
            del words[index:index + 2]
    status, again = report(botwork, words, folder, "rerun", verdict)
    verdict.require([entry["identity"]["id"] for entry in again] == [failed[0]["identity"]["id"]],
                    f"the rerun selected {[entry['identity']['id'] for entry in again]}, expected only {failed[0]['identity']['id']}")
    verdict.require(status == 1, f"the rerun exited with {status}, expected 1 for the same failure")


def u6(folder, botwork, verdict):
    main = read(verdict, folder / "main.botwork")
    helper = read(verdict, folder / "pricing.botwork")
    if main is None or helper is None:
        return
    original_main = (KIT / "tasks/U6/main.botwork").read_text()
    original = (KIT / "tasks/U6/pricing.botwork").read_text()
    repaired = original.replace("discont", "discount")
    verdict.require(normalized(main) == normalized(original_main), "main.botwork was changed")
    if normalized(helper) == normalized(original):
        verdict.reasons.append("the misspelled reference in pricing.botwork is unchanged")
    else:
        verdict.require(normalized(helper) == normalized(repaired),
                        "pricing.botwork differs from the original by more than the misspelled reference")
    verdict.ran("run", run(botwork, ["--file", "main.botwork"], folder), 0, "10\n")


def check(task, folder, botwork):
    verdict = Verdict(task)
    {"U1": u1, "U2": u2, "U3": u3, "U4": u4, "U5": u5, "U6": u6}[task](Path(folder), botwork, verdict)
    return verdict.result()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("task", choices=TASKS)
    parser.add_argument("folder", type=Path)
    parser.add_argument("--botwork", default="botwork", help="the botwork executable (default: botwork on PATH)")
    args = parser.parse_args()
    result = check(args.task, args.folder.resolve(), args.botwork)
    print(json.dumps(result, indent=2))
    sys.exit(0 if result["passed"] else 1)


if __name__ == "__main__":
    main()
