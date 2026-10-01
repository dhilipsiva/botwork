# WebAssembly statements

A script can call statements in a WebAssembly component, which Botwork runs
in-process with Wasmtime ([D8](decisions.md#d8-wasm-runtime)). Each call runs
in an instance of its own, in a sandbox: it gets clocks and randomness, and
no files, network, or environment. Fuel and a memory cap bound it, and a stop
interrupts it at once.

## What it needs

Nothing beyond the binary. The `wasm` build feature, on by default, compiles
Wasmtime into Botwork; see [what the binary needs](distribution.md#what-the-binary-needs).
Wasmtime and its Cranelift compiler are most of the binary: the Linux x86_64
release build is about 27 MB, against 11 MB without them
([D4](decisions.md#d4-budgets-and-baseline)).
A Rust program that embeds Botwork can leave it out with
`default-features = false`. In such a build, an import of a `.wasm` file fails
with BW6001 naming the feature.

## Writing a module

A module is a component of the `module` world in
[`wit/botwork.wit`](../wit/botwork.wit). It exports the `statements`
interface:

```wit
interface statements {
    use types.{value, failure};

    /// The statements' headers, such as "Greet |name|", in order.
    headers: func() -> list<string>;

    /// Run the statement at `index` in `headers`, with one value per parameter.
    call: func(index: u32, arguments: list<value>) -> result<value, failure>;
}
```

In Rust, a `cdylib` crate built for the `wasm32-wasip2` target with
[wit-bindgen](https://crates.io/crates/wit-bindgen) is such a component.
[`examples/wasm/greeter`](../examples/wasm/greeter/src/lib.rs) provides one
statement, `Greet |name|`: `headers` returns its header, and `call` matches
the statement's index and its argument's nodes. Build it with
`cargo build --release --target wasm32-wasip2` in that directory, after
`rustup target add wasm32-wasip2`.

A script imports the component as it imports a Botwork module, by a path
relative to the importing file, and calls its statements through the
namespace:

```
Import |"greeter.wasm"| As |greeter|
|greeting| = greeter::Greet |"Ada"|
Log |greeting|
```

With the component copied next to it as `greeter.wasm`, this script, the
example's [`greet.botwork`](../examples/wasm/greeter/greet.botwork), logs
`Hello, Ada`.

- Botwork passes one value for each parameter of the header, in order.
- Each call instantiates the module afresh, so its memory and globals do not
  carry from one call to the next.
- Namespaces follow the [module rules](extending.md#botwork-modules): a
  namespace already in use fails with BW6003, and an unknown statement in it
  with BW2002.

The component the tests call, built from [`tests/wasm/guest`](../tests/wasm/guest/src/lib.rs),
is a larger example: it converts values both ways and uses the capabilities
below.

## Values

WIT types cannot contain themselves, so a value is a list of nodes. The first
node is the value; arrays and maps name their items by index in the list.
Each index comes after the node that names it, and every node but the first
is named exactly once, so a value is always a tree.

| Botwork | Node |
| --- | --- |
| None | `none` |
| Int, 32 bits | `int(s32)` |
| Float, 32 bits | `float(f32)`, finite |
| Bool | `boolean(bool)` |
| String | `text(string)` |
| Array | `array(list<u32>)`, the items' indexes |
| Map | `map(list<tuple<string, u32>>)`, each key once |

Botwork lays values out depth first, with map entries in key order. A module
may return any layout that is a tree. A result that is not a tree, holds a
float that is not finite or a key twice, or exceeds the run's
[value limits](value-limits.md) fails the call with BW4002 saying why;
nothing is coerced.

## Errors

- A `failure` of kind `assertion` is an assertion failure, BW9001, with its
  message.
- A `failure` of kind `error` is BW4002 with its message.
- A trap, such as a Rust panic, is BW4002: `WebAssembly trap:` and the trap,
  then the end of what the module wrote to stderr, where Rust writes a panic's
  message, and the start of its backtrace, each up to 2 KiB.
- A script catches all of these with `Try`/`Catch`, as any statement failure.
- A module that cannot load fails its import with BW6001: a file that is not a
  component, a component that does not export `statements` or that imports
  more than WASI Preview 2, a trap while listing its headers, or a header that
  is not valid.

## Capabilities

Each instance receives WASI Preview 2 with:

- the wall and monotonic clocks, and random numbers;
- no preopened directories, so no files;
- no network: every socket is refused;
- no environment variables or arguments;
- stdin closed, and stdout and stderr captured: stdout is discarded, and
  stderr appears in a trap's diagnostic.

## Limits and stops

`RunLimits::wasm`, a `WasmLimits`, bounds each import and call. The CLI uses
the defaults.

| Limit | Default | Bounds |
| --- | --- | --- |
| `module_bytes` | 32 MiB | The module file an import reads |
| `fuel` | 10,000,000,000 | What a call runs, about one unit per WebAssembly instruction |
| `memory_bytes` | 256 MiB | The linear memory one call's instance may grow to, at most 4 GiB |
| `output_bytes` | 1 MiB | What a call writes to stdout, and to stderr |

A call that reaches a limit stops with BW8001, a resource limit, naming it:
`WASM fuel`, `WASM memory bytes`, `WASM table elements` (over 1,048,576),
`WASM stdout bytes`, or `WASM stderr bytes`.

A stop, whether cancellation, a deadline, or an interrupt, interrupts a
running call at its next loop iteration or function entry through Wasmtime's
epoch interruption. Even a call in an endless loop ends at once, and nothing
is abandoned. Compiling a module at import cannot be interrupted; like other
[blocking work](shutdown.md), it has the stop grace to finish.

An import compiles the module with Cranelift on a blocking thread, once per
module in a run; a small module compiles in a fraction of a second. Calls run
on blocking threads, up to 4 at once for each statement.

## Checks

`--check` cannot see a WebAssembly module's statements, so it reports calls
into one as not checked rather than as errors.

## Tests

`cargo test --locked --test wasm_adapter` covers values in both directions
and the results refused, failures and traps, instances of their own, the
capabilities denied, the fuel, memory, and output limits, a stop that ends a
busy call, loading failures, limit validation, and the CLI.

The tests call [`tests/wasm/statements.wasm`](../tests/wasm/statements.wasm),
built from `tests/wasm/guest`. `python3 scripts/wasm_guest.py check` verifies
that it matches its recorded sources; after changing them, rebuild it with
`python3 scripts/wasm_guest.py build`. CI also rebuilds it from source, runs
the tests against that build, and builds and runs the greeter example;
`python3 tests/wasm_tools.py` tests the script.
