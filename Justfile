# Botwork's local checks, the same commands .github/workflows/ci.yml runs on Linux. Run them in
# the dev shell, `nix develop --command just ci`, or with any stable Rust that has rustfmt and
# Clippy, plus Python 3 and Node. docs/testing.md explains what each one covers.

set shell := ["bash", "-euo", "pipefail", "-c"]

export RUST_BACKTRACE := "1"

# List the recipes.
default:
    @just --list

# CI's checks before the tests: formatting, Clippy, helper tests, the WASM guest, a full build.
check:
    cargo fmt --all -- --check
    cargo clippy --locked --all-targets -- -D warnings
    python3 tests/coverage_tools.py
    python3 tests/fuzz_tools.py
    python3 tests/mutation_tools.py
    python3 tests/release_tools.py
    python3 tests/api_tools.py
    python3 tests/wasm_tools.py
    python3 scripts/wasm_guest.py check
    cargo build --locked --all-targets
    python3 tests/performance_tools.py

# The unit, integration, and documentation tests and the doctests.
test:
    cargo test --locked --no-fail-fast

# Everything CI's Linux test job runs: check, the benchmark workloads' correctness, and test.
ci: check
    python3 scripts/performance.py --smoke --profile debug --target x86_64-unknown-linux-gnu
    cargo test --locked --no-fail-fast

# The library-unit and full-suite line coverage baselines, with cargo-llvm-cov 0.9.1.
coverage *args:
    python3 scripts/coverage.py {{ args }}

# The core mutation campaign: `just mutants targeted` or `just mutants generated`.
mutants kind="targeted":
    python3 scripts/mutation_core.py {{ kind }}

# The fixed libFuzzer smoke campaign, with the nightly and cargo-fuzz of `nix develop .#nightly`.
fuzz-smoke *args:
    nix develop .#nightly --command python3 scripts/fuzz_smoke.py --toolchain '' {{ args }}

# CI's public API check: no signature names an internal item; docs/public-api.txt is exact.
public-api:
    nix develop .#nightly --command cargo rustdoc --lib --locked -- -Z unstable-options --output-format json
    python3 scripts/public_api.py --leaks target/doc/botwork.json
    python3 scripts/public_api.py target/doc/botwork.json | diff --unified docs/public-api.txt -

# Package the crate and build it from the package, without publishing.
release-dry-run:
    cargo publish --locked --dry-run
