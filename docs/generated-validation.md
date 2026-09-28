# Generated Core Validation

The core now has seeded property checks and three libFuzzer targets sharing the same checks in `tests/support/generated.rs`. They supplement the named regression/conformance cases. These initial campaigns do not complete the mutation-testing, import/adapter fuzzing, formatter, or 24 CPU-hour release gates.

## Properties and Scope

| Check / target | Generated input | Oracle and limits | Specification evidence |
| --- | --- | --- | --- |
| `expression` | One to eight signed integer operands, with `+`, `-`, `*`, `%` | Independently calculate multiplicative terms and then sums/differences using `i128`, checking the `i32` range after each operation. Compare flat and explicitly grouped DSL spellings. A failing assignment preserves earlier effects and skips later assignments. | E1/E2, V1/V2 |
| `literals` | All value kinds, finite binary32 bit patterns, Unicode/control/escaped strings, arrays/maps up to three container levels with at most two entries each | Encode valid DSL, execute, and compare kinds, integer values, exact string bytes, float bits (including negative zero/subnormals), array elements, map keys, and last duplicate values. `None` comes from an empty custom call. | V1/V5/V7 |
| `parse` | Raw mutated UTF-8 source with a syntax dictionary and curated source corpus | Bounded parsing/lowering/validation must return a stable AST or diagnostic. Check source ownership, top-level spans, diagnostic coordinates and rendering, and repeatability. Invalid UTF-8 is rejected by the harness because the parser accepts `&str`. Raw fuzz source is never evaluated. | E1, L1, diagnostic locations, R2/R4 |

The expression oracle does not use the implementation's arithmetic helpers or Pratt parser. It covers the integer subset listed above; powers, division, floating arithmetic, comparisons, short-circuit effects, arbitrary control flow, and imports retain their existing named tests and need further generated coverage. The literal encoder is a test generator, not the future public formatter. AST construction here means parser lowering and validation, not arbitrary host-assembled trees.

Evaluation uses a fresh `Engine` per generated program, with environment inheritance disabled, 4,096 source bytes, 256 execution steps, eight call frames, and 64 evaluation frames. Other run limits keep their source-versioned defaults. Parser checks use 2,048 AST nodes, 96 AST levels, 4,096 aggregate AST source bytes, and the existing syntax ceilings. These are test budgets, not a change to public defaults.

## Deterministic Tests and Replay

`tests/generated_properties.rs` uses generator version 1: SplitMix64 with explicitly wrapping arithmetic, little-endian output, and a separate per-case seed stride. Eight fixed seeds (`0`, `1`, `42`, `65535`, `1311768467463790320`, `9223372036854775808`, `3735928559`, `18446744073709551615`) each generate 128 cases in every family. There are **3,072 generated inputs per profile**, plus five curated parser seeds and the minimum-integer remainder regression. Expressions run both flat and grouped forms. Parser mutations perform up to eight byte insertions, deletions, substitutions, or truncations of the curated sources.

The four Rust tests run in the ordinary GNU/musl debug/release CI jobs:

```sh
cargo test --locked --test generated_properties
cargo test --locked --release --test generated_properties
```

On a failed property, the panic includes its seed, case index, raw bytes, and the underlying assertion/source. Replay one case without changing the normal campaign:

```sh
BOTWORK_PROPERTY_SEED=42 BOTWORK_PROPERTY_CASE=7 cargo test --locked --test generated_properties integer_expressions_match_independent_wide_arithmetic -- --exact --nocapture
```

The same flags work for the other two generated families. Seed and case values are decimal `u64`s. They select a single generated input; the fixed seed corpus checks still run when their separate test is selected. Preserve a failing input and its decoded source before changing the generator, then add a focused regression under the reproduction procedure below.

## Coverage-Guided Fuzzing

The separate `fuzz/` package pins `libfuzzer-sys` 0.4.13 and tracks its own lockfile. [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html) supplies coverage-guided mutation and AddressSanitizer instrumentation. The initial tool versions are cargo-fuzz 0.13.2 and nightly-2026-08-14 (`rustc 1.99.0-nightly`, commit `ba28ff76f`). A native C++ compiler and Python 3.11+ are also required by the build/campaign tools.

```sh
rustup toolchain install nightly-2026-08-14 --profile minimal
cargo install cargo-fuzz --version 0.13.2 --locked
python3 scripts/fuzz_smoke.py
```

An existing `nightly` alias with exactly the recorded compiler may be selected with `--toolchain nightly`. The runner checks versions, fetches locked dependencies, builds offline, and checks that recorded source/lockfile inputs stayed unchanged. Compiler aliases with different versions are rejected rather than silently changing the campaign. Tool upgrades require a new recorded baseline.

The default smoke budget is frozen at **10,000 executions per target**, seed **314159**, maximum input **8,192 bytes**, a **two-second per-input timeout**, **512 MiB RSS**, and a **60-second fuzz-loop allowance**. The wrapper caps each build/run process group at **300 seconds**. A time-limited exit with fewer than the requested executions fails verification, even if libFuzzer exits with status zero. The default optimized fuzz build retains debug assertions and overflow checks.

Each target starts with an empty writable corpus and the checked-in `fuzz/seeds/<target>` inputs as an additional read corpus. `parse` also loads `fuzz/botwork.dict`. The runner creates a unique directory under ignored `target/fuzz-smoke/` containing commands, input hashes, toolchain/platform, seed/budgets, statistics, raw logs, discovered corpus files, and any failure artifacts. The summary is written on success or failure. CI runs this campaign separately from ordinary tests and uploads the directory with 30-day artifact retention. Hosted execution still needs observation after publication.

For another recorded smoke run, use `--seed N`, `--runs N`, or `--target parse|expression|literals`; omitted target means all three. These overrides are recorded and do not replace the fixed CI budget. Do not treat this wrapper's minute-scale smoke runs as the 24 CPU-hour release campaigns.

Replay and minimize a saved failure using the [cargo-fuzz workflow](https://rust-fuzz.github.io/book/cargo-fuzz/tutorial.html):

```sh
cargo +nightly-2026-08-14 fuzz run parse target/fuzz-smoke/CAMPAIGN/parse/artifacts/CRASH_FILE -- -runs=1
cargo +nightly-2026-08-14 fuzz tmin parse target/fuzz-smoke/CAMPAIGN/parse/artifacts/CRASH_FILE
```

Replace the placeholders with the printed artifact path and the target that failed. Archive the original and minimized bytes, decoded generated source where applicable, source/lockfile hashes, exact replay command, tool versions, and actual versus expected outcome. Fix production defects and retain a focused regression before accepting the campaign; distinguish harness defects from language defects.

## Initial Evidence

The [recorded campaign](generated-validation-evidence.json) completed 10,000 executions for each of the three targets with AddressSanitizer and no production crash, timeout, or semantic mismatch. GNU and musl passed all four deterministic tests in both profiles. These results establish a reproducible smoke baseline, not exhaustive correctness or the release duration threshold.

During oracle calibration, a generated `-2147483648 % -1` expression exposed an incorrect reference assumption. The existing language contract specifies integer zero, while Rust's narrow `i32` remainder traps. The independent wide-integer oracle was corrected to follow the DSL specification; `fuzz/seeds/expression/minimum-remainder` now pins that check. The evidence records the original expression and minimized case. No production code was changed for this calibration.
