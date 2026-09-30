# Configuration

Botwork has no configuration file and reads no `BOTWORK_*` environment
variables. An invocation is configured entirely by its
[command-line options](cli.md). A script is configured by the inputs, secrets,
directory, and environment it runs with.

## What configures a run

| Setting | Source | Precedence and scope | Details |
| --- | --- | --- | --- |
| Input variables | `--vars-file PATH`, `--var NAME=JSON` | Files apply in order, then every `--var` in order, whatever the argument order. Each run gets its own copy. | [Input variables](input-variables.md) |
| Secrets | `--secret NAME`, `--secret-env NAME=VARIABLE` | Masks the input's values in every output. `--secret-env` reads the value from the process environment. | [Secrets](secrets.md) |
| Working directory | The directory the command starts in | Relative paths in file and process statements, and relative variable-file paths, resolve against it. | [Operating-system statements](operating-system.md) |
| Environment | The process environment | Scripts see the environment `botwork` was started with. | [Operating-system statements](operating-system.md) |
| Imports | `Import \|"path"\| As \|alias\|` | Paths resolve relative to the importing file, not the working directory. | [Local modules](language.md#local-modules) |
| Resource limits | `--max-*` options and timeouts | Apply to every run of the invocation. | [Limits](cli.md#limits) |
| Concurrency | `--jobs` | Bounds simultaneous runs; each run still has its own variables and state. | [Parallel runs](parallel-cli.md) |
| Outputs | `--report-json`, `--report-html`, `--listener`, `--failures` | Written for the whole invocation. | [Reports and outputs](reporting.md) |

## Defaults

With no options, `botwork --file a.botwork` runs with:

- no input variables and no secrets;
- the current directory and the full process environment;
- the default [limits](cli.md#limits): 1,000,000 steps, call depth 32,
  evaluation depth 96, 8 MiB per output record, 32 MiB of output per run, and
  a stop grace of 2 seconds;
- console output only;
- up to four simultaneous runs, when several `--file` options are given.

## Hosts that embed Botwork

A Rust program that embeds the interpreter configures the same things through
`RunOptions`: root variables, the working directory, whether the environment
is inherited, an environment overlay, a timeout, and `RunLimits`. It can also
register its own statements. See [embedded runs](embedded-runs.md) and
[extending Botwork](extending.md).

## Editors

The editor packages start `botwork --lsp`, which takes no other option. In VS
Code, the `botwork.server.path` setting names the executable. See
[editor support](editors.md).
