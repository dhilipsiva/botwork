# Installing and distributing Botwork

Botwork is one executable, `botwork`. This page covers how to build and install
it, what it needs at run time, and how Botwork code is shared.

## Install from source

This is the supported way to install the current version. Botwork builds with
the latest stable Rust ([D16](decisions.md#d16-minimum-supported-rust-version));
install it from [rustup.rs](https://rustup.rs).

```sh
git clone https://github.com/dhilipsiva/botwork.git
cd botwork
cargo install --path . --locked --profile dist
```

`--locked` builds with the dependency versions in `Cargo.lock`, the ones the
tests ran with. `--profile dist` builds the optimized binary that releases ship
([D19](decisions.md#d19-distribution-build)): fat link-time optimization and one
codegen unit make it smaller and faster to start, and the build takes a few
minutes longer. Leave it out for a quicker `release` build.
[Getting started](getting-started.md) continues from here.

Builds and installs from the checkout read its `.cargo/config.toml`, which
packs relative relocations in GNU Linux builds. Those binaries need glibc 2.36
or later; on an older system, build the static binary below.

### A static Linux binary

A musl build has no runtime dependencies, not even the C library, so it can be
copied to any x86_64 Linux machine:

```sh
sudo apt-get install musl-tools
rustup target add x86_64-unknown-linux-musl
CC_x86_64_unknown_linux_musl=musl-gcc \
  cargo build --profile dist --locked --target x86_64-unknown-linux-musl
```

The binary is `target/x86_64-unknown-linux-musl/dist/botwork`. CI tests the
GNU and musl builds, each in debug and release.

## Published versions

The `botwork` crate on crates.io is version 0.2.0, an early prototype published
in April 2023. It predates this documentation and most of the language. The
checkout still reports 0.2.0 too, until the next release. Install from source
until the release workflow below publishes one.

## Releases

A tag `vX.Y.Z` releases version X.Y.Z through the channels the
[roadmap decisions](decisions.md#d17-binary-distribution) chose. The tag must
name the version that both `Cargo.toml` and `editors/vscode/package.json`
carry. `.github/workflows/release.yml` then:

1. runs every CI check;
2. builds the `dist` binary for Linux x86_64 (static musl), macOS arm64, and
   Windows x86_64, checks that it reports the version, and archives it with
   `LICENSE` and `README.md` as `botwork-vX.Y.Z-<target>.tar.gz`, or `.zip` on
   Windows;
3. packages the VS Code extension as `botwork-vX.Y.Z.vsix`;
4. writes `SHA256SUMS` and the Homebrew, Scoop, and winget manifests, and runs
   the packaged Linux binary;
5. creates the GitHub release with those files and attests their build
   provenance;
6. publishes to every other channel whose credentials are configured.

Archives are reproducible: their entries have a fixed order, owner, and mode,
and the tagged commit's timestamp. `scripts/release.py` makes them and the
manifests; `tests/release_tools.py` checks it and the workflow.

To check a download against the release:

```sh
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify botwork-vX.Y.Z-x86_64-unknown-linux-musl.tar.gz --repo dhilipsiva/botwork
```

Run the workflow by hand on a branch for a dry run, such as
`gh workflow run release.yml --ref main`. It does everything except publish,
including `cargo publish --dry-run`.

| Channel | Publishes | Needs |
| --- | --- | --- |
| GitHub Releases | The archives, the extension, `SHA256SUMS`, and provenance | Nothing beyond the workflow's own token |
| crates.io | The `botwork` crate, without the evidence records under `docs/` | A `CARGO_REGISTRY_TOKEN` secret |
| Homebrew | `Formula/botwork.rb` in the tap | A `HOMEBREW_TAP` repository variable naming the tap, and a `HOMEBREW_TAP_TOKEN` secret that can push to it |
| Scoop | `bucket/botwork.json` in the bucket | A `SCOOP_BUCKET` repository variable naming the bucket, and a `SCOOP_BUCKET_TOKEN` secret that can push to it |
| winget | A pull request to `microsoft/winget-pkgs` for `dhilipsiva.Botwork`, made with `wingetcreate` | A `WINGET_TOKEN` secret, a classic token with the `public_repo` scope |
| VS Code Marketplace | The extension | A `VSCE_PAT` secret, and the publisher's ID in `editors/vscode/package.json` |
| Open VSX | The extension | An `OVSX_PAT` secret, and a namespace matching the publisher |

A channel without its credentials is skipped with a notice in the run's
summary, and the others still publish.

## What the binary needs

| Feature | Needs at run time |
| --- | --- |
| Running scripts and suites, reports, `--check`, `--format`, `--lsp` | Nothing beyond the binary |
| Process statements and isolated workers | Linux, macOS, or Windows; see [worker platforms](worker-platforms.md) |
| HTTP statements | Network access to the servers a script calls |
| [WebAssembly statements](wasm.md) | Nothing: Wasmtime is part of the binary, through the default `wasm` feature, and makes up about 16 MB of it |
| [Python statements](python.md) | A build with the `python` feature, and libpython 3.10 or later |
| [JavaScript statements](javascript.md) | Node.js on `PATH` |

The [compatibility guide](compatibility.md) lists the supported platforms and
tool versions.

## Sharing Botwork code

Today, share statements as [modules](extending.md#botwork-modules): `.botwork`
files that scripts import by relative path, kept in the same repository or
added with your usual tools, such as git submodules.

Packages are planned for 1.0 ([D10](decisions.md#d10-packages-and-registry)):

- a package manifest;
- a lockfile with content hashes;
- git and URL sources pinned by version or content;
- an offline cache.

A trusted and verified package registry will follow after 1.0.

## Editor packages

The Helix, Vim, and VS Code packages are in `editors/`; see
[editor support](editors.md).
