import hashlib
import json
import os
import re
import runpy
import subprocess
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
RELEASE = runpy.run_path(ROOT / "scripts/release.py")
WORKFLOW = (ROOT / ".github/workflows/release.yml").read_text()


class Release(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.path = Path(self.directory.name)

    def tearDown(self):
        self.directory.cleanup()

    def project(self, cargo, extension):
        root = self.path / "project"
        (root / "editors/vscode").mkdir(parents=True)
        (root / "Cargo.toml").write_text(f'[package]\nname = "botwork"\nversion = "{cargo}"\n')
        (root / "editors/vscode/package.json").write_text(json.dumps({"version": extension}))
        (root / "LICENSE").write_text("license")
        (root / "README.md").write_text("readme")
        return root

    def assets(self, release="1.2.3"):
        """Archives of a stand-in binary for every target, with SHA256SUMS."""
        root = self.project(release, release)
        binary = self.path / "botwork"
        binary.write_bytes(b"\x7fELF binary")
        assets = self.path / "assets"
        for target in RELEASE["TARGETS"]:
            RELEASE["package"](release, target, binary, assets, root)
        RELEASE["checksums"](assets)
        return assets

    def test_only_a_tag_naming_the_shared_version_publishes(self):
        root = self.project("1.2.3", "1.2.3")
        self.assertEqual(RELEASE["version"]("refs/tags/v1.2.3", root), {"version": "1.2.3", "publish": "true"})
        self.assertEqual(RELEASE["version"]("refs/heads/main", root), {"version": "1.2.3", "publish": "false"})
        for tag in ("refs/tags/v1.2.4", "refs/tags/1.2.3", "refs/tags/v1.2.3-rc1"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                RELEASE["version"](tag, root)

    def test_the_extension_and_crate_versions_must_agree(self):
        root = self.project("1.2.3", "1.2.2")
        for ref in ("refs/tags/v1.2.3", "refs/heads/main"):
            with self.subTest(ref=ref), self.assertRaises(ValueError):
                RELEASE["version"](ref, root)

    def test_the_checked_in_versions_agree(self):
        cargo, extension = RELEASE["versions"]()
        self.assertEqual(cargo, extension)

    def test_archives_hold_the_binary_license_and_readme_reproducibly(self):
        root = self.project("1.2.3", "1.2.3")
        binary = self.path / "botwork"
        binary.write_bytes(b"binary bytes")
        for target, kind in RELEASE["TARGETS"].items():
            with self.subTest(target=target):
                first = RELEASE["package"]("1.2.3", target, binary, self.path / "one", root)
                second = RELEASE["package"]("1.2.3", target, binary, self.path / "two", root)
                self.assertEqual(first.name, f"botwork-v1.2.3-{target}.{kind}")
                self.assertEqual(first.read_bytes(), second.read_bytes())
                if kind == "zip":
                    with zipfile.ZipFile(first) as archive:
                        self.assertEqual(archive.namelist(), ["botwork.exe", "LICENSE", "README.md"])
                        self.assertEqual(archive.read("botwork.exe"), b"binary bytes")
                        self.assertEqual(archive.getinfo("botwork.exe").external_attr >> 16, 0o100755)
                        self.assertEqual({info.create_system for info in archive.infolist()}, {3})
                else:
                    with tarfile.open(first) as archive:
                        members = archive.getmembers()
                        self.assertEqual([member.name for member in members], ["botwork", "LICENSE", "README.md"])
                        self.assertEqual([member.mode for member in members], [0o755, 0o644, 0o644])
                        self.assertEqual({(member.uid, member.gid, member.uname) for member in members}, {(0, 0, "")})
                        self.assertEqual(archive.extractfile("botwork").read(), b"binary bytes")
        with self.assertRaises(ValueError):
            RELEASE["package"]("1.2.3", "x86_64-unknown-linux-gnu", binary, self.path, root)

    def test_archives_take_the_source_date_and_never_predate_zip_time(self):
        root = self.project("1.2.3", "1.2.3")
        binary = self.path / "botwork"
        binary.write_bytes(b"binary")
        for epoch, expected in (("1700000000", 1700000000), ("0", RELEASE["EARLIEST"])):
            with self.subTest(epoch=epoch), mock.patch.dict(os.environ, {"SOURCE_DATE_EPOCH": epoch}):
                path = RELEASE["package"]("1.2.3", "aarch64-apple-darwin", binary, self.path / epoch, root)
                with tarfile.open(path) as archive:
                    self.assertEqual({member.mtime for member in archive.getmembers()}, {expected})

    def test_checksums_match_sha256sum_and_skip_their_own_file(self):
        assets = self.path / "sums"
        assets.mkdir()
        (assets / "b.zip").write_bytes(b"b")
        (assets / "a.tar.gz").write_bytes(b"a")
        (assets / "SHA256SUMS").write_text("stale")
        RELEASE["checksums"](assets)
        expected = "".join(f"{hashlib.sha256(data).hexdigest()}  {name}\n" for name, data in (("a.tar.gz", b"a"), ("b.zip", b"b")))
        self.assertEqual((assets / "SHA256SUMS").read_text(), expected)
        check = subprocess.run(["sha256sum", "--check", "--strict", "SHA256SUMS"], cwd=assets, capture_output=True)
        self.assertEqual(check.returncode, 0, check.stdout)
        empty = self.path / "empty"
        empty.mkdir()
        with self.assertRaises(ValueError):
            RELEASE["checksums"](empty)

    def test_manifests_name_each_archive_and_its_checksum(self):
        assets = self.assets()
        out = self.path / "manifests"
        written = RELEASE["manifests"]("1.2.3", assets, out)
        sums = RELEASE["read_checksums"](assets)
        url = lambda target: f"https://github.com/dhilipsiva/botwork/releases/download/v1.2.3/botwork-v1.2.3-{target}.{RELEASE['TARGETS'][target]}"
        formula = (out / "homebrew/botwork.rb").read_text()
        for target in ("aarch64-apple-darwin", "x86_64-unknown-linux-musl"):
            name = url(target).rsplit("/", 1)[1]
            self.assertIn(f'url "{url(target)}"\n      sha256 "{sums[name]}"', formula)
        self.assertIn('bin.install "botwork"', formula)
        self.assertLess(len(RELEASE["DESCRIPTION"]), 80)
        windows = url("x86_64-pc-windows-msvc")
        digest = sums[windows.rsplit("/", 1)[1]]
        scoop = json.loads((out / "scoop/botwork.json").read_text())
        self.assertEqual(scoop["architecture"]["64bit"], {"url": windows, "hash": digest})
        self.assertEqual(scoop["version"], "1.2.3")
        self.assertEqual(scoop["bin"], "botwork.exe")
        installer = next(path for path in written if path.endswith(".installer.yaml"))
        installer = (out / installer).read_text()
        self.assertIn(f"InstallerUrl: {windows}\n  InstallerSha256: {digest.upper()}\n", installer)
        self.assertEqual(
            sorted(path for path in written if path.startswith("winget/")),
            sorted(f"winget/manifests/d/dhilipsiva/Botwork/1.2.3/dhilipsiva.Botwork{suffix}.yaml" for suffix in ("", ".installer", ".locale.en-US")),
        )
        for path in written:
            if path.startswith("winget/"):
                text = (out / path).read_text()
                self.assertIn("PackageIdentifier: dhilipsiva.Botwork\nPackageVersion: 1.2.3\n", text)
                self.assertTrue(text.endswith("ManifestVersion: 1.9.0\n"), text)

    def test_manifests_refuse_missing_or_altered_archives(self):
        assets = self.assets()
        archive = assets / "botwork-v1.2.3-x86_64-pc-windows-msvc.zip"
        archive.write_bytes(archive.read_bytes() + b"altered")
        with self.assertRaises(ValueError):
            RELEASE["manifests"]("1.2.3", assets, self.path / "altered")
        archive.unlink()
        RELEASE["checksums"](assets)
        with self.assertRaises(ValueError):
            RELEASE["manifests"]("1.2.3", assets, self.path / "missing")


class Workflow(unittest.TestCase):
    def test_every_action_is_pinned_to_a_commit(self):
        for workflow in sorted((ROOT / ".github/workflows").glob("*.yml")):
            for line in workflow.read_text().splitlines():
                if "uses:" in line and "./" not in line:
                    with self.subTest(workflow=workflow.name, line=line.strip()):
                        self.assertRegex(line, r"uses: [\w.-]+/[\w./-]+@[0-9a-f]{40} # v[\d.]+$")

    def test_the_workflow_builds_every_packaged_target(self):
        built = set(re.findall(r"^ +- target: (\S+)$", WORKFLOW, re.M))
        self.assertEqual(built, set(RELEASE["TARGETS"]))

    def test_publishing_needs_a_tag_and_skips_without_credentials(self):
        self.assertIn("permissions:\n  contents: read\n", WORKFLOW)
        for secret in ("CARGO_REGISTRY_TOKEN", "HOMEBREW_TAP_TOKEN", "SCOOP_BUCKET_TOKEN", "WINGET_TOKEN", "VSCE_PAT", "OVSX_PAT"):
            with self.subTest(secret=secret):
                self.assertIn(f"secrets.{secret}", WORKFLOW)
                self.assertIn(f"{secret} is not set", WORKFLOW)
        # Only the release job may write, and only for a tag.
        jobs = dict(re.findall(r"^  (\w+):\n(.*?)(?=^  \w+:\n|\Z)", WORKFLOW, re.M | re.S))
        self.assertEqual([name for name, body in jobs.items() if "contents: write" in body], ["release"])
        self.assertIn("    if: needs.version.outputs.publish == 'true'\n", jobs["release"])
        self.assertIn("gh release create", jobs["release"])
        # Every other step that publishes runs only for a tag.
        commands = ("cargo +stable publish --locked\n", "git push", "wingetcreate.exe submit", "vsce publish", "ovsx publish")
        steps = [step for name, body in jobs.items() if name != "release" for step in body.split("\n      - name: ")]
        for command in commands:
            publishing = [step for step in steps if command in step]
            with self.subTest(command=command):
                self.assertTrue(publishing)
                for step in publishing:
                    self.assertIn("if: env.PUBLISH == 'true' && ", step)


if __name__ == "__main__":
    unittest.main()
