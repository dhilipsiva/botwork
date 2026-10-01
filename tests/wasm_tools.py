import json
import runpy
import shutil
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GUEST = runpy.run_path(ROOT / "scripts/wasm_guest.py")


class Check(unittest.TestCase):
    """The record must describe the sources and the committed component."""

    def copy(self):
        """A temporary copy of the files the record describes."""
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        for path in [*GUEST["SOURCES"], GUEST["COMPONENT"], GUEST["RECORD"]]:
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / path, root / path)
        return root

    def test_the_committed_record_matches(self):
        self.assertEqual(GUEST["problems"](ROOT), [])
        self.assertEqual(GUEST["main"](["check"]), 0)

    def test_a_changed_source_needs_a_rebuild(self):
        root = self.copy()
        source = root / "tests/wasm/guest/src/lib.rs"
        source.write_text(source.read_text() + "\n// changed\n")
        self.assertEqual(
            GUEST["problems"](root),
            ["tests/wasm/guest/src/lib.rs changed since the component was built"],
        )
        self.assertEqual(GUEST["main"](["check"], root=root), 1)

    def test_a_changed_interface_needs_a_rebuild(self):
        root = self.copy()
        (root / "wit/botwork.wit").write_text("package botwork:other;\n")
        self.assertEqual(
            GUEST["problems"](root), ["wit/botwork.wit changed since the component was built"]
        )

    def test_a_replaced_component_is_noticed(self):
        root = self.copy()
        (root / "tests/wasm/statements.wasm").write_bytes(b"\0asm")
        self.assertEqual(
            GUEST["problems"](root),
            ["tests/wasm/statements.wasm is not the component the record describes"],
        )

    def test_a_record_naming_other_sources_is_refused(self):
        root = self.copy()
        record = json.loads((root / "tests/wasm/statements.json").read_text())
        record["sources"]["extra.rs"] = "0" * 64
        (root / "tests/wasm/statements.json").write_text(json.dumps(record))
        self.assertEqual(GUEST["problems"](root), ["the record lists other sources"])

    def test_unknown_commands_print_the_usage(self):
        self.assertEqual(GUEST["main"]([]), 2)
        self.assertEqual(GUEST["main"](["build", "a", "b"]), 2)
        self.assertEqual(GUEST["main"](["example"]), 2)


if __name__ == "__main__":
    unittest.main()
