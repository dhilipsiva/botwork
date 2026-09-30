# Roadmap decisions

These decisions were ratified by the project owner on 2026-09-30. They settle
the questions that [TODO.md](../TODO.md) left to the owner, and they revise
three preregistered gates. Later the same day, the owner revised D4 and added
D19 after the first campaign checked against D4 failed; D4 keeps its original
text as history. The roadmap requires a written rationale for any
revision of a frozen task, budget, or score ("Freeze tasks, budgets, and
scoring before measuring; any subsequent revision needs a written rationale
and renewed assessment"). This page is that record.

Each TODO item that a decision governs cites it by number and links here,
such as `D4`.

## Summary

| # | Decision |
| --- | --- |
| D1 | Two independent AI review sessions are the two scored reviewers |
| D2 | A study kit for the human usability sessions, plus an AI-simulated pilot that does not count |
| D3 | The reference benchmark host is the development workstation |
| D4 | Budgets are the accepted baseline's p95 time, peak heap, and binary size plus 25% (revised) |
| D5 | 1.0 supports Linux x86_64, macOS arm64, and Windows x86_64 |
| D6 | 1.0 ships WASM, Python, and JavaScript adapters |
| D7 | JavaScript runs in an out-of-process Node worker |
| D8 | WASM runs on Wasmtime with the component model |
| D9 | Python is an optional feature that links the system libpython |
| D10 | The package registry is deferred past 1.0 |
| D11 | 1.0 integrates WebDriver, Playwright, and Appium |
| D12 | Workers and processes have full parity on macOS and Windows |
| D13 | The branch-coverage gate is dropped; the line-coverage gate stays |
| D14 | Pre-release fuzzing is 1 CPU-hour per target |
| D15 | The language, diagnostic codes, reports, CLI, and Rust API are stable at 1.0 |
| D16 | The minimum supported Rust version is the latest stable release |
| D17 | Binaries ship through GitHub Releases, crates.io, Homebrew, winget, and Scoop |
| D18 | The VS Code extension ships through the Marketplace and Open VSX |
| D19 | Distributed binaries use fat LTO, one codegen unit, and packed relocations |

## Assessment

### D1: Reviewers

The two scored reviews that the quality target requires are two independent AI
review sessions. Each session:

- starts fresh, with no access to authoring transcripts or earlier reviews;
- scores from the identified release and its evidence bundle alone, using the
  [assessment protocol](quality-assessment.md);
- submits its scores before any comparison;
- records its model and version.

**Rationale.** No human reviewers are available on the roadmap's schedule.
**Limitation.** Reviewers from one model family can share blind spots. The
release evidence report states this beside the scores.

### D2: Usability sessions

The human sessions stay required: at least ten participants, as the usability
items in milestone 9 specify. The owner recruits them and runs the sessions.
The implementation provides:

- a study kit: starter files, prompts, automated success checks, a timing and
  scoring sheet, a consent note, and a results template;
- an AI-simulated pilot of tasks U1–U6, attempted from the published
  documentation only.

The pilot is formative. It finds confusing documentation and syntax before the
sessions, and it never counts toward the thresholds.

## Performance

### D3: Reference host

The reference benchmark environment is the development workstation that
recorded the [baseline campaign](performance-baseline-evidence.json): WSL2 on an AMD
Ryzen 9 9950X3D, with eight visible CPUs. Budgets and regression comparisons
apply only to campaigns on this host, using the same protocol.

### D4: Budgets and baseline

*Revised on 2026-09-30; the original decision follows as history.*

The campaign in [baseline evidence](performance-baseline-evidence.json),
captured on 2026-09-30 at revision `628b782` with measurement protocol 2 and
the `dist` build (D19), is the accepted baseline. Each workload's budget is its
p95 workload time × 1.25 and its maximum peak heap × 1.25:

| Workload | Baseline p95 | Time budget | Baseline peak heap | Heap budget |
| --- | ---: | ---: | ---: | ---: |
| CLI startup | 3.164 ms | 3.95 ms | 158 KiB | 198 KiB |
| Parse 10,000 statements | 32.060 ms | 40.08 ms | 14,508 KiB | 18,135 KiB |
| 100,000 custom calls | 344.909 ms | 431.14 ms | 162 KiB | 203 KiB |
| 1,000,000 loop iterations | 2,133.860 ms | 2,667.33 ms | 156 KiB | 195 KiB |
| Sixteen 256 KiB source loads | 8.476 ms | 10.59 ms | 933 KiB | 1,167 KiB |
| 100 waiting runs | 21.103 ms | 26.38 ms | 4,274 KiB | 5,343 KiB |

The CLI binary's budget is its 10,525,736-byte size × 1.25: 13,157,170 bytes.

Times are rounded to 0.01 ms, and memory and size are rounded up. The budget
check computes the same values from the recorded campaign. A later campaign on
the reference host fails the gate when any workload's p95 time or peak heap,
or the binary's size, exceeds its budget. It also fails when any of them
regresses more than 10% against the accepted baseline without an explanation
(milestone 6). Peak RSS is recorded but not budgeted.

Peak heap is the most memory a run's allocations hold at once. Unlike peak
RSS, it leaves out the binary's pages, so budgeting it separately from the
binary's size tracks growth in each on its own.

**Rationale.** The budgets guard against regressions without making release
depend on optimization work.

**Why D4 was revised.** The first campaign checked against the original
budgets, at revision `628b782`, failed every workload. Peak RSS was over budget
for all six, 27.4% to 104.1% above the first baseline. CLI startup's p95 time
was over budget too, 115.8% above the baseline, and four other workloads' p95
times regressed 13.8% to 22.2%. The 51 commits since the
first baseline added HTTP, process, operating-system, and data statements,
reports, checking, formatting, and the language server. They grew the
`release` binary from 4,134,064 to 14,645,512 bytes. Every process pays for the
binary's code and relocated data, so peak RSS rose by 3.5 to 5.8 MiB even for
the loop, whose live data did not change. A budget on peak RSS was a budget on
binary size, and the original budgets could not be met without removing
features. The owner chose to:

- re-baseline at the current revision with the optimized build (D19), with
  budgets at 1.25 times the new baseline;
- budget peak heap and binary size in place of peak RSS.

[Measurement protocol 2](performance.md) implements both.

**Original decision.** The campaign in [performance evidence](performance-evidence.json),
captured on 2026-09-28 at revision `2aed64d` with protocol 1 and the `release`
build, was the accepted baseline. Each workload's budget was its p95 workload
time × 1.25 and its maximum peak resident memory × 1.25:

| Workload | Baseline p95 | Time budget | Baseline peak RSS | Memory budget |
| --- | ---: | ---: | ---: | ---: |
| CLI startup | 1.620 ms | 2.03 ms | 4,880 KiB | 6,100 KiB |
| Parse 10,000 statements | 30.832 ms | 38.54 ms | 12,944 KiB | 16,180 KiB |
| 100,000 custom calls | 305.379 ms | 381.72 ms | 5,136 KiB | 6,420 KiB |
| 1,000,000 loop iterations | 1,905.920 ms | 2,382.40 ms | 5,132 KiB | 6,415 KiB |
| Sixteen 256 KiB source loads | 7.374 ms | 9.22 ms | 6,048 KiB | 7,560 KiB |
| 100 waiting runs | 18.703 ms | 23.38 ms | 7,892 KiB | 9,865 KiB |

### D19: Distribution build

*Added on 2026-09-30, with the revision of D4.*

Distributed binaries and benchmark campaigns use the `dist` Cargo profile: the
`release` profile with fat link-time optimization and one codegen unit. On GNU
Linux x86_64, `.cargo/config.toml` also links with packed relative relocations
(`-z pack-relative-relocs`). `release` stays the quick default, so
`cargo test --release` does not link every test binary with LTO.

**Rationale.** At revision `628b782` the `release` CLI was 14,645,512 bytes,
with 48,483 relocations (1,163,592 bytes) that the loader applies at every
start. Most of its growth since the first baseline was code and relocated data
that every process loads, whatever it runs. The `dist` build of the same source
is 10,525,736 bytes, 28% smaller, and its relocation tables take 22,752 bytes.
In campaigns with the same workloads and timing boundaries, the `dist` one at a
higher load, it cut median CLI startup from 3.102 to 2.749 ms and median parse
time from 31.473 to 26.823 ms, and every other workload's median also fell, by
2% to 7%.

Packed relocations need GNU ld 2.38 or later (or lld 15), and GNU ld adds a
dependency on glibc 2.36's `GLIBC_ABI_DT_RELR` symbol version. The release
workflow ships static musl binaries for Linux (D17), which the setting does not
affect, so only GNU builds made from a checkout need that glibc.

## Platforms and extensions

### D5: Platforms

1.0 supports Linux x86_64 (GNU and static musl), macOS arm64, and Windows
x86_64. Each needs a CI matrix entry and must pass the conformance corpus
(milestone 10).

### D6: Adapters

1.0 ships three adapters: WASM via WASI, Python via PyO3, and JavaScript. All
three share one adapter conformance suite.

### D7: JavaScript hosting

JavaScript extensions run in an out-of-process Node worker that speaks the
existing [worker protocol](worker-protocol.md). This replaces the README's
Neon proposal. Neon would make Botwork a native module inside Node, which
conflicts with the standalone CLI.

The worker gives extensions npm packages, including Playwright (D11), process
isolation, and the existing cancellation. Node is needed only when a script
uses a JavaScript extension.

### D8: WASM runtime

WASM modules run on Wasmtime with the component model. A WIT interface
declares statements, WASI Preview 2 grants capabilities, and fuel and epoch
limits bound execution. The adapter is in-process and sandboxed, and it keeps
the single binary.

### D9: Python packaging

Python support is an optional `python` Cargo feature. It links the system
libpython 3.10 or later through PyO3's stable ABI (`abi3`). The default binary
stays a single file, and Python users build or download the python-enabled
variant. Python extensions are trusted and run in-process, unlike capability-restricted
WASM modules.

### D10: Packages and registry

1.0 ships the pieces that make packages reproducible:

- a package manifest;
- a lockfile with content hashes;
- git and URL sources with integrity pinning;
- an offline cache.

The trusted and verified registry moves past 1.0. Its verification
guarantees, publisher identity, and revocation design come before any
registry is published.

### D11: Browser and mobile integrations

1.0 integrates three tools:

- **WebDriver/Selenium:** a native Rust client for the W3C WebDriver protocol.
  CI verifies it with headless Chrome and chromedriver.
- **Playwright:** the official Playwright library through the Node host (D7).
- **Appium:** the WebDriver client with Appium capabilities. CI verifies it on
  an Android emulator.

### D12: Platform parity

Isolated workers, process-tree ownership, and the process statements offer the
same ownership guarantees on macOS and Windows as on Linux. Each platform uses
its native mechanisms: Job Objects on Windows, and process groups with kqueue
process tracking on macOS.

**Risk.** macOS has no equivalent of the PID namespace behind
`with_pid_namespace`. If a guarantee proves unattainable there, the work stops
and reports the gap for a new decision. It does not ship a weaker mode under
the same name.

## Release gates

### D13: Coverage

Item 303's 90% branch-coverage requirement is dropped, and the 95%
line-coverage gate on correctness code remains.

**Rationale.** Stable Rust cannot measure branch coverage, and a pinned
nightly toolchain for coverage was declined.

### D14: Fuzzing

Before release, each fuzz target runs for at least 1 CPU-hour instead of 24,
with seeds and minimized failures archived as before. The CI smoke budget is
unchanged.

**Rationale.** A shorter pre-release campaign was chosen over dedicated
long-running compute. The release evidence report states the budget.

## Compatibility and distribution

### D15: Stable contracts

At 1.0, these contracts become stable. Breaking any of them needs a major
version and migration notes:

- the language and the built-in statements' signatures;
- diagnostic codes: new codes may be added, and retired codes are never
  reused;
- the JSON report schema, which is versioned and changes only by adding;
- the CLI flags and exit statuses;
- the Rust embedding API of the `botwork` crate, under SemVer.

The Rust API gets a review before 1.0 that hides implementation details from
the public surface.

### D16: Minimum supported Rust version

Botwork builds with the latest stable Rust release. CI tests the newest
stable, and no older toolchain is promised.

### D17: Binary distribution

Botwork ships through:

- **GitHub Releases:** binaries for every supported platform (static musl on
  Linux), with SHA-256 checksums and build provenance, built by a
  tag-triggered workflow;
- **crates.io:** the `botwork` crate, already published at 0.2.0;
- **Homebrew:** a tap formula;
- **winget and Scoop:** manifests that point at the GitHub Release binaries.

### D18: VS Code extension

The VS Code extension is published to the VS Code Marketplace and to Open VSX
by the release workflow.

## Owner prerequisites

The release workflow skips each publishing job whose credentials are missing.
Work can proceed before these exist.

| Channel | Needed |
| --- | --- |
| crates.io | A `CARGO_REGISTRY_TOKEN` repository secret |
| Homebrew | A tap repository and a `HOMEBREW_TAP_TOKEN` secret |
| winget | A `WINGET_TOKEN` secret for winget-pkgs pull requests |
| Scoop | A bucket repository |
| VS Code Marketplace | A publisher ID, which replaces the placeholder in `editors/vscode/package.json`, and a `VSCE_PAT` secret |
| Open VSX | A namespace and an `OVSX_PAT` secret |
| Usability study | At least ten participants (D2) |
