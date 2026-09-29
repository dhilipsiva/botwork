# Structured data: JSON and CSV

Three built-in statements convert test data, and suites can read JSON or CSV
dataset files:

| Statement | Result |
| --- | --- |
| `Parse JSON \|text\|` | The value of one JSON document |
| `Format JSON \|value\|` | Compact canonical JSON text as a String |
| `Parse CSV \|text\|` | An Array of Maps from header names to String fields |

All three run inline in both execution modes. Invalid input fails with a
catchable BW3003 whose message names the problem and its location. A result
larger than the value, temporary, or output limits stops the run with BW8001,
which Catch cannot handle.

## JSON conversion

`Parse JSON` follows the [input-variable rules](input-variables.md#values-and-names)
exactly, so a document read from a file, a flag, or an HTTP response produces the
same value:

| JSON | Botwork value |
| --- | --- |
| `null`, `true`, `false` | None, Bool |
| Integer token such as `-7` | Int; tokens outside -2147483648..2147483647 fail |
| Decimal or exponent token such as `1.0` or `1e3` | Float rounded directly to f32; overflow fails, underflow rounds to zero, `-0.0` keeps its sign |
| String | String; JSON escapes and surrogate pairs decode to Unicode |
| Array, object | Array, Map, converted recursively |

- A later duplicate object key replaces the earlier value.
- One leading byte order mark (U+FEFF) is ignored. Any other text outside the
  document, including trailing values, is an error.
- Documents may nest at most 128 containers, and the result must also fit the
  value depth limit (64 by default).

Errors include a JSON path and serde's line and column, for example
`Parse JSON: $["a"]["b"]: integer is outside -2147483648..2147483647` or
`Parse JSON: $: expected value at line 1 column 10`.

Each converted node is charged to the run's temporary-value budget *before* it is
allocated. A document whose decoded strings or nodes exceed the budget is
rejected without building the rest of the value.

## Canonical JSON

`Format JSON` writes compact JSON with no whitespace:

- Map keys are sorted by Unicode scalar value.
- A Float always contains a decimal point or exponent (`1.0`, `-0.0`,
  `3.4028235e38`), using the shortest text that restores the same f32. An Int
  never contains one.
- None becomes `null`.
- Strings escape `"` and `\`, use `\b \f \n \r \t` for those controls and `\u00XX`
  for other controls below U+0020, and keep all other characters unescaped.

Every value survives a round trip: `Parse JSON |@{ Format JSON |value| }|` equals
`value`, with the same Int or Float kind at every position. Formatting the same
value always produces the same text. The format is canonical for Botwork values,
but it is not RFC 8785 JCS, whose number syntax and key order differ.

## CSV tables

`Parse CSV` reads comma-separated records with a header row:

- The first record names the columns. Names must be nonempty and distinct.
- Records end with LF or CRLF. A final line ending does not add an empty record.
  Header-only input gives an empty Array, but empty input is an error.
- Fields may be quoted. Quoted fields can contain commas, line breaks, and
  doubled quotes (`""` for one `"`), as in RFC 4180. A quote inside an unquoted
  field, text after a closing quote, or a carriage return without a line feed is
  an error.
- Every record must have exactly as many fields as the header. A blank line is
  one empty field, so it is valid only in single-column tables.
- Fields are always Strings. Convert them explicitly: `Parse JSON |row.qty|`
  turns `"10"` into Int 10.
- One leading byte order mark is ignored.

Errors name the line, and for malformed fields the Unicode-scalar column:
`Parse CSV: line 2, column 4: a quoted field must end at a comma or line end`,
or `Parse CSV: line 3: record 2 has 1 field; the header has 2`.

## Missing fields

Parsed data uses ordinary collection access. A missing key or index fails with
BW3004, and Catch sees the full `details.path` and the failing `details.segment`,
for example `zip` in `doc.users[1].address.zip`. Test for optional fields
explicitly with `Collection Contains |map| Item |"key"|` before reading them.

## Runnable example

<!-- botwork-test: structured-data -->
```botwork
|order| = Parse JSON |"{\"id\": 7, \"total\": 12.5, \"items\": [{\"sku\": \"A-1\", \"qty\": 2}], \"note\": null}"|
Log |order.items[0].sku|
Log |@{ Type Of |order.total| }|
Log |@{ Format JSON |order| }|
|rows| = Parse CSV |"sku,qty\nA-1,2\n\"B,2\",10\n"|
Log |rows[1].sku|
Log |@{ Parse JSON |rows[1].qty| } + 1|
Try { |bad| = Parse JSON |"[1,]"| } Catch |error| { Log |error.message| }
```

Output:

```text
A-1
Float
{"id":7,"items":[{"qty":2,"sku":"A-1"}],"note":null,"total":12.5}
B,2
11
Operation performed on incompatible types: Parse JSON: $: expected value at line 1 column 4
```

## JSON and CSV dataset files

Name the format after `From` to read a [dataset](parameterized-cases.md) file as
JSON or CSV. The format keywords are case-insensitive:

```text
Dataset |"users"| From JSON |"datasets/users.json"|
Dataset |"prices"| From CSV |"datasets/prices.csv"|
```

Without a format, `From` reads a Botwork `Dataset` document as before. A
filename suffix never chooses the format.

- **JSON file:** an array of objects. Each object needs a String `id` field, which
  becomes the row ID. The whole object, including `id`, is the row value. Values
  follow the JSON conversion rules above, so Ints and Floats keep their kinds.
- **CSV file:** a header row that includes an `id` column, then one row per
  record. The row value maps every header, including `id`, to its String field.

Row IDs follow the usual rules: at most 128 bytes, starting with an ASCII letter
or digit, and containing only letters, digits, `.`, `_`, or `-`. IDs must be
distinct. Each row's name is its ID. Rows carry no tags of their own, so they
inherit only the suite and case tags. File limits match Botwork dataset files: 1 MiB per
file, 1,024 rows, and the default value limits per row.

Invalid files fail during discovery with BW7002 (or BW8001 for a limit), before
any case runs. Errors point into the data file, for example
`users.json:3:3-3:25` for a row object without an `id`. One leading byte order
mark is ignored, as spreadsheet exports often add one.

A file declared in two formats is parsed once per format. Otherwise the usual
discovery rules apply: canonical-path caching, UTF-8 regular files only, and
reading unused datasets too.

## Dataset-driven HTTP checks

[Example 39](../examples/39-dataset-http.suite.botwork) reads its expectations
from a [JSON dataset](../examples/datasets/users.json). For each row, it fetches
`{base_url}/{id}.json`, parses the response body, and compares the name, roles,
and nested city. To run it, start the fixture server in another terminal
(Python 3 required):

```sh
python3 -m http.server 8765 --bind 127.0.0.1 --directory examples/fixtures/users
```

Then run:

```sh
cargo run -- --suite examples/39-dataset-http.suite.botwork --jobs 2 \
  --var 'base_url="http://127.0.0.1:8765"'
```

Both rows pass. If you change a row's expected `city`, only that row fails. The
failure names the case, dataset, and row, and shows the first difference, for
example `difference at $: expected "Osaka" (String), got "東京" (String)`.

## Verification

Integration tests cover:

- conversion boundaries, rejection messages, and value and temporary limits;
- canonical output and seeded round trips;
- RFC 4180 tables and CSV errors, including CRLF, quoted line breaks, and BOMs;
- missing fields in nested payloads;
- JSON and CSV dataset files and their errors, and per-format caching;
- the dataset-driven HTTP example against a loopback fixture.

Unit tests prove that the per-node admission totals equal each value's accounted
size, and that CSV positions are exact. An allocation test shows that a decoded
string is rejected before it is copied. B9 conformance cases and the executed
example above cover the language contract. Run them with
`cargo test --locked --test structured_data`.
[Validation evidence](structured-data-evidence.json) records the measured
profiles and mutations.
