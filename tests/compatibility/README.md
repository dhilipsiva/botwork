# Compatibility fixtures

Samples of every versioned contract, at each version Botwork reads or writes,
and at newer versions it must refuse. `tests/compatibility.rs` checks them,
and [compatibility](../../docs/compatibility.md#versioned-contracts) lists
them. Keep a fixture when its version stops being current: it is how a
supported upgrade stays tested.

| Fixture | How it was made |
| --- | --- |
| `outputs/compat.suite.botwork` | The suite the output fixtures come from |
| `outputs/report-v1.json` | `botwork --suite compat.suite.botwork --report-json report-v1.json`, pretty-printed |
| `outputs/events-v1.jsonl` | The same run's event stream, through `--listener sh -c 'cat > events-v1.jsonl'` |
| `failed-cases/v2.json` | The same run's `--failures` record |
| `failed-cases/v1.json` | Version 1, as earlier Botwork wrote it: ordinary case IDs only |
| `failed-cases/v3.json` | By hand: a later version, with reshaped `failed` entries and a new field |
| `run-records/v1.json` | The failing case's record from `outputs/report-v1.json`, without the report's `number` |
| `run-records/v2.json` | By hand: version 2, with `statements` reshaped |
| `journals/interrupted.suite.botwork` | The suite the journal fixtures come from |
| `journals/v1` | `botwork --suite interrupted.suite.botwork --report-json report.json`, killed while its second case slept. The header's absolute `json` path is replaced by `report.json`, so the copy a test reconciles is the one beside it |
| `journals/v2` | `journals/v1` with a version 2 header, its report paths moved under `reports` |
| `packages/locked` | A project with one path package, locked by `botwork --fetch` |
| `packages/newer.lock` | By hand: a version 2 lockfile, with a key version 1 lacks |
| `packages/project-needs-newer` | A project whose own `botwork` requirement excludes every release so far |
| `packages/newer-keys` | A manifest with a key Botwork does not know, needing a later Botwork |
| `packages/dependency-needs-newer` | `packages/locked`, whose package then raised its `botwork` requirement |
| `worker/response-v1.bin` | A worker protocol response carrying the String `hello` |
| `worker/response-v2.bin` | The same frame, marked version 2 |
| `wasm/statements-0.2.0.wasm` | By hand: a component exporting an empty `botwork:statements/statements@0.2.0` instance |
