# Input Variables

Pass repeatable `--vars-file PATH` and `--var NAME=JSON` flags with `--file`. Files must be UTF-8 JSON objects mapping variable names to values. Paths are relative to the launching working directory, independently of the script's directory. See [example 20](../examples/20-input-variables.botwork), its [defaults](../examples/inputs/defaults.json), and the runnable command in the README.

## Values and Names

| JSON | Botwork value |
| --- | --- |
| `null` | None |
| `true`, `false` | Bool |
| Integer token, including `-0` | Checked i32, from -2147483648 through 2147483647 |
| Decimal or exponent token, including `1.0`, `1e0` | f32 rounded directly from the token; reject overflow/non-finite results |
| String | String with standard JSON escapes and Unicode |
| Array, object | Array, Map; recursively apply these conversions |

Float underflow rounds to zero; negative floating zero retains its sign. Large integer tokens fail instead of silently becoming floats. JSON supports escapes/exponents beyond the DSL source-literal syntax. Arrays/maps are ordinary owned values, and nested map keys may be any Unicode string. Values are data: neither DSL expressions nor shell substitutions are evaluated by Botwork. Quote arguments appropriately for your shell; for example, Bash accepts `--var 'message="hello=world"'`.

Top-level names must exactly match DSL identifiers, including Unicode XID characters and `_`. Names are case-sensitive; lowercase `true`, `false`, `and`, and `or` are reserved. Leading/trailing whitespace, dots, brackets, and assignment syntax are rejected. Flags split at the first `=`; strings still need JSON quotes.

## Precedence and Scope

1. Read variable files in their flag order; later files replace earlier bindings.
2. Apply explicit `--var` settings in their flag order, after **all** files regardless of argument interleaving.
3. Install the result in the entry script's root scope before execution.

Replacement is whole-value replacement, without deep merging. Within a JSON object, duplicate keys use the last value before conversion. Each file/flag is validated even if a later input would replace it. A script can assign over inputs; custom statements read them lexically, while parameters/local assignments shadow them. Imported modules retain independent globals; pass input values as arguments to module statements.

## Failure Behavior

Malformed JSON, invalid names, range failures, unreadable/non-UTF-8 input files, and nesting beyond 128 containers return BW7001 and CLI status 1 before script effects or debug traces. The root object counts toward depth. Diagnostics identify the file or numbered `--var`, a collection path, and JSON syntax position when available; they avoid reproducing the complete input payload. Input validation precedes entry-script loading/parsing. Flag misuse returns Clap status 2; input flags conflict with statement-list/help modes.

Input failure cannot be caught by the script because execution has not begun. JSON size and collection-length budgets, secret-marked redaction, and broader interpreter limits remain separate TODOs; this parser is not a sandbox.

## Rust Hosts

`core::input::{parse_variables, parse_variable, load_variables}` return owned values with detailed errors. `Context::set_input_variables` validates all names and nested finite values before atomically replacing root bindings; omitted bindings remain. Context clones copy binding maps and share immutable storage, so later replacements are independent. See the executed [Rust input example](interpreter-architecture.md#input-variables).

Installed values also pass [per-value admission](value-limits.md). JSON conversion still has its independent 128-container guard; an otherwise valid JSON document can exceed the tighter execution value-depth/node/payload budgets. Value resource failures use BW8001 and precede root installation or script effects. JSON preallocation/total-input bounds remain separate work.
