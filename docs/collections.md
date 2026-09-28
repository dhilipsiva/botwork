# Collection statements

The default Engine and CLI provide sixteen collection signatures alongside the
nine [basic built-ins](builtins.md). They run synchronously or asynchronously,
use ordinary statement calls and metadata, and need no external dependencies.
Names are case-insensitive with insignificant whitespace; variables and map keys
remain case-sensitive. `--statement-help 'Set In |collection| At |key| To |value|'`
shows the same registered signature used for validation.

All updates return owned replacement collections. Bind the result with ordinary
assignment to keep it. Discarding a result leaves the original unchanged, including
nested values. Dot and bracket reads remain immutable; indexed assignment is still
invalid syntax. There is no implicit coercion between arrays, maps, or strings.

| Signature | Inputs and result |
| --- | --- |
| `Create Array` | Empty Array |
| `Create Map` | Empty Map |
| `Create Map From \|entries\|` | Array of exactly `[String key, value]` pairs → Map; duplicate keys are invalid |
| `Repeat \|value\| Times \|count\|` | Any value, nonnegative Int → Array of independent copies; zero gives an empty array |
| `Get From \|collection\| At \|key\|` | Array/Map, existing index/key → selected value copy |
| `Set In \|collection\| At \|key\| To \|value\|` | Array/Map, index/key, any value → replacement of the same kind; array index must exist, map keys are inserted or replaced |
| `Append To \|array\| Value \|value\|` | Array, any value → Array with one new final element |
| `Remove From \|collection\| At \|key\|` | Array/Map, existing index/key → replacement of the same kind; array elements after the removed index shift left |
| `Collection Contains \|collection\| Item \|item\|` | Array and any value → deep value membership; Map and String → key membership; returns Bool |
| `Length Of \|collection\|` | Array/Map → top-level entry count as Int |
| `Map Keys \|map\|` | Map → Array of String keys, sorted ascending |
| `Map Values \|map\|` | Map → Array of values in ascending key order |
| `Map Entries \|map\|` | Map → Array of `[key, value]` pairs in ascending key order |
| `Enumerate \|array\|` | Array → Array of `[Int index, value]` pairs in original order |
| `Collections Equal \|left\| And \|right\|` | Two Array/Map values → Bool deep equality; different collection kinds are unequal |
| `Slice \|array\| From \|start\| To \|end\|` | Array, Int, Int → copied half-open range; require `0 <= start <= end <= length` |

Arrays use zero-based, nonnegative **Int** indexes. Numeric strings, Float indexes,
and negative indexes are invalid. Map keys must be exact Strings, including empty
strings and arbitrary Unicode. No Unicode normalization or locale collation occurs.
Map iteration sorts by Unicode scalar order (equivalently UTF-8 lexicographic order),
so Keys, Values, and Entries agree across runs. Use `For` over Entries or Enumerate
to retain key/index and value together. These helpers materialize arrays; they are
not lazy iterators.

Membership and comparison use the language's existing deep equality: array order
matters, map entry order does not, and mixed Int/Float comparison preserves the Int
exactly. For example, `16777217` differs from `16777216.0`. None is an ordinary value
obtained from a call such as `No Operation`. Map membership tests keys, even when a
value matches the requested item. An empty collection has length zero, no members,
and an empty iteration result.

Wrong declared parameter kinds, negative repetition counts, malformed entry pairs,
and duplicate keys in Create Map From produce **BW3003**. This explicit duplicate
rejection differs from map literals, whose later duplicate key replaces an earlier
value. Invalid lookup/update key kinds, absent required keys, and out-of-range or
reversed ranges produce **BW3004**. Length/index conversion beyond the Int range
produces **BW3002**. These operational failures carry the call site and call frames
and may be handled with Try/Catch/Finally. Failed assignment preserves its previous
destination. No comparison result or operational error is itself an assertion;
use Assert to require a result.

Arguments are evaluated once in source order, before the helper runs. In particular,
Repeat with zero count still evaluates its value argument, and an invalid update
index does not skip evaluation of its replacement argument. Parser validation still
rejects malformed syntax anywhere before effects begin.

Each constructed result is measured against value limits, then reserves temporary
storage while all evaluated arguments remain live, before any result payload copy
or result container allocation. Repetition uses checked multiplication to reject
huge counts without a count-sized scan or allocation. Updates account for the final
selected children, excluding removed/replaced values. Map-key-to-String conversion
checks string limits separately from input map key limits. Create Map From validates
pair shapes before copying, then checks duplicate keys during the admitted build;
invalid duplicates can therefore encounter a resource failure first. Normal stop
control is checked before invocation, after result admission, and during copy/search
loops. There are no external effects.

Ordinary argument evaluation copies a variable collection before the helper examines
it. `Get From` then copies the selected value. Existing dot/bracket access can retain
the root binding and copy just the selected leaf, making it preferable for repeated
reads of a large stored collection. Collection construction and copying are linear
in selected value size; sorted map iteration also costs O(n log n) comparisons and
O(n) borrowed sorting slots after output admission.

Initialization is idempotent and preserves host registrations made before it.
These signatures occupy default root names, so existing conflicting root declarations
need renaming or qualification; child scopes can shadow them normally. The complete
89-signature fixed catalogue, including sixteen [string statements](strings.md)
and sixteen [date/time statements](datetime.md),
is exempt from user registry retention budgets, while
its 89 table slots count toward snapshot admission. Execution, values, temporaries,
diagnostics, and result exports keep their normal limits.

<!-- botwork-test: collection-statements -->
```botwork
|items| = Create Array
|items| = Append To |items| Value |{name: "tea", quantity: 2}|
|replacement| = Set In |items| At |0| To |{name: "tea", quantity: 3}|
Assert |items.0.quantity| Equals |2|
Assert |replacement[0].quantity| Equals |3|
|stock| = Create Map From |[["tea", 3], ["coffee", 1]]|
|stock| = Set In |stock| At |"tea"| To |4|
For |entry| In |@{ Map Entries |stock| }| {
    Log |entry|
}
Assert |@{ Collection Contains |stock| Item |"tea"| }|
Assert |@{ Collections Equal |@{ Map Keys |stock| }| And |["coffee", "tea"]| }|
Log |@{ Slice |[10, 20, 30]| From |1| To |3| }|
Log |@{ Enumerate |["a", "b"]| }|
Log |@{ Length Of |stock| }|
```

Run the same script as `cargo run -- --file examples/29-collections.botwork`.
The example, reference block, focused tests, allocator observations, and B2
conformance cases execute in the test suite. [Verification evidence](collections-evidence.json)
records the tested source and platform profiles.
