# String statements

The default Engine and CLI provide sixteen string signatures as part of the
100-statement fixed catalogue. All transformations return independent values for
ordinary assignment. They preserve their inputs and require the declared kinds;
there is no automatic String conversion in joining, splitting, or matching.
Format String explicitly renders arbitrary supplied values as described below.
Names follow ordinary case-insensitive statement matching.

| Signature | Inputs and result |
| --- | --- |
| `Format String \|template\| With \|values\|` | String, Array/Map → String with positional or named substitutions |
| `Join Strings \|strings\| With \|separator\|` | Array containing only Strings, String → joined String; empty Array gives `""` |
| `Split String \|text\| On \|separator\|` | String, nonempty String → Array of Strings, preserving empty fields |
| `Split Lines \|text\|` | String → Array of lines without LF/CRLF terminators |
| `Replace String \|text\| Find \|needle\| With \|replacement\|` | Three Strings; nonempty needle → String replacing all non-overlapping literal occurrences |
| `String Contains \|text\| Text \|needle\|` | Two Strings → Bool for exact literal substring membership |
| `String Starts With \|text\| Prefix \|prefix\|` | Two Strings → Bool for an exact literal prefix |
| `String Ends With \|text\| Suffix \|suffix\|` | Two Strings → Bool for an exact literal suffix |
| `String Length \|text\|` | String → Int count of Unicode scalar values |
| `Slice String \|text\| From \|start\| To \|end\|` | String, Int, Int → half-open Unicode scalar slice; require `0 <= start <= end <= length` |
| `Trim String \|text\|` | String → String without leading/trailing Unicode whitespace |
| `Uppercase String \|text\|` | String → Unicode default uppercase String |
| `Lowercase String \|text\|` | String → Unicode default lowercase String |
| `String Matches \|text\| Regex \|pattern\|` | Two Strings → Bool for a regex search anywhere in the text |
| `Find Matches In \|text\| Regex \|pattern\|` | Two Strings → Array of full, non-overlapping regex matches in source order |
| `Capture From \|text\| Regex \|pattern\|` | Two Strings → Array containing the first full match followed by numbered capture groups; no match gives `[]` |

`--statement-help 'Format String |template| With |values|'` displays the registered
parameter, return, and error metadata. The fixed names occupy root slots; existing
conflicting declarations need renaming or qualification. Host registrations made
before initialization remain intact, repeated initialization is idempotent, and
child scopes may shadow the names. All 100 table slots count toward snapshot work;
fixed metadata remains exempt from user registry retention admission.

## Formatting, splitting, and replacement

For an Array, `{0}`, `{1}`, and so on select zero-based positions. Indexes contain
ASCII digits only; leading zeros are allowed. Automatic `{}` indexing, negative
indexes, format specifiers, property access, and expressions are unsupported.
For a Map, the field is an exact key: `{name}`, `{தமிழ்}`, and `{a.b}` select those
literal keys. `{}` selects an empty map key. Whitespace is part of a map key.
Braces cannot occur inside field names. `{{` and `}}` insert literal braces.
Unused values are permitted; missing fields and unmatched/nested braces fail.

Substitution uses readable Botwork display: a String inserts its raw text, None
inserts `none`, scalars use their ordinary spelling, nested Strings are quoted,
and maps sort their keys. This is not a JSON serializer; use [Format JSON](structured-data.md#canonical-json). Inserted content is never
parsed again as a template or executed. To surround a substituted value with
braces, use `{{{0}}}`. Formatting always receives its data explicitly and never
reads variable names from the template.

Joining permits an empty separator. Literal splitting requires a nonempty
separator and preserves leading, consecutive, and trailing empty fields. Splitting
an empty string gives `[""]`. Literal replacement requires a nonempty needle;
an empty replacement deletes matches. Replacement text has no `$1`, backslash,
or template expansion. Matches are chosen left to right without overlap, so
replacing `aa` in `aaaaa` leaves the last `a` after two replacements.

Split Lines recognizes LF and CRLF. A bare CR remains content. Empty input gives
`[]`; a final terminator does not create an additional line, while consecutive
terminators do create empty lines. An empty needle/prefix/suffix matches in the
three literal Bool operations, including against empty text.

## Unicode and regex behavior

Text remains UTF-8 without normalization. Literal matching is case-sensitive and
exact; precomposed `é` differs from `e` plus a combining acute accent. String Length
and Slice String count Unicode scalar values, including combining marks, variation
selectors, and joiners separately. They do not count bytes or user-perceived
grapheme clusters. A slice may therefore contain only a combining mark or one part
of a displayed emoji, while always remaining valid UTF-8.

Case conversion uses the Rust toolchain's Unicode default mappings. It may expand
one scalar into several: uppercase `ß` becomes `SS`, and lowercase `İ` becomes
`i` plus a combining dot. Lowercasing includes contextual Greek final sigma.
No locale tailoring, normalization, or full case folding is performed. Trim uses
Unicode whitespace and preserves internal spacing. Unicode tables follow the
compiled Rust toolchain. [Rust String methods](https://doc.rust-lang.org/std/primitive.str.html#method.to_lowercase)
describe these mappings.

Regex operations use the locked `regex` crate, currently 1.13.1, with its Unicode
tables. Patterns are case-sensitive by default; inline flags such as `(?i)` and
`(?m)` are supported. Search is unanchored; use `\A` and `\z` for the whole string.
Backreferences and look-around are unsupported. Match boundaries remain valid
UTF-8 boundaries, and an empty pattern matches at scalar boundaries. An empty
match directly adjacent to a preceding nonempty match is suppressed by the regex
iterator. Captures are ordered by group number, including named groups in their
numbered position; an unmatched optional group is None, distinct from an empty
matched String. See the [regex syntax reference](https://docs.rs/regex/1.13.1/regex/#syntax).

DSL string escaping is unchanged: only `\"`, `\\`, and `\n` are escapes. Write a
regex backslash twice inside a quoted DSL string, for example `"\\d+"` or
`"\\A[0-9]+\\z"`. CR, tabs, and other controls can arrive through host/JSON inputs
or literal source characters; `\r`, `\t`, and `\u` are not DSL escapes.

## Errors, ownership, and execution limits

Invalid declared kinds, non-String join elements, malformed/missing format fields,
empty split separators/replacement needles, invalid scalar ranges, and regex
syntax/nesting errors produce **BW3003**. Lengths outside Int range produce
**BW3002**. Resource admission failures produce **BW8001** and retain normal stop
behavior. Ordinary failures carry the call site and frames and can be handled by
Try/Catch/Finally. Failed assignments preserve their old destination. All arguments
are evaluated and kind-checked in source order; a later replacement argument still
runs before a helper can reject an empty needle.

Output byte/node/depth/entry size and live temporary overlap are measured before
result payload copying or result container allocation. Formatting measures borrowed
values without sorting map entries, then renders sorted maps after admission.
Join, replacement, split, and regex extraction use measured passes. Case conversion
measures the UTF-8 size before invoking the standard conversion. Input arguments
remain live while the result is built; output quotas do not imply in-place reuse.
These are semantic value budgets, not exact allocator capacities.

Regex compilation and search also have fixed per-call limits:

| Resource | Limit |
| --- | ---: |
| Pattern UTF-8 bytes | 16,384 |
| Approximate compiled regex bytes | 2 MiB |
| Approximate lazy-DFA cache capacity | 2 MiB |
| Parser nesting | 64 |
| Search accounting | 64 MiB of charged suffix bytes |

The pattern byte limit applies before compilation. Compiled-size exhaustion is
BW8001; nesting is reported with other syntax errors as BW3003. Matches/Captures
charge `text bytes + 1`. Find Matches charges four times `remaining suffix bytes + 1`
before each iterator query during planning, including the final unsuccessful query.
The suffix starts at the previous match's end. This conservatively accounts for two
passes and the iterator's empty-match retry, and can reject many small matches even
when they fit result quotas. It is deterministic accounting, not a CPU timer.

Regex scratch storage is bounded separately from Literal temporary budgets, and
regexes are not retained in a global cache. Search cost also depends on compiled
pattern size; ordinary iteration can require repeated suffix scans. The
[regex limits documentation](https://docs.rs/regex/1.13.1/regex/struct.RegexBuilder.html)
explains the compiler/cache bounds. Stop checks run around compilation/searches,
between iterator queries, and during output copying. They cannot preempt a single
library compile, search, or case conversion. Async regex calls use the existing
bounded worker pool and its cancellation/ownership contract; synchronous calls run
on the caller thread. Each async regex call also charges a worker call-frame
snapshot. Literal operations and formatting remain in the evaluator.

## Runnable example

<!-- botwork-test: string-statements -->
```botwork
|input| = |" tea ,coffee,,water "|
|parts| = Split String |input| On |","|
|cleaned| = Create Array
For |part| In |parts| {
    |cleaned| = Append To |cleaned| Value |@{ Trim String |part| }|
}
|joined| = Join Strings |cleaned| With |" / "|
Log |@{ Format String |"Items: {items}"| With |{items: joined}| }|
Assert |input| Equals |" tea ,coffee,,water "|
Log |@{ Replace String |joined| Find |" / "| With |", "| }|
Log |@{ Uppercase String |"Straße"| }|
Log |@{ Slice String |"a🙂éz"| From |1| To |4| }|
|text| = |"order=42; retry=3"|
Assert |@{ String Matches |text| Regex |"order=[0-9]+"| }|
Log |@{ Find Matches In |text| Regex |"[0-9]+"| }|
Log |@{ Capture From |text| Regex |"order=([0-9]+)"| }|
```

Run the same script with `cargo run -- --file examples/30-strings.botwork`.
The reference, example, B3 corpus, semantic/quota tests, and allocator observations
are executable. [Verification evidence](strings-evidence.json) records the tested
source, profiles, and focused mutation outcomes.
