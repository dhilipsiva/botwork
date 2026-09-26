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

Input failure cannot be caught by the script because execution has not begun. Resource budgets below apply before publication; secret-marked redaction and broader interpreter limits remain separate TODOs.

## Rust Hosts

`core::input::{parse_variables, parse_variable, load_variables}` return owned values with detailed errors. `Context::set_input_variables` validates all names and nested finite values before atomically replacing root bindings; omitted bindings remain. Context clones copy binding maps and share immutable storage, so later replacements are independent. See the executed [Rust input example](interpreter-architecture.md#input-variables).

## Input Resource Budgets

Default parsing/loading uses `InputLimits`; the `_with_limits` variants of all three Rust functions accept local configuration. Each parse call has a fresh budget. `load_variables_with_limits` shares one budget across files and subsequent flags.

| Field | Default | Count |
| --- | --- | --- |
| `source_bytes` | 8 MiB | Each JSON file/document or complete NAME=JSON setting |
| `total_bytes` | 32 MiB | Cumulative source bytes across the ordered load |
| `sources` | 128 | Files/settings visited, including repeated or overridden sources |
| `raw_nodes` | 262,144 | Every JSON value/container and object-key token, including discarded duplicates; flag names are outside JSON |
| `variables` | 16,384 | Distinct retained root names in each source and the merged result |
| `values` | ValueLimits defaults | Decoded value nodes/depth/strings/keys/entries/payload; root names also obey `key_bytes` |

Read at most the smaller of the remaining total and per-source budget plus one detection byte; reject oversize before UTF-8 decoding. An iterative raw scan bounds nesting/tokens, array width/minimum shape, and decoded string/key lengths before serde allocates owned values or escape buffers. Count Unicode escapes and surrogate pairs by resulting UTF-8 bytes. Bounded raw visitors preserve last-key-wins semantics while limiting distinct map/root names. Convert winning raw values with checked child metrics before parent insertion, preserving exact integer/direct-f32 conversion.

The independent 128-container JSON guard still includes the root object and discarded values. Converted values now obey the 64-level default value ceiling during parsing, rather than waiting for Context installation. A discarded duplicate can exceed converted-value depth if it remains within raw guards; oversized encoded strings/arrays/tokens still fail raw admission even when discarded. Every file/flag is admitted before a later source can override it. Whole-value replacement and file-then-flag order remain unchanged.

Resource failures return BW8001 before script effects. Their span identifies the input origin at its start (line 1, column 1) and retains no payload; it does not claim the failing token's location. Ordinary syntax/conversion errors keep BW7001 and collection paths; long key/path previews use an ellipsis to bound diagnostic construction. Unsupported value-depth configuration returns BW7002. Resource preflight may take priority over later syntax/conversion errors.

Use the same ValueLimits in InputLimits and RunLimits when raising decoded-value budgets for execution. Empty file/flag lists need no source allowance; an empty JSON object still consumes its source, bytes, and root token. Default standalone APIs intentionally reject decoded values above execution defaults.

Tests cover exact/zero/cumulative/local budgets, Unicode/UTF-16 boundaries, malformed escapes, raw versus converted nesting, duplicate/override behavior, file cuts, origin retention, path abbreviation, and CLI rejection before output/traces. Allocator observations verify raw string/array rejection before payload-sized allocation. R8 corpus cases and an executed Rust example pin counts. Host allocations, aggregate runtime state, callback behavior, output/diagnostic serialization, and filesystem hard deadlines retain separate resource contracts.
