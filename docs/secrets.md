# Secret inputs

Inputs marked secret are masked from everything a run writes. Every occurrence of
a secret's text becomes `***`, including in:

- script output, statement traces, and diagnostics;
- run records and the JSON and HTML reports;
- listener events and assertion artifacts;
- errors from statements such as file, process, and HTTP operations.

Masking is by exact text, as in CI secret masking; the rules follow.

## Marking inputs

| Option | Effect |
| --- | --- |
| `--secret NAME` | Mark the input variable `NAME`, from `--var` or `--vars-file`, as secret (repeatable) |
| `--secret-env NAME=VARIABLE` | Define input `NAME` as the string value of environment variable `VARIABLE`, marked secret (repeatable) |

`--secret-env` keeps the value off the command line, where other users of the
machine could see it. It applies after `--var` and `--vars-file`.

Marking is checked before any run or output. Each of these fails with BW7001 and
exit status 1:

- an unknown `--secret` name;
- an unset or non-Unicode environment variable;
- a setting without `=`;
- an invalid variable name.

The options conflict with case listing and statement help, which take no inputs.

```text
botwork --file deploy.botwork --vars-file inputs.json --secret credentials \
  --secret-env token=DEPLOY_TOKEN --report-json report.json
```

## What is masked

A secret value may be a string, a number, or any nesting of arrays and maps.
Every string and number leaf is secret, however deep:

- `{"user": "alice", "keys": ["k1-…", 4812]}` masks `alice`, `k1-…`, and `4812`;
- map keys are not secret;
- booleans and None carry too little to mask without masking ordinary output.

Each secret text is masked in every spelling an output can contain:

- the text itself;
- its JSON-escaped form, as in reports, `Format JSON`, and events;
- its quoted form inside a collection, as `Log` shows maps and arrays;
- its URL form encoding, as in query strings.

When one secret contains another, the longer is masked whole. A secret
concatenated into a larger string, logged inside a map, or embedded in an error
message is masked wherever its text appears.

Derived values are not secrets unless marked too, for example a substring, an
uppercase copy, a hash, or a base64 encoding. Mark each such value explicitly.

## Where masking applies

- **Script output and traces.** `Log` output and `--debug` statement traces are
  masked record by record before they reach stdout or stderr. A secret split
  across formatting chunks is still found.
- **Diagnostics.** The console, error messages, and failure recaps are masked,
  whether the error comes from the script or from a file, process, or HTTP
  operation.
- **Assertion failures.**
  - Reasons and previews are built from masked copies of the operands, so a
    character-level difference shows `*`, never a letter of the secret, and a cut
    preview never ends inside one.
  - An embedding host still receives the exact typed operands in the diagnostic.
  - CLI [assertion artifacts](assertion-diagnostics.md) mask them. A masked
    artifact sets `secrets_masked: true` and `operands_complete: false`, and moves
    the masked operands into the excerpt fields.
- **Records, reports, and listeners.**
  - Captured log text and error messages are masked before they are bounded, so
    a truncation never exposes part of a secret.
  - JSON and HTML reports, the report journal, and listener events carry the
    masked records.
  - An HTML source excerpt whose line contains a secret shows the whole line
    masked, without the statement highlight.
- **Shared registry.** Every run and suite fixture registers its secret values in
  one registry for the invocation. Output from any run is masked with every
  secret seen so far, and all secrets are registered before the first run starts.

Masking can make output up to 2 bytes longer per occurrence of a secret shorter
than the 3-byte mask; output limits count the bytes before masking.

## Embedding

`RunOptions::secrets` takes a `core::secret::Secrets` registry. The run masks its
output, traces, and record with it. Hosts that prepare their own `Context` call
`Context::set_secrets`, and `RecordOptions::secrets` masks a `Recording`.

`Secrets::add` marks a value's leaves and `Secrets::add_texts` marks exact texts.
`Secrets::redact` masks a text, `Secrets::mask_value` returns a masked copy of a
value, and `Secrets::writer` wraps an `io::Write`. Clones share one registry, and
`Debug` never prints the texts.

```rust
use botwork::core::{
    grammar::Literal,
    report::RecordOptions,
    run::{Engine, RunOptions},
    secret::Secrets,
};

let secrets = Secrets::default();
secrets.add(&Literal::String("hunter2-token".into()));
let result = Engine::default().run_source(
    "embedded.botwork",
    "Log |\"token=\" + token|",
    RunOptions {
        variables: [("token".to_owned(), Literal::String("hunter2-token".into()))].into(),
        record: Some(RecordOptions::default()),
        secrets: secrets.clone(),
        ..RunOptions::default()
    },
);
assert!(result.result.is_ok());
assert_eq!(result.record.unwrap().logs[0].text, "token=***");
assert_eq!(secrets.redact("Authorization: hunter2-token"), "Authorization: ***");
assert_eq!(format!("{secrets:?}"), "Secrets(1 texts)");
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Limits

- **Exact text only.** Masking matches exact texts, in the spellings above. It
  cannot recognize transformed values or secrets split by the script, for example
  across two `Log` calls.
- **Input files.** The input files themselves still hold the secrets; protect
  them as the source of the secret.
- **Long diagnostics.** A diagnostic other than an assertion failure is bounded
  by its text limit, 8 MiB by default, before masking. A message cut inside a
  secret at that limit could show part of it.
- **Dataset rows.** Values read from suite dataset files are not inputs and
  cannot be marked secret.

## Verification

`cargo test --locked --test secrets` covers:

- one invocation that scans every output file, stdout, and stderr for every
  spelling of every secret: logs, nested values, `Format JSON`, traces, assertion
  failures and artifacts, file errors, a secret written into the source, both
  reports, and a listener;
- a single-file failure's final error;
- masked suite fixtures and cases;
- marking errors;
- embedded runs that mask records and reasons while keeping full operands for
  the host.

Unit tests in `src/core/secret/tests.rs` cover:

- spellings;
- nesting;
- longest-first matching;
- shared registries;
- writers split across chunks;
- masked copies.

[Validation evidence](secrets-evidence.json) records the measured profiles and
mutations.
