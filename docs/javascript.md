# JavaScript statements

A script can call statements written in JavaScript, which Botwork runs with
Node.js ([D7](decisions.md#d7-javascript-hosting)). Each call runs in a Node
process of its own, supervised like an [isolated worker](isolated-workers.md):
a stop ends the process, and nothing in it outlives the call.

## What it needs

Node.js on `PATH` where Botwork runs; any build of Botwork can use it. An
import without Node fails with BW6001, naming what it needs. CI tests
JavaScript statements on Linux, macOS, and Windows with the runners' Node.

## Writing a module

A JavaScript module exports `statements`, an object that maps each header to
the function behind it:

```js
export const statements = {
  "Greet |name|": (name) => `Hello, ${name}`,
  "Total of |prices| with tax |rate|": (prices, rate) =>
    prices.reduce((sum, price) => sum + price, 0) * (1 + rate),
  "Fetch |url|": async (url) => (await fetch(url)).status,
};
```

A CommonJS module sets `module.exports.statements` instead. A script imports
the file, `.mjs`, `.js`, or `.cjs`, as it imports a Botwork module, by a path
relative to the importing file, and calls its statements through the namespace:

```
Import |"pricing.mjs"| As |pricing|
|greeting| = pricing::Greet |"Ada"|
|due| = pricing::Total of |[10, 20.5]| with tax |0.2|
```

- Botwork passes the parameters in order. A function may take fewer
  parameters than its header passes, but not more; its `length`, which counts
  the parameters before any default or rest one, must not exceed the header's.
- An `async` function's promise is awaited.
- Each call imports the module afresh in a new process, so module-level
  variables do not carry from one call to the next. Keep state in the script,
  or in files and services the statements use.
- What a module prints, through `console` or `process.stdout`, goes to Node's
  standard error, which Botwork discards: return values instead. Printing more
  than 1 MiB in one call fails it with BW8001.
- Namespaces follow the [module rules](extending.md#botwork-modules): a
  namespace already in use fails with BW6003, and an unknown statement in it
  with BW2002.

## Values

| Botwork | JavaScript | Back to Botwork |
| --- | --- | --- |
| None | `null` | `null` or `undefined` |
| Int, 32 bits | number | An integer number within 32 bits |
| Float, 32 bits | number | A fraction, rounded to the nearest 32-bit float; or a whole number beyond 32 bits that a 32-bit float holds exactly |
| Bool | boolean | boolean |
| String | string | A string without unpaired surrogates |
| Array | Array | Array |
| Map | Object with a `null` prototype | A plain object |

JavaScript has one number type, so a whole number comes back as an Int. A
whole number no Int or 32-bit float holds exactly, such as `2 ** 31 + 1`,
would change if rounded, so it fails the call. So does any other value
without an exact Botwork equivalent, such as a BigInt, a `Map`, a `Date`, a
class instance, a function, `NaN`, `Infinity`, or a string, or map key, with
an unpaired surrogate: each fails with BW4002 naming it, and nothing is
coerced. Values nest at most 64 levels.

## Errors

- An `AssertionError`, such as `node:assert` throws, is an assertion failure,
  BW9001, with its message.
- Any other exception is BW4002, `JavaScript RangeError: bad input` for
  example, followed by the end of its stack, up to 2 KiB.
- A script catches both with `Try`/`Catch`, as any statement failure.
- A module that cannot load fails its import with BW6001, with Node's error:
  a syntax error, an exception at its top level, no `statements` object, a
  statement that is not a function, or a function taking more parameters than
  its header passes.

## Calls, stops, and checks

Calls run through the [worker protocol](worker-protocol.md) in a pool of up
to 4 Node processes per run, each with the [default worker limits](isolated-workers.md):
30 seconds and 1 MiB of output per call. Each process receives the run's
environment, and starts in the module's directory. A stop, whether
cancellation, a deadline, or an interrupt, ends the process and everything it
started, at once. Starting Node takes tens of milliseconds, so a statement
called in a tight loop is better written to take a batch.

Node's first start after its files leave the disk cache is much slower: on
hosted Windows CI runners it has taken from seconds to over a minute, longer
than the 30 seconds an import waits for a module's headers. Start Node once
before such runs, as Botwork's CI does with a small module script; there,
`node --version`, which runs no JavaScript, did not help.

`--check` cannot see a JavaScript module's statements, so it reports calls
into one as not checked rather than as errors.

## Tests

`cargo test --locked --test javascript_adapter` covers values in both
directions and the ones refused, exceptions and stacks, calls in processes of
their own, printing, `async` functions and CommonJS modules, a stop that ends
a busy call, loading failures, and the CLI with and without Node. Without Node the
tests are skipped unless `BOTWORK_REQUIRE_NODE` is set, as it is in CI.
