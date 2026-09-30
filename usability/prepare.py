#!/usr/bin/env python3
"""Create a participant's workspace for the usability study.

    python3 usability/prepare.py PARTICIPANT FOLDER

PARTICIPANT is the participant's number, from 1. FOLDER must not exist yet; it
receives one folder per task with its prompt and starter files, `order.txt`
with this participant's task order, and `kit.json` identifying the kit.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

KIT = Path(__file__).resolve().parent
# U2 measures the first script, so it precedes the other writing tasks. U1 and
# U6 swap between first and last, and U3 to U5 rotate between them.
ORDERS = [
    [opening, "U2", *middle, closing]
    for opening, closing in (("U1", "U6"), ("U6", "U1"))
    for middle in (("U3", "U4", "U5"), ("U4", "U5", "U3"), ("U5", "U3", "U4"))
]


def order(participant):
    return ORDERS[(participant - 1) % len(ORDERS)]


def kit_files():
    return {str(path.relative_to(KIT)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted((KIT / "tasks").rglob("*")) if path.is_file()}


def prepare(participant, folder):
    if participant < 1:
        raise ValueError("participants are numbered from 1")
    folder.mkdir(parents=True)
    for task in order(participant):
        shutil.copytree(KIT / "tasks" / task, folder / task)
    (folder / "order.txt").write_text("\n".join(order(participant)) + "\n")
    revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=KIT, capture_output=True, text=True)
    (folder / "kit.json").write_text(json.dumps({
        "participant": participant, "order": order(participant),
        "revision": revision.stdout.strip() or None, "files": kit_files(),
    }, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("participant", type=int)
    parser.add_argument("folder", type=Path)
    args = parser.parse_args()
    prepare(args.participant, args.folder)
    print(f"prepared {args.folder} for participant {args.participant}: {' '.join(order(args.participant))}")


if __name__ == "__main__":
    main()
