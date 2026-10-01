"""Packaging for the tag-triggered release workflow (decisions D17 and D18).

Usage: release.py COMMAND ...
  version REF                   print `version=` and `publish=` lines for
                                $GITHUB_OUTPUT; only a tag publishes, and it
                                must name the version that Cargo.toml and the
                                VS Code extension carry
  package VERSION TARGET BINARY OUT
                                archive the binary with LICENSE and README.md
  checksums DIRECTORY           write DIRECTORY/SHA256SUMS for its assets
  manifests VERSION DIRECTORY OUT
                                write the Homebrew formula, the Scoop manifest,
                                and the winget manifests for DIRECTORY's assets

Archives are reproducible: entries have fixed order, owners, modes, and
timestamps (SOURCE_DATE_EPOCH, or 1980-01-01, the earliest a zip can store).
"""
import gzip
import hashlib
import io
import json
import os
import sys
import tarfile
import time
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REPOSITORY = "dhilipsiva/botwork"
HOMEPAGE = f"https://github.com/{REPOSITORY}"
DESCRIPTION = "Plain-text automation framework for acceptance testing and RPA"
WINGET_IDENTIFIER = "dhilipsiva.Botwork"
WINGET_MANIFEST = "1.9.0"
# Each published binary and the archive format its platform expects.
TARGETS = {
    "x86_64-unknown-linux-musl": "tar.gz",
    "aarch64-apple-darwin": "tar.gz",
    "x86_64-pc-windows-msvc": "zip",
}
EARLIEST = 315532800  # 1980-01-01T00:00:00Z


def versions(root=ROOT):
    """The versions Cargo.toml and the VS Code extension carry."""
    cargo = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    extension = json.loads((root / "editors/vscode/package.json").read_text())["version"]
    return cargo, extension


def version(ref, root=ROOT):
    """The release version and whether `ref` publishes it."""
    cargo, extension = versions(root)
    if extension != cargo:
        raise ValueError(f"the VS Code extension is {extension}, but Cargo.toml is {cargo}")
    if not ref.startswith("refs/tags/"):
        return {"version": cargo, "publish": "false"}
    tag = ref.removeprefix("refs/tags/")
    if tag != f"v{cargo}":
        raise ValueError(f"tag {tag} does not name version {cargo}; tag v{cargo} instead")
    return {"version": cargo, "publish": "true"}


def archive_name(release, target):
    return f"botwork-v{release}-{target}.{TARGETS[target]}"


def timestamp():
    return max(int(os.environ.get("SOURCE_DATE_EPOCH", EARLIEST)), EARLIEST)


def package(release, target, binary, out, root=ROOT):
    """Write the target's archive into `out` and return its path."""
    if target not in TARGETS:
        raise ValueError(f"unknown target {target}")
    executable = "botwork.exe" if TARGETS[target] == "zip" else "botwork"
    entries = [
        (executable, Path(binary).read_bytes(), 0o755),
        ("LICENSE", (root / "LICENSE").read_bytes(), 0o644),
        ("README.md", (root / "README.md").read_bytes(), 0o644),
    ]
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    path = out / archive_name(release, target)
    moment = timestamp()
    if TARGETS[target] == "zip":
        with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
            for name, data, mode in entries:
                info = zipfile.ZipInfo(name, date_time=zip_time(moment))
                # Unix permissions, whichever system builds the archive.
                info.create_system = 3
                info.external_attr = (0o100000 | mode) << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(info, data)
    else:
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for name, data, mode in entries:
                info = tarfile.TarInfo(name)
                info.size, info.mode, info.mtime = len(data), mode, moment
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                archive.addfile(info, io.BytesIO(data))
        with path.open("wb") as file, gzip.GzipFile(filename="", mode="wb", fileobj=file, mtime=moment) as compressed:
            compressed.write(buffer.getvalue())
    return path


def zip_time(moment):
    return time.gmtime(moment)[:6]


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def checksums(directory):
    """Write SHA256SUMS for every other file in `directory`, as sha256sum does."""
    directory = Path(directory)
    assets = sorted(path for path in directory.iterdir() if path.is_file() and path.name != "SHA256SUMS")
    if not assets:
        raise ValueError(f"{directory} holds no release assets")
    lines = [f"{sha256(path)}  {path.name}\n" for path in assets]
    (directory / "SHA256SUMS").write_text("".join(lines))
    return lines


def read_checksums(directory):
    sums = {}
    for line in (Path(directory) / "SHA256SUMS").read_text().splitlines():
        digest, name = line.split("  ", 1)
        if len(digest) != 64 or any(character not in "0123456789abcdef" for character in digest):
            raise ValueError(f"malformed checksum line: {line}")
        sums[name] = digest
    return sums


def download(release, name):
    return f"{HOMEPAGE}/releases/download/v{release}/{name}"


def manifests(release, directory, out):
    """Write the package manager manifests for the assets in `directory`."""
    sums = read_checksums(directory)
    for target in TARGETS:
        name = archive_name(release, target)
        if name not in sums:
            raise ValueError(f"SHA256SUMS lacks {name}")
        if sha256(Path(directory) / name) != sums[name]:
            raise ValueError(f"{name} does not match its checksum")
    asset = lambda target: (download(release, archive_name(release, target)), sums[archive_name(release, target)])
    out = Path(out)
    written = {}

    mac_url, mac_hash = asset("aarch64-apple-darwin")
    linux_url, linux_hash = asset("x86_64-unknown-linux-musl")
    written["homebrew/botwork.rb"] = f'''# Generated by scripts/release.py for Botwork {release}.
class Botwork < Formula
  desc "{DESCRIPTION}"
  homepage "{HOMEPAGE}"
  version "{release}"
  license "MIT"

  on_macos do
    on_arm do
      url "{mac_url}"
      sha256 "{mac_hash}"
    end
  end

  on_linux do
    on_intel do
      url "{linux_url}"
      sha256 "{linux_hash}"
    end
  end

  def install
    bin.install "botwork"
  end

  test do
    assert_match version.to_s, shell_output("#{{bin}}/botwork --version")
  end
end
'''

    windows_url, windows_hash = asset("x86_64-pc-windows-msvc")
    pattern = archive_name("$version", "x86_64-pc-windows-msvc")
    written["scoop/botwork.json"] = json.dumps({
        "version": release,
        "description": DESCRIPTION,
        "homepage": HOMEPAGE,
        "license": "MIT",
        "architecture": {"64bit": {"url": windows_url, "hash": windows_hash}},
        "bin": "botwork.exe",
        "checkver": "github",
        "autoupdate": {
            "architecture": {"64bit": {"url": f"{HOMEPAGE}/releases/download/v$version/{pattern}"}},
            "hash": {"url": "$baseurl/SHA256SUMS"},
        },
    }, indent=4) + "\n"

    publisher, name = WINGET_IDENTIFIER.split(".", 1)
    base = f"winget/manifests/{publisher[0].lower()}/{publisher}/{name}/{release}/{WINGET_IDENTIFIER}"
    header = lambda kind: (f"# yaml-language-server: $schema=https://aka.ms/winget-manifest.{kind}.{WINGET_MANIFEST}.schema.json\n\n"
                           f"PackageIdentifier: {WINGET_IDENTIFIER}\nPackageVersion: {release}\n")
    footer = lambda kind: f"ManifestType: {kind}\nManifestVersion: {WINGET_MANIFEST}\n"
    written[f"{base}.yaml"] = header("version") + "DefaultLocale: en-US\n" + footer("version")
    written[f"{base}.installer.yaml"] = (header("installer") + "InstallerType: zip\n"
        "NestedInstallerType: portable\nNestedInstallerFiles:\n- RelativeFilePath: botwork.exe\n"
        "  PortableCommandAlias: botwork\nInstallers:\n- Architecture: x64\n"
        f"  InstallerUrl: {windows_url}\n  InstallerSha256: {windows_hash.upper()}\n" + footer("installer"))
    written[f"{base}.locale.en-US.yaml"] = (header("defaultLocale")
        + f"PackageLocale: en-US\nPublisher: {publisher}\nPackageName: Botwork\nLicense: MIT\n"
        f"LicenseUrl: {HOMEPAGE}/blob/main/LICENSE\nShortDescription: {DESCRIPTION}\nPackageUrl: {HOMEPAGE}\n"
        + footer("defaultLocale"))

    for relative, text in written.items():
        path = out / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    return sorted(written)


def main(arguments):
    command, *rest = arguments
    if command == "version" and len(rest) == 1:
        for key, value in version(rest[0]).items():
            print(f"{key}={value}")
    elif command == "package" and len(rest) == 4:
        print(package(*rest))
    elif command == "checksums" and len(rest) == 1:
        sys.stdout.writelines(checksums(rest[0]))
    elif command == "manifests" and len(rest) == 3:
        print("\n".join(manifests(*rest)))
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except ValueError as error:
        sys.exit(f"release: {error}")
