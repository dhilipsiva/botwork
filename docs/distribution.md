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
cargo install --path . --locked
```

`--locked` builds with the dependency versions in `Cargo.lock`, the ones the
tests ran with. [Getting started](getting-started.md) continues from here.

### A static Linux binary

A musl build has no runtime dependencies, not even the C library, so it can be
copied to any x86_64 Linux machine:

```sh
sudo apt-get install musl-tools
rustup target add x86_64-unknown-linux-musl
CC_x86_64_unknown_linux_musl=musl-gcc \
  cargo build --release --locked --target x86_64-unknown-linux-musl
```

The binary is `target/x86_64-unknown-linux-musl/release/botwork`. CI tests the
GNU and musl builds, each in debug and release.

## Published versions

The `botwork` crate on crates.io is version 0.2.0, an early prototype published
in April 2023. It predates this documentation and most of the language. The
checkout still reports 0.2.0 too, until the next release. Install from source
for now.

The [roadmap decisions](decisions.md#d17-binary-distribution) plan these
release channels:

- GitHub Releases, with SHA-256 checksums and build provenance;
- crates.io;
- a Homebrew tap;
- winget and Scoop.

A tag-triggered release workflow will publish to all of them. None of them is
available yet.

## What the binary needs

| Feature | Needs at run time |
| --- | --- |
| Running scripts and suites, reports, `--check`, `--format`, `--lsp` | Nothing beyond the binary |
| Process statements and isolated workers | Linux; see [worker platforms](worker-platforms.md) |
| HTTP statements | Network access to the servers a script calls |
| Planned Python and JavaScript adapters | libpython or Node; see [extending Botwork](extending.md#language-adapters) |

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
