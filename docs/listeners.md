# Report listeners

A listener receives an invocation's execution events while they happen. These
are the same events that [run records](run-records.md) fold and the
[JSON report](json-report.md) summarizes. The CLI streams them as JSON Lines to a
listener program. Embedded hosts attach an `EventObserver` to a recording and
deliver through a bounded `Dispatcher`.

This page defines:

- the order events arrive in;
- what happens when a listener fails;
- how slow listeners are handled.

## CLI listeners

```text
botwork --suite checkout.suite.botwork \
  --listener python3 --listener-arg dashboard.py
```

| Option | Meaning |
| --- | --- |
| `--listener PROGRAM` | Start `PROGRAM` and write the event stream to its stdin |
| `--listener-arg ARG` | One argument for the program (repeatable; values may start with `-`) |
| `--listener-queue EVENTS` | Events that may wait for delivery before a slow listener is detached (1–1,048,576; default 8,192) |
| `--listener-timeout-ms MS` | How long to wait for the listener after the last event (1–3,600,000; default 10,000) |

The program runs directly, without a shell. It inherits the environment, the
working directory, and stderr. Its stdout is redirected to Botwork's stderr, so
script `Log` output alone stays on stdout. It runs in its own process group.

The listener starts after the [JSON report](json-report.md) marker and before
suite discovery or any run, so its stream covers every run. A program that cannot
be started stops the invocation before anything runs.

### Example

<!-- botwork-test: listener-stream -->
```botwork
Log |"checking"|
Assert |1 + 1| Equals |2|
```

Running this as `listener-stream.botwork` with
`--listener sh --listener-arg -c --listener-arg 'cat > events.jsonl'` writes the
following. Timestamps, offsets, and durations vary between runs:

```jsonl
{"position":0,"event":"stream_started","format":"botwork-events","version":1,"mode":"file","started_at":"2026-09-29T07:04:05.880278Z"}
{"position":1,"run":1,"sequence":0,"event":"run_started","identity":{"id":"listener-stream.botwork","name":"listener-stream.botwork","dataset":null,"row":null},"started_at":"2026-09-29T07:04:05.880404Z"}
{"position":2,"run":1,"sequence":1,"event":"statement_started","index":0,"kind":"call","location":{"file":"listener-stream.botwork","start_byte":0,"end_byte":16,"line":1,"column":1,"end_line":1,"end_column":17},"offset_us":3479}
{"position":3,"run":1,"sequence":2,"event":"log","statement":0,"offset_us":3691,"bytes":8,"text":"checking","truncated":false}
{"position":4,"run":1,"sequence":3,"event":"statement_finished","index":0,"status":"succeeded","code":null,"offset_us":3820}
{"position":5,"run":1,"sequence":4,"event":"statement_started","index":1,"kind":"call","location":{"file":"listener-stream.botwork","start_byte":17,"end_byte":42,"line":2,"column":1,"end_line":2,"end_column":26},"offset_us":3843}
{"position":6,"run":1,"sequence":5,"event":"statement_finished","index":1,"status":"succeeded","code":null,"offset_us":3902}
{"position":7,"run":1,"sequence":6,"event":"run_finished","status":"succeeded","finished_at":"2026-09-29T07:04:05.884364Z","duration_us":3959,"error":null}
{"position":8,"event":"stream_finished","finished_at":"2026-09-29T07:04:05.884404Z","status":"succeeded","cases":{"total":1,"succeeded":1,"expected_failure":0,"failed":0,"unexpected_pass":0,"skipped":0,"cancelled":0,"timed_out":0,"limit_exceeded":0,"interrupted":0},"fixture_failures":0}
```

## The stream

Each line is one JSON object. `position` numbers the lines from 0 without gaps,
and `event` names the kind:

| `event` | Fields | Sent |
| --- | --- | --- |
| `stream_started` | `format` (`"botwork-events"`), `version` (`1`), `mode` (`file`, `batch`, or `suites`), `started_at` | First, once |
| Run events | `run`, `sequence`, and the [run record event](run-records.md#events-and-ordering) fields | As runs execute |
| `fixture_finished` | `suite`, `status`, `error` | When a shared suite fixture finishes |
| `stream_finished` | `finished_at`, `status`, `cases`, `fixture_failures` | Last, once, after every run and fixture |

- **Run events** are `run_started`, `statement_started`, `statement_finished`,
  `log`, `artifact`, `run_finished`, and `run_skipped`. `run` is the run's number
  in selection order: the JSON report's `number`, and a batch's `[run N]`.
  `sequence` is the event's position within its run.
- **Log text** follows the report's retention limits. Each `log` event carries up
  to 1 KiB of text for the first 64 logs of a run, within 16 KiB. Later events
  carry an empty `text` with `truncated: true`, and `bytes` always counts the full
  output. The complete output is still written to stdout.
- **Fixtures.** Logs inside shared suite fixtures are not events; the fixture's
  outcome is.
- **Artifacts.** A [WebDriver screenshot](webdriver.md#screenshots) is an
  `artifact` event when it is taken. Assertion artifacts are attached to JSON
  report records after a run finishes, and are not streamed.
- **The trailer.** `status`, `cases`, and `fixture_failures` in `stream_finished`
  describe execution: the verdict of every run and fixture. Report and listener
  delivery can still fail after the trailer, and the exit status then becomes 1.
  A listener that received `stream_finished` and exits 0 in time has seen the
  final verdict.
- **Interruption.** After an [interruption](terminal-outcomes.md#interruption),
  cancelled runs still end with `run_finished`, and `stream_finished` reports
  `status: "interrupted"`.
- **Incomplete streams.** A stream that ends without `stream_finished` is
  incomplete. This happens when the invocation stopped early, for example after a
  discovery error. The listener still sees the end of its input.

### Ordering

- Lines arrive in exactly the order they were queued, and `position` records that
  order.
- `stream_started` is first and `stream_finished` is last.
- **Within a run**, events arrive in `sequence` order without gaps. They start
  with `run_started`, or with `run_skipped` as a skipped run's only event, and end
  with `run_finished`.
- **Across runs**, events of concurrent runs interleave in the order they
  happened. With `--jobs 1`, runs do not overlap.
- A shared fixture's `fixture_finished` precedes the `run_skipped` events its
  failure causes.
- Folding one run's events reproduces the record that the JSON report holds for
  that run: its `events` count equals the number of streamed events.

## Listener failures

A listener fails when:

- a write to its stdin fails (for example, it closed stdin or exited early);
- it exits with a non-zero status, or is killed, after its stdin is closed;
- it falls behind by the queue size;
- it misses the close timeout.

A failure never changes a run's status and never stops or slows execution. The
listener is detached: no further events are queued for it, and its stdin is
closed once any write in progress returns. At the end of the invocation:

- the listener error, including how many events were delivered, is printed;
- the exit status is 1;
- the [JSON report](json-report.md) verdict records `delivery: "failed"`. The
  report stays complete, and its runs keep their statuses.

## Slow listeners

Runs never wait for a listener.

- **Queueing.** Events are queued without blocking and written by a dedicated
  thread, one line at a time, flushing each line.
- **Falling behind.** A listener that does not keep up first fills its pipe, then
  the queue. The event that would exceed `--listener-queue` detaches it instead of
  blocking or silently dropping events. The queued events are discarded, so
  memory stays bounded.
- **Closing.** After the last event, the listener's stdin is closed, and Botwork
  waits up to `--listener-timeout-ms` for queued events to be written and the
  program to exit.
- **Timeouts.** A listener still running at the timeout fails. On Linux and
  macOS its process group is killed while the listener is still unreaped, so
  processes it started are stopped too and a reused group ID is never
  signalled. On Windows only the listener itself is killed.

## Embedded listeners

`RecordOptions::observer` takes an `EventObserver`, called with each event the
run records, in `sequence` order. The observer runs while the run's recorder is
locked, so it must return promptly.

`core::listener::Dispatcher` provides the delivery policy for slow or fallible
work. `ListenerSender::send` queues an item without blocking. A dedicated thread
calls `Listener::deliver` in queue order, then `Listener::finish` once.
`Dispatcher::close` returns a `ListenerOutcome`:

| Field | Contents |
| --- | --- |
| `accepted`, `delivered` | Items queued and items delivered |
| `failure` | The first `ListenerFailure`, or `None` |
| `finished` | `false` when the delivery thread was still running at the close deadline |

A `ListenerFailure` is one of:

- `Rejected`, when `deliver` or `finish` returned an error;
- `Panicked`;
- `Overflow`, when the queue was full;
- `Timeout`, when `close` gave up.

A panicking listener is caught and detached; the host keeps running.

```rust
use botwork::core::{
    listener::{Dispatcher, Listener, ListenerOptions},
    report::{EventObserver, EventRecord, RecordOptions},
    run::{Engine, RunOptions},
};
use std::sync::{Arc, Mutex};

/// Keeps event names; a real listener might post them to a dashboard.
struct Names(Arc<Mutex<Vec<String>>>);

impl Listener<EventRecord> for Names {
    fn deliver(&mut self, event: &EventRecord) -> Result<(), String> {
        let value = serde_json::to_value(event).map_err(|error| error.to_string())?;
        let name = value["event"].as_str().unwrap_or_default().to_owned();
        self.0.lock().unwrap().push(name);
        Ok(())
    }
}

let names = Arc::new(Mutex::new(Vec::new()));
let dispatcher = Dispatcher::spawn(Names(Arc::clone(&names)), ListenerOptions::default())?;
let sender = dispatcher.sender();
let options = RunOptions {
    record: Some(RecordOptions {
        observer: Some(EventObserver::new(move |event| {
            sender.send(event.clone());
        })),
        ..RecordOptions::default()
    }),
    ..RunOptions::default()
};
let result = Engine::default().run_source("listened.botwork", "Log |\"ready\"|", options);
assert!(result.result.is_ok());
let outcome = dispatcher.close();
assert_eq!((outcome.failure, outcome.delivered, outcome.finished), (None, 5, true));
assert_eq!(
    *names.lock().unwrap(),
    ["run_started", "statement_started", "log", "statement_finished", "run_finished"]
);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Versioning

The first line identifies the stream as `botwork-events` version 1. Consumers
should ignore unknown event kinds and fields. Adding event kinds or fields keeps
version 1, as with run records. Removing a field, or changing its type or what a
value means, increments the version.

## Verification

`cargo test --locked --test listeners` covers:

- batch framing, and agreement with JSON report records;
- suite rows, skips, and fixture outcomes;
- listener exit and write failures;
- a slow listener detached by the queue limit while every Log still reaches
  stdout;
- a hung listener stopped at the close timeout together with its own processes;
- setup and argument errors;
- incomplete streams;
- the example above.

Unit tests in `src/core/listener/tests.rs` cover:

- delivery order from concurrent senders;
- rejection, panics, and finish errors;
- overflow without blocking senders;
- close timeouts.

[Validation evidence](listeners-evidence.json) records the measured profiles and
mutations.
