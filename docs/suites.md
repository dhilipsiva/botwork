# Named suites and cases

Use `--suite` to discover and run an explicit suite file. The bundled example
uses imported helpers and two independent cases:

```sh
cargo run -- --suite examples/23-named-cases.suite.botwork --jobs 1
cargo run -- --suite examples/23-named-cases.suite.botwork --list-cases
cargo run -- --suite examples/23-named-cases.suite.botwork --tag fast
```

The first command prints `42` and `8`, on separate stdout lines. Stderr identifies
each case's start and outcome and finishes with the selected/succeeded/failed
counts. `--list-cases` prints one JSON metadata object per selected case, with
`id`, suite display name, case display name, and effective tags. Listing evaluates
no imports, custom definitions, inputs, or case bodies.

## Declaration and identity

Each file declares one suite. Optional [datasets](parameterized-cases.md) precede its
Library and cases. Its optional `Library` block comes before the cases
and accepts custom definitions and imports only. A case contains ordinary Botwork
statements. This self-contained example prints `42` and `8`:

<!-- botwork-test: named-suite -->
```botwork-suite
Suite |"arithmetic"| Named |"Arithmetic checks"| Tags |["math"]| {
    Library {
        Double |number| { Return |number * 2| }
    }
    Case |"answer"| Named |"Double twenty-one"| Tags |["fast"]| {
        Log |@{ Double |21| }|
    }
    Case |"small"| {
        Log |@{ Double |4| }|
    }
}
```

The strings immediately after `Suite` and `Case` are required, explicit IDs.
Each ID is 1–128 ASCII bytes, starts with a letter or digit, and otherwise uses
letters, digits, `.`, `_`, or `-`. IDs are case-sensitive. The qualified case ID
is `suite-id/case-id`; parameterized rows append `/row-id`. It does not depend on paths, display names, declaration
positions, tags, or completion order. Keep IDs unchanged when renaming or moving
a test. Duplicate suite IDs across supplied files and duplicate case IDs within
a suite are errors before any case starts.

`Named` is optional; the ID is the default display name. Display names contain
1–512 UTF-8 bytes and can use Unicode. Names are escaped in progress records.
`Tags` is optional and follows `Named` when both are present. Tags are a literal
array of distinct strings, each containing 1–128 UTF-8 bytes without control
characters; at most 32 tags are allowed per declaration. A case inherits suite
tags, with duplicates across those two declarations deduplicated. Matching uses
exact case-sensitive strings, without Unicode normalization.

Declaration words are case-insensitive, like existing control statements. Suite
metadata must be literal: expressions, variables, and statement calls are not
evaluated during discovery. `Suite`, `Case`, `Library`, `Named`, and `Tags` retain
their existing availability as custom-statement names in ordinary scripts; the
suite syntax has a separate parser entry point. Use `--file` for scripts and
`--suite` for suites. The modes cannot be mixed in one invocation.

## Discovery and selection

Pass `--suite` repeatedly to discover several files. Discovery reads those explicit paths and declared dataset files; it does not scan directories, expand globs, or execute imported
libraries. Suite order follows the command line, and case order follows each
declaration. The `.suite.botwork` suffix is a convention, not an implicit scan.
All supplied files and every case body are parsed and validated before effects,
including bodies later excluded by filters. Syntax/control errors or duplicate
IDs prevent the entire selected run from starting.

The default selection includes all discovered cases. Repeat `--case suite/id`
to choose cases (including all their rows), or `--case suite/id/row`
to choose an exact row; repeat `--tag value` to include cases matching **any** listed
tag. Case and tag filters intersect. Any `--exclude-tag` match removes the case.
Filters do not reorder or duplicate cases. Unknown case IDs and empty selections
are errors. An empty completed failed-case list is the explicit exception below.
Unknown tag names simply have no matches; if that leaves no cases, selection fails.

With `--jobs 1`, cases execute in discovery order. With larger job counts,
admission follows that order while completion and Log output may differ. The
[parallel policy](parallel-cli.md) applies: whole Log/status records, independent
limits, finish-all execution after case failures, and stopped admission/draining
after reporter failure. Status is 0 if selected cases and requested output
delivery succeed, 1 for discovery/selection/run/reporting failure, and 2 for
invalid flag usage. A caught ordinary error can still leave a case successful.

Every admitted case creates fresh variables, definitions, module initialization
and cache state, execution counters, and output/retention budgets. Its Library
imports and definitions are installed in that context before its body. Imports
can run their ordinary module initialization once per case; they are not suite
fixtures. Common `--var`/`--vars-file` settings are loaded for each admitted case.
Case timeouts start at admission, after discovery, and include input loading and
case-program preparation. Original suite file coordinates and call frames are
preserved in diagnostics and debug traces. Case bodies retain script control
rules, including Return requiring a custom-statement body.

## Rerun failed cases

```sh
cargo run -- --suite examples/23-named-cases.suite.botwork --failures failed-cases.json
cargo run -- --suite examples/23-named-cases.suite.botwork --rerun-failed failed-cases.json
```

`--failures PATH` writes a small versioned selection record, not a full execution
report. Version 2 contains exactly `format` (`botwork-failed-cases`), `version`
(`2`), `complete` (boolean), and `failed` (distinct qualified case or row IDs). Readers also accept version 1
records containing ordinary case IDs. Reruns never expand a saved parent ID
into rows; each saved ID must identify one runnable case or row. Failed
IDs follow discovery order, regardless of completion order. Runtime errors,
timeouts, and resource-limit stops count as failures. Earlier successful effects
are not rolled back. To update the same record after a rerun, supply both flags
with the same path.

Rerun selection requires a complete record and intersects its IDs with the other
filters. Every recorded ID must still exist in the supplied suite set; stale IDs
are errors rather than silently dropped cases. Changing a case's display name,
path, or body does not invalidate its ID, so a fixed case can be rerun. A complete
record with no failures runs zero cases, reports that fact, and succeeds.

After validating any rerun input, the output writer marks its record incomplete
**before discovery and case effects**. It publishes a complete record only after
all selected cases have terminal results and their status/summary writes succeed.
Interrupted execution, invalid discovery, or reporter failure therefore leaves
an incomplete record that cannot be used as a failed-only selection. Errors
before opening/validating the output leave any old output untouched; check the
command's exit status. On final record-write failure, the command fails even if
the case summary reports successful cases.

Writes use a new temporary file in the destination directory, file sync, atomic
rename, and parent-directory sync on Unix. A persistent `PATH.lock` sidecar holds
an exclusive advisory lock throughout discovery/execution/publication; competing
writers fail before case effects, and process exit releases the lock. Do not
remove the lock file while a writer is active. Existing unrelated output files
and output symlinks are rejected. Linux also rejects lock symlinks and uses
nonblocking opens to reject special record inputs without waiting for FIFO peers.
Use a filesystem supporting the required locking/rename/sync operations; failures
are explicit. Atomic visibility is not a promise against arbitrary external edits,
storage corruption, or rollback by the storage system. Interrupted temporary
files are never treated as completed records.

## Bounds and current scope

Each suite retains the existing 1 MiB source and syntax guards and a cumulative
65,536-node AST allowance across Library and all case bodies. It contains at most
1,024 cases. One discovery accepts at most 64 suites, 8 MiB of combined source,
and 4,096 expanded case/row executions. Dataset files share the discovery
source allowance and have their own [data bounds](parameterized-cases.md#bounds-and-embedding). Filters are bounded to 4,096 case IDs and 32 included/excluded
tags each. Failed-case files are limited to 2 MiB and 4,096 valid IDs; malformed,
duplicate, unknown-field, unsupported-version, and incomplete records fail.
Only admitted case programs are composed; immutable definition/source ownership
can be shared while invocation/module state remains fresh.

Listing uses the requested output limits. Progress/error records have their
separate bounded reporting allowance. Discovery/storage and preparation execute
off the async executor. These byte/count limits do not supply a hard deadline
for blocked filesystem calls or cleanup. Discovery is outside per-case timeouts.

Fixtures, awaited teardown, assertion policy, rich shared
events/reports, and coordinated signal handling retain their own roadmap tasks.
They can build on these stable IDs without treating a library import as a case.

## Validation

Rule T1 connects this contract to the conformance corpus. Model tests cover
metadata, source/AST/count boundaries, literal discovery, declaration layout,
selection, and ordinary-script compatibility. CLI tests execute imported helpers,
fresh case state, filters, failure-only reruns, bounded concurrent admission,
queued deadlines, interrupted records, exclusive writers, and output failures.
Storage tests verify exact ID capacity, rejected writes preserving old records,
and temporary collisions. The documentation snippet and example 23 execute in
the normal test suite. [Validation evidence](suites-evidence.json) records local
profile checks, frozen mutation inventories, repairs, and remaining observations.
