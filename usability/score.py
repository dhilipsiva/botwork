#!/usr/bin/env python3
"""Score a completed usability sheet against the registered thresholds.

    python3 usability/score.py SHEET.csv [--json RESULT.json]

The sheet has the columns of `sheet.csv`, one row per participant and task.
Prints each task's unaided success by cohort and each threshold's verdict, then
exits 0 when every threshold holds, 1 when one does not, and 2 when the sheet
is invalid. Retest rows are scored separately and never replace originals.
"""
import argparse
import csv
import json
from pathlib import Path
import statistics
import sys

TASKS = ("U1", "U2", "U3", "U4", "U5", "U6")
COHORTS = ("newcomer", "experienced")
ROUNDS = ("original", "retest")
OUTCOMES = ("success", "failure", "abandoned", "timeout")
COLUMNS = ("participant", "cohort", "round", "task", "seconds", "outcome", "assisted", "checked",
           "explanation_ok", "notes")
LIMIT_SECONDS = 20 * 60
# docs/quality-assessment.md: the registered thresholds.
MINIMUM_PARTICIPANTS, MINIMUM_PER_COHORT = 10, 5
TASK_SUCCESS = 0.9
U2_MEDIAN_SECONDS = 10 * 60
U6_WITHIN_SECONDS, U6_SHARE = 5 * 60, 0.8


class InvalidSheet(ValueError):
    pass


def rows_from(path):
    with open(path, newline="") as sheet:
        reader = csv.DictReader(sheet)
        if tuple(reader.fieldnames or ()) != COLUMNS:
            raise InvalidSheet(f"columns must be {', '.join(COLUMNS)}")
        rows = list(reader)
    for number, row in enumerate(rows, start=2):
        for column, allowed in (("cohort", COHORTS), ("round", ROUNDS), ("task", TASKS), ("outcome", OUTCOMES),
                                ("assisted", ("yes", "no")), ("checked", ("pass", "fail"))):
            if row[column] not in allowed:
                raise InvalidSheet(f"line {number}: {column} must be one of {', '.join(allowed)}")
        if row["explanation_ok"] not in (("yes", "no") if row["task"] == "U1" else ("",)):
            raise InvalidSheet(f"line {number}: explanation_ok is yes or no for U1 and empty otherwise")
        try:
            row["seconds"] = float(row["seconds"])
        except ValueError:
            raise InvalidSheet(f"line {number}: seconds must be a number") from None
        if not 0 <= row["seconds"] <= LIMIT_SECONDS:
            raise InvalidSheet(f"line {number}: seconds must be within the {LIMIT_SECONDS}-second task limit")
        if row["outcome"] == "success" and row["checked"] != "pass":
            raise InvalidSheet(f"line {number}: a success must pass its automated check")
    return rows


def unaided(row):
    """Success without assistance, checked, and for U1 with correct explanations."""
    return (row["outcome"] == "success" and row["assisted"] == "no" and row["checked"] == "pass"
            and (row["task"] != "U1" or row["explanation_ok"] == "yes"))


def score_round(rows):
    participants = {}
    for row in rows:
        known = participants.setdefault(row["participant"], {"cohort": row["cohort"], "tasks": {}})
        if known["cohort"] != row["cohort"]:
            raise InvalidSheet(f"participant {row['participant']} has more than one cohort")
        if row["task"] in known["tasks"]:
            raise InvalidSheet(f"participant {row['participant']} has more than one {row['task']} row in this round")
        known["tasks"][row["task"]] = row
    tasks = {}
    for task in TASKS:
        attempts = [p["tasks"][task] for p in participants.values() if task in p["tasks"]]
        cohorts = {cohort: [row for row in attempts if row["cohort"] == cohort] for cohort in COHORTS}
        tasks[task] = {"attempts": len(attempts), "unaided": sum(map(unaided, attempts)),
                       "cohorts": {cohort: {"attempts": len(group), "unaided": sum(map(unaided, group))}
                                   for cohort, group in cohorts.items()}}
    return participants, tasks


def thresholds(participants, tasks):
    counts = {cohort: sum(p["cohort"] == cohort for p in participants.values()) for cohort in COHORTS}
    missing = sorted(f"{name}/{task}" for name, p in participants.items() for task in TASKS if task not in p["tasks"])
    verdicts = [{"threshold": f"at least {MINIMUM_PARTICIPANTS} participants, {MINIMUM_PER_COHORT} per cohort, every task attempted",
                 "value": {"participants": len(participants), **counts, "missing": missing},
                 "passed": len(participants) >= MINIMUM_PARTICIPANTS and not missing
                 and all(count >= MINIMUM_PER_COHORT for count in counts.values())}]
    for task, result in tasks.items():
        rate = result["unaided"] / result["attempts"] if result["attempts"] else 0.0
        verdicts.append({"threshold": f"{task}: at least {TASK_SUCCESS:.0%} unaided success",
                         "value": round(rate, 4), "passed": result["attempts"] > 0 and rate >= TASK_SUCCESS})
    # Unsuccessful U2 attempts count as the task limit.
    u2 = [p["tasks"]["U2"] for p in participants.values() if "U2" in p["tasks"]]
    median = statistics.median(row["seconds"] if unaided(row) else LIMIT_SECONDS for row in u2) if u2 else None
    verdicts.append({"threshold": f"U2: median time to a first passing script at most {U2_MEDIAN_SECONDS} s",
                     "value": median, "passed": median is not None and median <= U2_MEDIAN_SECONDS})
    fast = sum(unaided(p["tasks"]["U6"]) and p["tasks"]["U6"]["seconds"] <= U6_WITHIN_SECONDS
               for p in participants.values() if "U6" in p["tasks"])
    share = fast / len(participants) if participants else 0.0
    verdicts.append({"threshold": f"U6: at least {U6_SHARE:.0%} of participants repair within {U6_WITHIN_SECONDS} s",
                     "value": round(share, 4), "passed": share >= U6_SHARE})
    return verdicts


def score(rows):
    result = {}
    for round_ in ROUNDS:
        selected = [row for row in rows if row["round"] == round_]
        if not selected:
            continue
        participants, tasks = score_round(selected)
        result[round_] = {"tasks": tasks}
        if round_ == "original":
            result[round_]["thresholds"] = thresholds(participants, tasks)
    if "original" not in result:
        raise InvalidSheet("the sheet has no original round")
    result["passed"] = all(verdict["passed"] for verdict in result["original"]["thresholds"])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("sheet", type=Path)
    parser.add_argument("--json", type=Path, help="also write the result as JSON")
    args = parser.parse_args()
    try:
        result = score(rows_from(args.sheet))
    except InvalidSheet as error:
        print(f"invalid sheet: {error}", file=sys.stderr)
        sys.exit(2)
    for round_, scored in result.items():
        if round_ == "passed":
            continue
        print(f"{round_} round")
        for task, counts in scored["tasks"].items():
            cohorts = ", ".join(f"{cohort} {c['unaided']}/{c['attempts']}" for cohort, c in counts["cohorts"].items())
            print(f"  {task}: {counts['unaided']}/{counts['attempts']} unaided ({cohorts})")
        for verdict in scored.get("thresholds", []):
            print(f"  {'pass' if verdict['passed'] else 'FAIL'}: {verdict['threshold']} ({verdict['value']})")
    if args.json:
        args.json.write_text(json.dumps(result, indent=2) + "\n")
    print("all thresholds hold" if result["passed"] else "thresholds not met")
    sys.exit(0 if result["passed"] else 1)


if __name__ == "__main__":
    main()
