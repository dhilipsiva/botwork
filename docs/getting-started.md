# Getting started

This guide takes you from installation to a first script, a first failure, and
a complete acceptance-test workflow with reports. Every script and command below
is executed by the test suite, so the output shown is what you will see.

## Install

botwork builds with a stable Rust toolchain; install one from
[rustup.rs](https://rustup.rs) if you do not have it. Linux is the tested
platform, and isolated process workers are Linux-only.

```sh
git clone https://github.com/dhilipsiva/botwork.git
cd botwork
cargo install --path . --locked
```

This installs the `botwork` binary into Cargo's bin directory, usually
`~/.cargo/bin`. Check that it runs:

```sh
botwork --version
```

To try botwork without installing it, run `cargo run --` in the checkout
wherever this guide writes `botwork`.

## Your first script

Save this as `hello.botwork`:

<!-- botwork-test: hello -->
```botwork
Log |"Hello, botwork!"|
```

Run it:

```sh
botwork --file hello.botwork
```

It prints `Hello, botwork!` and exits with status 0.

Statements are sentences. Each value between pipes, such as `|"Hello, botwork!"|`,
is a parameter: a literal, a variable, or an expression.

## Variables and your own statements

Save this as `greeting.botwork`:

<!-- botwork-test: greeting -->
```botwork
# Input variables arrive before the script runs; give this one a default.
If |!@{ Variable Exists |"name"| }| {
    |name| = |"Ada"|
}

# A custom statement is a sentence with parameters.
Greet |person| with |greeting| {
    Return |greeting + ", " + person + "!"|
}

# Calls ignore case and extra spaces between words.
Log |@{ greet |name|  WITH |"Welcome"| }|

|prices| = |[3, 4, 5]|
|total| = |0|
For |price| In |prices| {
    |total| = |total + price|
}
If |total > 10| {
    Log |"Large order"|
} Else {
    Log |"Small order"|
}
Log |total|
```

```sh
botwork --file greeting.botwork
```

```text
Welcome, Ada!
Large order
12
```

A few things to notice:

- `|name| = |"Ada"|` assigns a variable. Both sides take pipes. Variable names
  are case-sensitive.
- `@{ ... }` calls a statement inside an expression and uses its result. The
  call inside keeps its own pipes, so in `|@{ greet |name| WITH |"Hi"| }|` the
  outer pipes hold the whole `@{ ... }`, and each inner pair holds one argument.
- `Greet |person| with |greeting|` defines a statement. Statement names are
  matched without regard to case or spacing, so `greet |name|  WITH |...|`
  calls it.
- A definition takes effect when its line runs, so define a statement above
  the first line that calls it.
- A statement's parameters and the variables it assigns are its own. Calling
  `Greet` never changes the caller's variables, even ones with the same names;
  see [variables and scope](language.md#variables-and-invocation-scope).
- To use statements from another file, `Import |"pricing.botwork"| As |pricing|`
  runs that file, and its statements are then called with the prefix, as in
  `@{ pricing::Total of |3| at |4| }`; see [local modules](language.md#local-modules).

Input variables are JSON values supplied on the command line or in files. Run
the same script for someone else:

```sh
botwork --file greeting.botwork --var 'name="Grace"'
```

It greets Grace instead. See [input variables](input-variables.md) for
`--vars-file` and precedence.

## When a script fails

Save this as `first-failure.botwork`. The expectation is wrong on purpose:

<!-- botwork-test: first-failure -->
```botwork
Total of |prices| {
    |total| = |0|
    For |price| In |prices| {
        |total| = |total + price|
    }
    Return |total|
}

Assert |@{ Total of |[2, 3]| }| Equals |6|
```

```sh
botwork --file first-failure.botwork
```

The run exits with status 1 and explains why on stderr:

```text
first-failure.botwork:9:1-9:43: [BW9001] Assertion failed: Expected 6 (Int), got 5 (Int)
  difference at $: expected 6 (Int), got 5 (Int)
  full operands: details.expected / details.actual (typed JSON)
  source: Assert |@{ Total of |[2, 3]| }| Equals |6|
  in `assert|param|equals|param|` called at first-failure.botwork:9:1
  help: Inspect the condition or compared values; fix the behavior or update the expectation deliberately.
```

Read a diagnostic from the top:

- **Where.** `first-failure.botwork:9:1-9:43` is the file, then the line and
  column range of the failing statement.
- **What.** `BW9001` is a stable code; [diagnostic codes](diagnostics.md) lists
  every code and its category. The message and the `difference at $` line show
  the expected and actual values.
- **How to fix it.** The `source` line quotes the statement, and `help` suggests
  the next step.

Changing `6` to `5` makes the script pass. These commands help while you work:

- `botwork --statement-help 'Assert |actual| Equals |expected|'` shows a
  statement's parameters, return kinds, and errors.
- `botwork --list-statements` lists every built-in statement.
- `botwork --file first-failure.botwork --debug` traces each top-level statement
  on stderr.

Scripts can also handle failures themselves with `Try`, `Catch`, and `Finally`;
see [the language guide](language.md#trycatch) and [cleanup](cleanup.md).

## An acceptance-test workflow

Acceptance tests live in suites: files ending in `.suite.botwork` that group
named cases with shared setup, libraries, and data. Save this as
`checkout.suite.botwork`:

<!-- botwork-test: checkout -->
```botwork-suite
Suite |"checkout"| Named |"Checkout totals"| Tags |["smoke"]| {
    Dataset |"orders"| {
        Row |"single"| Values |{items: [5], expected: 5}|
        Row |"several"| Values |{items: [2, 3, 4], expected: 9}|
    }
    Library {
        Total of |prices| {
            |total| = |0|
            For |price| In |prices| {
                |total| = |total + price|
            }
            Return |total|
        }
    }
    SuiteSetup {
        |currency| = |"EUR"|
    }
    Case |"empty"| Named |"An empty cart totals zero"| {
        Assert |@{ Total of |[]| }| Equals |0|
    }
    Case |"total"| Named |"Order totals"| Using |"orders"| As |order| {
        Assert |@{ Total of |order.items| }| Equals |order.expected|
        Log |currency|
    }
}
```

- The **library** defines statements that every case can call.
- **SuiteSetup** runs once, before the cases, and shares its variables with them.
- The **dataset** turns the `total` case into one case per row.

Run the suite:

```sh
botwork --suite checkout.suite.botwork
```

Progress and the summary go to stderr, and each case's `Log` output goes to
stdout. Cases run in parallel, so the order of these lines can vary:

```text
[suite checkout] setup started
[suite checkout] setup succeeded
[case checkout/empty] started: "An empty cart totals zero"
[case checkout/total/single] started: "Order totals / single"
[case checkout/total/several] started: "Order totals / several"
[case checkout/empty] succeeded: "An empty cart totals zero"
[case checkout/total/single] succeeded: "Order totals / single"
[case checkout/total/several] succeeded: "Order totals / several"
[suite checkout] teardown succeeded
[cases] 3 selected: 3 succeeded, 0 failed
```

Each case has a stable ID, such as `checkout/total/several`. List the cases, or
run a single one, by ID:

```sh
botwork --suite checkout.suite.botwork --list-cases
botwork --suite checkout.suite.botwork --case checkout/total/several
```

`--tag` and `--exclude-tag` select cases by tag; tags on a suite apply to all of
its cases.

In continuous integration, run cases in parallel, write reports, and record
failures:

```sh
botwork --suite checkout.suite.botwork --tag smoke --jobs 2 --report-json report.json --report-html report.html --failures failed.json
```

This command:

- runs up to two cases at a time;
- writes a [JSON report](json-report.md) for tools and a self-contained
  [HTML report](html-report.md) for people;
- writes the IDs of failed cases to `failed.json`. Botwork also keeps a
  `failed.json.lock` beside it, so that two runs never write the record at
  once; see [rerun failed cases](suites.md#rerun-failed-cases).

After fixing the failures, rerun only the cases that failed:

```sh
botwork --suite checkout.suite.botwork --rerun-failed failed.json
```

After a clean run the record lists no failures, so the rerun selects no cases
and succeeds.

The exit status tells a CI job what happened:

| Exit status | Meaning |
| --- | --- |
| 0 | Every selected case and shared setup succeeded |
| 1 | Something failed, timed out, or was skipped |
| 2 | Invalid command-line usage; nothing ran |

## Where next

The [documentation index](README.md) lists every page. Good next steps:

- [Syntax reference](syntax.md): every form a script or suite can use.
- [Command-line reference](cli.md): every option and exit status.
- [Configuration](configuration.md): inputs, secrets, environment, and limits.
- [Reports and outputs](reporting.md): choosing a report and reading its version.
- [Checking scripts](check.md): find mistakes without running a script.
- [Formatting](format.md): rewrite files in canonical layout.
- [Language server](lsp.md): diagnostics, completion, hover, and navigation
  in your editor.
- [Statement reference](statements.md): every built-in statement's signature,
  kinds, errors, and an example.
- [Language guide](language.md): values, operators, control flow, and errors.
- Built-in statements: [built-ins](builtins.md), [collections](collections.md),
  [strings](strings.md), [dates and times](datetime.md),
  [files and environment](operating-system.md), [processes](processes.md),
  [HTTP](http.md), and [JSON and CSV data](structured-data.md).
- Suites: [named cases](suites.md), [fixtures](fixtures.md),
  [parameterized cases](parameterized-cases.md), and
  [console output](console-report.md).
- Robustness: [assertion diagnostics](assertion-diagnostics.md),
  [polling and retries](polling.md), [cleanup](cleanup.md), and
  [cancellation](cancellation.md).
- Embedding botwork in Rust: [embedded runs](embedded-runs.md).
