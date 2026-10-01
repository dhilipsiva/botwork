"""The WebAssembly component the WASM adapter's tests call (decision D8).

Usage: wasm_guest.py COMMAND
  build [OUT]   build tests/wasm/guest for wasm32-wasip2 and copy the component
                to OUT; without OUT, replace tests/wasm/statements.wasm and
                record its sources' and its own SHA-256 with the toolchain in
                tests/wasm/statements.json
  check         fail unless the recorded hashes match the sources and the
                committed component, so a change to either needs a rebuild
  example BOTWORK
                build examples/wasm/greeter and run its greet.botwork with the
                BOTWORK executable, which must log `Hello, Ada`

Builds need `rustup target add wasm32-wasip2`. The component can differ between
toolchains, so CI checks the record, then builds afresh and runs the tests
against that build (BOTWORK_WASM_GUEST names it).
"""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GUEST = Path("tests/wasm/guest")
SOURCES = [
    Path("wit/botwork.wit"),
    GUEST / "Cargo.toml",
    GUEST / "Cargo.lock",
    GUEST / "src/lib.rs",
]
COMPONENT = Path("tests/wasm/statements.wasm")
RECORD = Path("tests/wasm/statements.json")
TARGET = Path("target/wasm-guest")
BUILT = TARGET / "wasm32-wasip2/release/botwork_test_statements.wasm"
GREETER = Path("examples/wasm/greeter")
GREETER_BUILT = TARGET / "wasm32-wasip2/release/botwork_greeter.wasm"


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sources(root):
    return {str(path): sha256(root / path) for path in SOURCES}


def cargo_build(root, crate):
    environment = dict(os.environ, CARGO_TARGET_DIR=str(root / TARGET))
    subprocess.run(
        [
            "cargo", "build", "--locked", "--release", "--target", "wasm32-wasip2",
            "--manifest-path", str(root / crate / "Cargo.toml"),
        ],
        check=True,
        env=environment,
    )


def build(root, out=None):
    cargo_build(root, GUEST)
    if out is not None:
        shutil.copyfile(root / BUILT, out)
        return
    shutil.copyfile(root / BUILT, root / COMPONENT)
    record = {
        "sources": sources(root),
        "component": sha256(root / COMPONENT),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
    }
    (root / RECORD).write_text(json.dumps(record, indent=2) + "\n")


def problems(root):
    """Why the record does not describe the sources and component, if it does not."""
    record = json.loads((root / RECORD).read_text())
    found = []
    for path, digest in sources(root).items():
        if record["sources"].get(path) != digest:
            found.append(f"{path} changed since the component was built")
    if set(record["sources"]) != {str(path) for path in SOURCES}:
        found.append("the record lists other sources")
    if record["component"] != sha256(root / COMPONENT):
        found.append(f"{COMPONENT} is not the component the record describes")
    return found


def example(root, botwork):
    """Build the greeter and run its script; the output, or None if it ran as documented."""
    cargo_build(root, GREETER)
    with tempfile.TemporaryDirectory() as directory:
        shutil.copyfile(root / GREETER_BUILT, Path(directory) / "greeter.wasm")
        shutil.copyfile(root / GREETER / "greet.botwork", Path(directory) / "greet.botwork")
        output = subprocess.run(
            [botwork, "--file", "greet.botwork"], cwd=directory, capture_output=True, text=True
        )
    if output.returncode == 0 and output.stdout == "Hello, Ada\n":
        return None
    return f"exit {output.returncode}\n{output.stdout}{output.stderr}"


def main(arguments, root=ROOT):
    if arguments[:1] == ["build"] and len(arguments) <= 2:
        build(root, Path(arguments[1]).resolve() if len(arguments) == 2 else None)
        return 0
    if arguments[:1] == ["example"] and len(arguments) == 2:
        failure = example(root, str(Path(arguments[1]).resolve()))
        if failure is not None:
            print(f"examples/wasm/greeter did not log `Hello, Ada`: {failure}", file=sys.stderr)
        return 1 if failure is not None else 0
    if arguments == ["check"]:
        found = problems(root)
        for problem in found:
            print(f"{problem}; run `python3 scripts/wasm_guest.py build`", file=sys.stderr)
        return 1 if found else 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
