# Assertion diagnostics and full operands

Both built-in assertions fail with BW9001. Equality failures report expected and
actual values with their kinds, then a deterministic first difference. Strings
are quoted and control characters escaped. Arrays report the first differing
index, or the first missing/extra element and the two lengths. Maps choose the
lexicographically first differing key, regardless of insertion order. Missing
entries are shown as `<missing>`, distinct from a present None value.

Nested paths use `$`, numeric array indexes, and quoted map keys: for example,
`$["items"][2]["message"]`. A string difference also names the zero-based Unicode
scalar index and the expected/actual characters with their code points, or
`<end of string>`. Comparison uses the language's existing exact numeric rules
and performs no Unicode normalization. This reports one useful difference; it
does not compute an edit script or enumerate every mismatch.

Each value preview and difference path admits at most 256 UTF-8 bytes before an
explicit `…[truncated]` marker. A truncated preview can end inside a quoted value;
it is a readable excerpt, not serialized data. The differing leaf values and
character position remain visible even when a long common prefix is truncated.
Source spans and entered call frames retain the original assertion location,
including imported helpers. The existing whole-diagnostic
[render limits](diagnostic-rendering.md) can additionally truncate exceptionally
large source/stack output without changing the retained evidence.

## Retained evidence and Catch

Built-in failures now use `BWErr::AssertionMismatch { reason, actual, expected }`.
The code remains BW9001. `reason` contains the bounded human explanation;
`actual` and `expected` contain complete typed-JSON strings. Catch exposes these
as `error.details.reason`, `error.details.actual`, and `error.details.expected`.
The Bool assertion keeps its short `Expected true, got false` reason while also
retaining both operands. Rethrow preserves them and the original location.
Host-created `BWErr::AssertionFailed(String)` remains supported as reason-only
assertion evidence.

The typed representation gives every value a `kind` and `value`. Int and Float
remain distinguishable inside collections, and None is represented explicitly:

```json
{"kind":"Array","value":[{"kind":"Int","value":1},{"kind":"Float","value":1.0},{"kind":"None","value":null}]}
```

The seven kinds are None, Int, Float, Bool, String, Array, and Map. None's value
is JSON null; Array elements and Map values are recursively typed nodes. Map
keys are the original strings and JSON object order has no meaning. Finite Float
values retain their f32 value, including negative zero. JSON string escapes
preserve controls, quotes, backslashes, and Unicode. Nesting can approach twice
the value's container depth; consumers must allow that depth when decoding.

This caught failure prints its code, the bounded reason, and both full operands.
The preview shows `1`, while the typed expected operand keeps its Float kind:

<!-- botwork-test: assertion-details -->
```botwork
Try {
    Assert |{items: ["cafe", 1]}| Equals |{items: ["café", 1.0]}|
} Catch |error| {
    Log |error.code|
    Log |error.details.reason|
    Log |error.details.actual|
    Log |error.details.expected|
}
```

Output:

```text
BW9001
Expected {"items": ["café", 1]} (Map), got {"items": ["cafe", 1]} (Map)
  difference at $["items"][0]: expected "café" (String), got "cafe" (String); Unicode scalar index 3: expected 'é' (U+00E9), got 'e' (U+0065)
  full operands: details.expected / details.actual (typed JSON)
{"kind":"Map","value":{"items":{"kind":"Array","value":[{"kind":"String","value":"cafe"},{"kind":"Int","value":1}]}}}
{"kind":"Map","value":{"items":{"kind":"Array","value":[{"kind":"String","value":"café"},{"kind":"Float","value":1.0}]}}}
```

The full strings are measured and admitted under the existing per-diagnostic
and retained-diagnostic text budgets **before** allocation. Full operand data
therefore counts even when the human preview is short. Insufficient retention
produces BW8001 with explicitly omitted assertion evidence. It cannot qualify
as a clean expected assertion failure. Preview truncation alone does not lose
the admitted operands and does not change classification.

Catch conversion has its own [value limits](diagnostic-value-limits.md), and can
reject a large full operand before copying it into a DSL String. That rejection
does not shorten the original diagnostic. Hosts can inspect the admitted error
fields directly or use the CLI artifact option below for unhandled failures.
No automatic file is written for an assertion consumed by Catch.

## CLI artifacts and case identity

Run the deliberately failing [example 37](../examples/37-assertion-diagnostics.suite.botwork):

```sh
cargo run -- --suite examples/37-assertion-diagnostics.suite.botwork \
  --jobs 2 --assertion-artifacts artifacts
```

The `unchanged` row passes. The `changed` row fails, the command exits nonzero,
and stderr identifies `diagnostics/greeting/changed`, dataset `messages`, row
`changed`, the assertion source and custom call, and the difference at
`$["message"]`: scalar index 3 expects `e` and receives `é`. Row/dataset identity
appears on failures both with and without the artifact option, including suites
with shared fixtures.

`--assertion-artifacts PATH` prepares a new `botwork-assertions-*` directory under
PATH before executing scripts. PATH is created if necessary. Each unhandled
assertion, including assertions in a diagnostic's causes, gets a numbered JSON
file. stderr prints each published path. Parallel runs and repeated invocations
have distinct files; existing files are never replaced. The new directory uses
0700 and its files 0600 on Unix, further restricted by the process umask. Other
platforms use their native access-control behavior. Successful invocations can
leave an empty invocation directory.

Each file has `format: "botwork-assertion"` and `version: 1`, with these fields:

| Field | Meaning |
| --- | --- |
| `identity` | Stable CLI occurrence number and file; selected case ID/name, dataset/row IDs when present; suite fixture ID for fixture failures |
| `cause_path` | Array of zero-based cause indexes; empty for the primary diagnostic |
| `code` | Assertion diagnostic category, currently BW9001 |
| `source` | Original filename, byte offsets, and one-based line/scalar-column coordinates when retained |
| `omitted_source` | Bounded filename/byte offsets when emergency evidence retains only an omitted location |
| `reason` | Human explanation, including preview truncation markers |
| `actual_typed_json`, `expected_typed_json` | Strings containing full typed JSON; null unless `operands_complete` is true |
| `actual_excerpt`, `expected_excerpt` | Emergency operand text from an incomplete diagnostic; null otherwise and for legacy reason-only assertions |
| `operands_complete` | True only when both operands were retained without diagnostic omissions |
| `diagnostic_omissions` | Whether this diagnostic is an emergency omission summary |

Suite-owned fixture records use occurrence number 0, an empty identity file,
and `suite_fixture`; they do not invent a case or dataset. The `source` field
still identifies the actual failing statement. Case artifacts include the stable
selected case/row ID even when the assertion originates in another module.

Decode the outer artifact as JSON, check `operands_complete`, then JSON-decode
each typed operand string. When false, the typed fields are null. Any retained
operand text appears in the excerpt fields instead: a bounded human preview when
construction was rejected, or a truncated typed-JSON prefix when a retained
diagnostic was summarized. An excerpt such as `false` can happen to parse as
JSON, so never decode it as an operand. Artifact completeness describes operand
retention, not a passing verdict or a complete run report.

The writer allows at most 256 records, 64 MiB of total JSON bytes, and 64 MiB of
conservatively charged source-coordinate scanning per CLI invocation. Failed
attempts consume their record number and bytes already written. Files publish
only after a complete write using exclusive creation; a failed pending file is
removed. A quota, filesystem, or serialization failure fails reporting and keeps
the original failure in the diagnostic. Previously published artifacts remain.
This is not an fsync/durability guarantee or a multi-file transaction. The existing
batch policy stops new admission and drains started work after reporting fails.

Full JSON run reports, event listeners, secret-marked input redaction, and richer
artifact lifecycle policies remain their separate roadmap tasks.

## Compatibility and verification

The [worker protocol](worker-protocol.md) adds wire tag 9003 for the three-field
structured assertion shape while preserving the existing single-string 9001
shape. Both decode to public diagnostic code BW9001. Older peers reject the new
tag explicitly and need upgrading to exchange structured assertion evidence.

Integration and unit checks exercise Unicode/end-of-string differences, sorted
map selection, missing versus None, numeric boundaries, bounded previews and
full values, exact diagnostic quotas, allocation ordering, Catch/rethrow,
sync/async execution, worker round trips, imported dataset failures, cleanup
causes, file publication collisions, writer quotas, and CLI occurrence identity.
Run them with `cargo test --locked --test assertion_diagnostics --test assertion_artifacts`.
B8 conformance cases and the executed example above cover the language contract.
[Validation evidence](assertion-diagnostics-evidence.json) records the measured
profiles and mutations.
