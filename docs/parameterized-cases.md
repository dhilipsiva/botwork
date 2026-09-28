# Parameterized cases and reusable datasets

A case can run once for every row of a named dataset. Each row has an explicit
ID, its own input value, and an independent outcome. A failed row leaves other
selected rows runnable under the existing finish-all policy.

Declare datasets before the optional Library and the cases. This example prints
`42`, `8`, and `0` on separate lines:

<!-- botwork-test: parameterized-suite -->
```botwork-suite
Suite |"arithmetic"| Tags |["math"]| {
    Dataset |"numbers"| Named |"Example numbers"| {
        Row |"answer"| Values |{number: 21}|
        Row |"small"| Values |{number: 4}|
        Row |"zero"| Tags |["edge"]| Values |{number: 0}|
    }
    Library {
        Double |number| { Return |number * 2| }
    }
    Case |"double"| Named |"Double each number"|
        Using |"numbers"| As |data| {
        Log |@{ Double |data.number| }|
    }
}
```

`Using` references one dataset declared in the same suite; `As` names the
case-sensitive variable holding that row's value. This binding replaces a common
`--var` or `--vars-file` input of the same name for this run only. Other common
inputs remain available. The binding is copied only when the row is admitted,
checked against the run's value limits, and charged to its retained input storage.
Root custom definitions can read it through their ordinary lexical scope.
Imported modules keep their existing independent module-global scope.

Each row gets fresh variables, definitions, module initialization/cache state,
budgets, and an admission-time timeout. Assigning a replacement value to the row
variable affects only that invocation. Cases may reuse the same dataset with
different binding names. Each case uses one dataset; there is no implicit product
of several datasets. Cases without `Using` still run once.

## Literal data and external files

Values accept `none`, booleans, signed 32-bit integers, finite 32-bit floats,
escaped strings, arrays, and maps, including empty and nested collections.
`none` is a dataset literal; this does not add it to ordinary expression syntax.
Numbers retain their types without string conversion. Integer overflow and
non-finite floats are discovery errors. Map keys use ordinary bare or quoted
syntax; duplicate keys retain the last value. Expressions, variables, calls,
imports, and executable statements are not allowed inside data. Existing
whitespace, LF/CRLF, `#`/`###` comments, and trailing-comma rules apply.

To share data across files, place a single inline Dataset declaration in a
`.dataset.botwork` file and reference it with
`Dataset |"local-alias"| From |"relative/path.dataset.botwork"|`.
Use the local alias in the case's `Using`; the file's own dataset ID can differ.
External references take a nonempty literal path of at most 4,096 UTF-8 bytes
without NUL. Paths resolve against the supplied suite file's directory; absolute
paths also work. Filename suffixes are conventions. A data file cannot reference
another data file.

The [bundled suite](../examples/24-parameterized-cases.suite.botwork) uses both
inline data and a [reusable file](../examples/datasets/numbers.dataset.botwork):

```sh
cargo run -- --suite examples/24-parameterized-cases.suite.botwork --jobs 1
cargo run -- --suite examples/24-parameterized-cases.suite.botwork --case parameters/double/small
cargo run -- --suite examples/24-parameterized-cases.suite.botwork --tag edge --list-cases
```

The first command prints `42`, `8`, `0`, and `hello`. Discovery reads every
declared dataset, including unused/excluded data, before listing or running any
case. Files must be valid UTF-8 ordinary files; Linux rejects FIFOs without
waiting for a peer. Symlinks are permitted. Canonical file paths share one
immutable parsed snapshot within a discovery, so aliases neither reread nor
recount that file. Later filesystem edits do not change queued rows. Separate
CLI invocations rediscover data. Discovery does not evaluate Library imports.

## Identity, selection, and results

The qualified row ID is `suite/case/row`. Dataset aliases, file paths, values,
display names, and row order do not form its identity. IDs use the existing
1–128 byte ASCII [suite ID rules](suites.md); row IDs must be unique within
their dataset and dataset aliases unique within a suite. Reordering or repairing
data while keeping its row ID preserves failed-row selection.

Datasets and rows accept optional `Named` and `Tags` in that order, with the
same limits as suite metadata. Effective tags combine suite, case, dataset, and
row tags. External datasets contribute their file's tags. The display name is
`case name / row name`; it is escaped in status records.

`--case suite/case` selects all rows of that case; `--case suite/case/row`
selects just that row. Tag inclusion/exclusion still intersects case selection.
Execution and listing preserve supplied suite order, then case order, then row
order; filter order never duplicates or reorders work. Parallel completion can
differ. Listing emits the existing metadata fields plus `case` (parent ID),
`dataset` (local alias), and `row` (ID and display name) for parameterized rows.
Ordinary-case listing objects keep their existing fields.

Every admitted row receives its own start and terminal status. Failure records
contain only failed leaf IDs in discovery order. Writers now produce version 2;
readers accept version 1 ordinary `suite/case` IDs and version 2 ordinary or row
IDs. Version 1 row IDs are invalid. Reruns require exact runnable IDs: a saved
`suite/case` failure cannot expand into all rows if that case was subsequently
parameterized. Stale row IDs fail selection. The existing complete/incomplete,
locking, atomic publication, and same-path rerun rules are unchanged.

## Bounds and embedding

Discovery permits at most 64 unique datasets, 1,024 rows per dataset, 4,096
defined rows, and 65,536 parsed literal nodes across unique dataset owners.
Overwritten map values count toward parsed nodes. Unused datasets count too.
One suite permits at most 64 dataset declarations and 1,024 case declarations.
Expanded executions are limited to 4,096 across all suites **before filtering**.
Reusing a dataset counts its data once but each case's executions separately.

Each suite/data source is limited to 1 MiB; the combined unique-source allowance
is 8 MiB. Existing syntax guards apply, including a maximum nesting of 32 across
declaration braces, expression pipes, and collection delimiters. Row values also
obey default value limits: depth 64, 65,536 nodes, 16,384 entries per container,
1 MiB per string, 64 KiB per key, and 8 MiB payload. The syntax/source limits can
reject text before the corresponding value ceiling is reached. Case/Library
AST nodes retain their separate cumulative limit. Discovery runs off the async
executor and outside row timeouts; byte/count bounds do not promise a hard
deadline for filesystem calls.

`Suite::parse` and `Dataset::parse` do no file I/O. Hosts explicitly supply
external datasets with `Suite::resolve_datasets`; unresolved declarations fail
selection. Hosts choose their own file/cache policy. For each selected row, use
`SelectedCase::bind_inputs` before running its composed program. This example
uses an in-memory resolver and checks an observable result:

```rust
use botwork::core::{
    grammar::Literal,
    run::{Engine, RunOptions, RunOutcome},
    suite::{Dataset, Selection, Suite},
};
use std::{collections::BTreeMap, sync::Arc};

let data = Arc::new(Dataset::parse("data", r#"
Dataset |"numbers"| { Row |"answer"| Values |21| }
"#)?);
let suite = Suite::parse("suite", r#"
Suite |"math"| {
    Dataset |"input"| From |"shared"|
    Case |"double"| Using |"input"| As |number| {
        |answer| = |number * 2|
    }
}
"#)?.resolve_datasets(|path, _| {
    assert_eq!(path, "shared");
    Ok(Arc::clone(&data))
})?;
let cases = Selection::default().select(&[Arc::new(suite)])?;
assert_eq!(cases[0].id(), "math/double/answer");
let mut options = RunOptions::default();
options.variables = cases[0].bind_inputs(BTreeMap::new(), &options.limits.values)?;
let result = Engine::default().run_program(&cases[0].program(), options);
assert_eq!(result.outcome(), RunOutcome::Succeeded);
assert!(matches!(result.variables["answer"], Literal::Int(42)));
# Ok::<(), botwork::core::diagnostic::Diagnostic>(())
```

Rule T2, model and CLI tests cover typed values, boundaries, discovery errors,
tag/ID selection, state isolation, file snapshots, concurrent sibling progress,
queued deadlines, history migration, and reruns after repair. The documentation
examples and example 24 execute in the normal suite.
[Validation evidence](parameterized-cases-evidence.json) records the profile
matrix, frozen mutation campaigns, exact replays, fixture repairs, and remaining
observations. [Fixtures](fixtures.md) wrap each selected row with case setup and
teardown and can provide immutable shared suite inputs. Assertions, tabular/JSON
data conversion, and full structured reports retain their separate roadmap tasks.
