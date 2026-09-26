# Reproducing Regressions

Keep each confirmed defect's smallest failing source and its intended result in a named test before changing the interpreter. Preserve that test after the fix. Inline sources in `tests/regressions.rs` cover the original 19 reviewed defects; process-level failures use files under `tests/fixtures/`. Later contract/unit suites retain additional minimal inputs beside their assertions. The development log records the initial failure and validation for each change.

## Replay a Failure

Run one named case in both profiles, using the checked-in dependency lock:

```sh
cargo test --locked --test regressions minimum_signed_integer_literal_is_representable -- --exact
cargo test --locked --release --test regressions minimum_signed_integer_literal_is_representable -- --exact
```

For a CLI fixture, run `cargo run --locked -- --file tests/fixtures/runtime-error.botwork`; it intentionally exits with status `1`. Record stdout, stderr, and status separately. Add `--offline` when dependencies are already cached. Do not regenerate expected output until the intended behavior change has been reviewed.

## Failure Record

Include these fields with a new reproduction or campaign artifact:

| Field | Required evidence |
| --- | --- |
| Revision | Commit ID, relevant uncommitted patch, and lockfile revision |
| Build | `rustc -Vv`, `cargo -V`, target/OS/architecture, debug or release |
| Replay | Exact command, minimal source/fixture, expected result, actual status/stdout/stderr |
| Randomization | Seed, generator/harness version, original input, minimized input; `none` for deterministic cases |
| Adapters | Names, versions, configuration, and controlled service fixtures; `none` for interpreter-only cases |
| Resources | Configured timeout, source/nesting/step/collection limits, worker count, and measurement environment |

Current core cases are deterministic and use no external adapters. The interpreter has no configurable resource limits yet; record that explicitly. The documentation CLI harness imposes a five-second process timeout and reports toolchain, platform, build profile, document location, and source on failure. Randomized campaigns, adapter matrices, and runtime limits retain their own roadmap tasks; this procedure does not claim those measurements already exist.

## Documentation Regressions

Run `cargo test --locked --test documentation` to extract and execute registered Botwork fences from README and every Markdown file under `docs/`. Checked stdout lives in `tests/doc-examples/`; each snippet must be a self-contained successful script. The harness asserts exact stdout, empty stderr, and status `0`, and rejects missing, duplicate, stale, or unregistered example IDs.

Place an HTML comment of the form `<!-- botwork-test: example-id -->` immediately before each Botwork fence, using a unique lowercase/digit/hyphen ID. Register its document and stdout file in `tests/documentation.rs`. The reader supports backtick/tilde fences, up to three leading spaces, and CRLF; it preserves code bytes after fence indentation. Use standard unindented fences in project docs. Inline fragments remain explanatory and are covered by language contract tests where relevant.

Rust API examples are included into crate documentation and run by `cargo test --doc`. New Rust examples must be added to rustdoc's inputs and the documentation inventory. Rust fence options that skip execution are rejected. Shell fences describe contributor commands and are not executed by this harness. CI's debug/release jobs run the complete suite, including documentation checks and doctests.
