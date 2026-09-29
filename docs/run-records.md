# Run records

A run record is the shared result model for one run: its identity, timing,
outcome, top-level statements, logs, error, and artifacts. `core::report` defines
the record and the ordered event stream it is built from. Console, JSON, HTML,
and listener reports build on these types; this page defines their meaning.

## Recording an embedded run

Set `RunOptions::record` to receive `RunResult::record`:

| `RecordOptions` field | Meaning |
| --- | --- |
| `identity` | `RunIdentity` with `id`, `name`, and optional `dataset`/`row`. An empty `id` defaults to the source name, and an empty `name` to the `id` |
| `expectation` | `CaseExpectation` that decides the terminal status (default: success required) |
| `limits` | `RecordLimits` retention bounds (below) |
| `observer` | An optional `EventObserver` that receives each event as it is recorded; see [listeners](listeners.md) |

Recording is off by default. Recording starts before run preparation, so a run
whose limits, source, or environment are rejected still ends with one terminal
record.

## Recording a host-prepared context

Hosts that prepare their own `Context`, as the CLI does, record through
`Recording`:

1. `Recording::start(options)` emits `run_started` when the run begins.
2. `Context::attach_recording(&recording)` captures the context's top-level
   statements and logs. It needs a run environment, as from
   `Context::with_host_environment`; a context without one is rejected with
   BW7002.
3. `Recording::finish(result)` emits the terminal event and returns the record.
   Call it exactly once, including when preparation fails before a context exists.

```rust
use botwork::core::{
    acceptance::CaseStatus,
    ast::Program,
    eval::{evaluate_program_async, Context},
    operation::OperationControl,
    report::{RecordOptions, Recording, RunIdentity},
    run::RunLimits,
};

let recording = Recording::start(RecordOptions {
    identity: RunIdentity::new("host/run", "host run"),
    ..RecordOptions::default()
});
let rejected = Context::default().attach_recording(&recording).unwrap_err();
assert_eq!(rejected.code().as_str(), "BW7002");
let program = Program::parse_detailed("host.botwork", "Log |\"ready\"|\nAssert |false|")?;
let mut context = Context::with_host_environment(RunLimits::default(), OperationControl::default())?;
context.init_statements();
context.attach_recording(&recording)?;
let result = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()?
    .block_on(evaluate_program_async(&program, context));
let record = recording.finish(result.as_ref().map(|_| ()));
assert_eq!(record.status, CaseStatus::Failed);
assert_eq!(record.statements.len(), 2);
assert_eq!(record.logs[0].text, "ready");
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Outcomes

Records use the [acceptance](acceptance-policy.md) status vocabulary, with one
meaning everywhere:

| Status | Run | Top-level statement |
| --- | --- | --- |
| `succeeded` | Completed without an unhandled error | Completed; a handled error inside it still counts as success |
| `failed` | An unhandled error that is not a stop | Raised that error |
| `expected_failure` | Only a clean body assertion failed under `CaseExpectation::failure` | Not used |
| `unexpected_pass` | Succeeded despite an expected failure | Not used |
| `cancelled` | Run cancellation (BW5001) | Was running when cancelled |
| `timed_out` | A deadline (BW5002) | Was running at the deadline |
| `limit_exceeded` | A resource limit (BW8001) | Exceeded the limit |
| `skipped` | Never started (a `RunSkipped` event with its `SkipReason`) | Not used |
| `interrupted` | Started, but no terminal event was observed | Not used |

A run's status comes from the same `CaseCompletion` decision as case verdicts. A
statement's status comes from its error code: a stop code maps to its stop
status, and any other code to `failed`. `complete` is true only after a terminal
event (`RunFinished` or `RunSkipped`). A record without one stays
`interrupted`, so a pass is never inferred from partial output.

## What a record contains

| Field | Contents |
| --- | --- |
| `format`, `version` | `"botwork-run"`, `1` |
| `identity`, `status`, `complete` | See above |
| `skip_reason` | `suite_setup_failed` or `suite_stopped` for skipped work |
| `started_at`, `finished_at` | Wall-clock RFC 3339 UTC times with microseconds |
| `duration_us` | Monotonic run duration in microseconds |
| `statements`, `omitted_statements` | Top-level statement records, then a count of those not retained |
| `logs`, `omitted_logs`, `logged_bytes` | Captured Log records, the count not retained, and the UTF-8 bytes of every Log |
| `error` | The unhandled error, if any |
| `artifacts`, `omitted_artifacts` | Files or resources the run produced |
| `events` | Number of events folded into the record |

**Statements.** Only the run's own top-level statements are recorded. Each has
an `index`, a `kind` (such as `call`, `assignment`, `while`, or `eventually`),
its `location` (file, byte range, and one-based lines and Unicode-scalar
columns), an `offset_us` from the run start, `duration_us`, `status`, and error
`code`. Nested blocks, loop iterations, custom-statement bodies, and imported
modules' own statements are never records. A loop is therefore one statement,
however many times it iterates. Statements after an unhandled error never start
and have no records.

**Logs.** Each successfully written `Log` value becomes a record with its
top-level `statement` index, `offset_us`, full UTF-8 `bytes`, and a bounded
`text` prefix. `truncated` marks a cut, which always ends with `…[truncated]`.
This includes Logs in asynchronous workers, imported modules, cleanup, and
polling attempts. Log output is still written to stdout; the record is a copy.

**Errors.** `error` holds the stable `code`, derived `status`, a bounded
`message`, the primary `location`, and the codes of its causes in depth-first
order (`causes` and `omitted_causes`). The complete `Diagnostic` remains in
`RunResult::result`.

**Artifacts.** An `Artifact` event records a `kind` and a `path`. Embedded runs
produce none. The CLI [JSON report](json-report.md) attaches assertion evidence
files to the run that produced them.

## Events and ordering

A run's events are numbered from 0 by `sequence`:

| Event | Emitted |
| --- | --- |
| `run_started` | Once, first, with the identity and `started_at` |
| `statement_started`, `statement_finished` | Around each top-level statement, strictly in index order and never overlapping |
| `log`, `artifact` | While the run is active |
| `run_finished` | Once, last, with status, `finished_at`, duration, and error |
| `run_skipped` | As the only event of work that never started |

`RunRecord::from_events` and `RunRecord::apply` fold an event stream into a
record and reject an event that breaks these rules (`RecordError`). The rejected
event leaves the record unchanged. Violations include:

- a sequence gap;
- any event before `run_started`;
- a second start, or a skip after a start;
- any event after a terminal event;
- statements out of order.

Per-run ordering does not depend on other runs, so parallel execution can reuse
the same model. A run stopped inside a statement closes that statement with the
stop status before `run_finished`.

## Limits

| `RecordLimits` field | Default | Beyond the limit |
| --- | --- | --- |
| `statements` | 1,024 | `omitted_statements` counts the rest |
| `logs` | 256 | `omitted_logs` counts the rest |
| `log_bytes` | 256 KiB of captured text per run | Further logs are omitted; `logged_bytes` still counts them |
| `log_record_bytes` | 4 KiB per record | `text` is truncated; `bytes` stays exact |
| `message_bytes` | 4 KiB | The error message is truncated |
| `causes` | 16 | `omitted_causes` counts the rest |
| `artifacts` | 256 | `omitted_artifacts` counts the rest |

Records never retain values, variables, or loop iterations, so their size is
bounded by these limits rather than by run length.

## Serialization and compatibility

Records and events serialize with serde, using snake_case field and status
names. `RunRecord` and its parts also deserialize, so hosts can store records and
load them back. Loading validates the format and version, every diagnostic code
and statement kind (`ast::STATEMENT_KIND_NAMES`), and the status vocabulary.
Unknown fields are ignored. A loaded record is a finished snapshot: further
events fold with default limits. An event is tagged by its `event` field. Adding fields or event kinds
keeps `version` 1; removing or reinterpreting a field increments it. Consumers
should ignore unknown fields and check `format`, `version`, and `complete` first.

## Example

```rust
use botwork::core::{
    acceptance::{CaseStatus, SkipReason},
    report::{Event, EventRecord, RecordLimits, RecordOptions, RunIdentity, RunRecord, timestamp},
    run::{Engine, RunOptions},
};

let options = RunOptions {
    record: Some(RecordOptions {
        identity: RunIdentity::new("checkout/total", "checkout total"),
        ..RecordOptions::default()
    }),
    ..RunOptions::default()
};
let result = Engine::default().run_source(
    "checkout.botwork",
    "|total| = |40 + 2|\nLog |total|\nAssert |total| Equals |41|",
    options,
);
let record = result.record.expect("recording was requested");
assert_eq!(record.status, CaseStatus::Failed);
assert_eq!(record.statements.len(), 3);
assert_eq!(record.statements[2].code, Some("BW9001"));
assert_eq!((record.logs[0].statement, record.logs[0].text.as_str()), (Some(1), "42"));
let json = serde_json::to_value(&record)?;
assert_eq!(json["format"], "botwork-run");
assert_eq!(json["error"]["location"]["line"], 3);

// Work that never started has exactly one event and no start time.
let skipped = RunRecord::from_events(
    RunIdentity::default(),
    RecordLimits::default(),
    &[EventRecord {
        sequence: 0,
        event: Event::RunSkipped {
            identity: RunIdentity::new("checkout/refund", "refund"),
            reason: SkipReason::SuiteSetupFailed,
            recorded_at: timestamp(std::time::SystemTime::now()),
        },
    }],
)?;
assert_eq!((skipped.status, skipped.complete), (CaseStatus::Skipped, true));
assert!(skipped.started_at.is_none());
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Current boundary

The CLI [JSON report](json-report.md) records every file and case run, and
[listeners](listeners.md) receive the same events live, and the
[HTML report](html-report.md) renders the records for people. Suite case programs run as one merged program, so their records do not
yet separate setup, body, and teardown phases. Engine runs cannot be reconciled
after a forced process termination, because their records live in memory.

## Verification

Unit tests cover folding, ordering rejections, interruption, skipped work,
retention counts, bounded text, and status decisions. Integration tests cover:

- statement, log, error, and timing records;
- default and supplied identities;
- expected failures and unexpected passes;
- timeouts, cancellation, and step limits;
- bounded loops, statement counts, and long logs;
- synchronous and asynchronous parity with worker logs;
- imported-module logs, and rejected preparation.

Run them with `cargo test --locked --test run_records`.
[Validation evidence](run-records-evidence.json) records the measured profiles
and mutations.
