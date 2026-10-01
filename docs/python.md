# Python statements

A script can call statements written in Python, in a Botwork build with the
`python` feature ([D9](decisions.md#d9-python-packaging)). Python code runs
in-process and is trusted, like a native Rust statement: it is not a sandbox.

## Building with Python

```sh
cargo install --path . --locked --profile dist --features python
```

The build links the libpython of the `python3` (or, on Windows, `python`) it
finds, which must be 3.10 or later and present where Botwork runs:

| Platform | Building | Running |
| --- | --- | --- |
| Linux | The distribution's Python with its development files, `python3-dev` on Debian and Ubuntu | The same libpython |
| macOS | A Homebrew or python.org Python | The same Python |
| Windows | A python.org Python | That Python's directory on `PATH`, so Windows finds its DLLs |

CI builds and tests the feature on all three, against Python 3.12. The release
binaries leave Python out, so they stay single files. A build without the
feature refuses a Python import with BW6001, naming the feature.

## Writing a module

A Python file declares statements by marking functions with
`botwork.statement`, which takes the statement's header:

```python
import botwork

@botwork.statement("Greet |name|")
def greet(name):
    return f"Hello, {name}"

@botwork.statement("Total of |prices| with tax |rate|")
def total(prices, rate):
    return round(sum(prices) * (1 + rate), 2)
```

A script imports the file as it imports a Botwork module, by a path relative
to the importing file, and calls its statements through the namespace:

```
Import |"pricing.py"| As |pricing|
|greeting| = pricing::Greet |"Ada"|
|due| = pricing::Total of |[10, 20.5]| with tax |0.2|
```

- Botwork passes the parameters in order. When the file loads, each function
  must accept as many positional arguments as its header has parameters.
- An `async def` statement runs its coroutine to completion.
- While the file loads, its directory comes first on `sys.path`, so it can
  import modules beside it. Import them at the top of the file.
- Namespaces follow the [module rules](extending.md#botwork-modules): a
  namespace already in use fails with BW6003, and an unknown statement in it
  with BW2002.
- `botwork.__version__` is the version of the Botwork running the module, such
  as `"0.2.0"`, so a module can check it supports that Botwork.

## Values

| Botwork | Python | Back to Botwork |
| --- | --- | --- |
| None | `None` | `None` |
| Int, 32 bits | `int` | An `int` within 32 bits |
| Float, 32 bits | `float` | A `float` a finite 32-bit float holds, rounded to the nearest one |
| Bool | `bool` | `bool` |
| String | `str` | `str` |
| Array | `list` | `list` or `tuple` |
| Map | `dict` with `str` keys | `dict` with `str` keys |

Any other value, such as `bytes`, a `set`, an object, an `int` beyond 32 bits,
or `nan`, fails the call with BW4002 naming it; nothing is coerced. A returned
value must also fit the [value limits](interpreter-architecture.md).

## Errors

- An `AssertionError` is an assertion failure, BW9001, with its message.
- Any other exception is BW4002, `Python ValueError: bad input` for example,
  followed by the end of its traceback, up to 2 KiB.
- A script catches both with `Try`/`Catch`, as any statement failure.
- A file that cannot load fails its import with BW6001: a syntax error, an
  exception while it runs, a header that is not a `str`, or a function that
  cannot take its header's arguments.

## Runs, threads, and stops

CPython runs one interpreter per process. Each run loads its own module
objects, so module-level variables stay within the run, and two imports of a
file in one run share its module. The interpreter, and the packages a file
imports into `sys.modules`, serve every run in the process.

Loading a file, which starts the interpreter on first use and runs the file's
top level, is blocking work too: it runs on a worker thread, and a stop
interrupts it as it interrupts a call.

Python statements run as blocking native operations, on worker threads, with
at most 4 calls of each in flight; the interpreter lock runs one Python thread
at a time. They need asynchronous execution: the CLI, or `Engine`'s `_async`
entry points. A synchronous entry point refuses them with BW5003.

A stop, whether cancellation, a deadline, or an interrupt, raises
`botwork.Stopped` in the thread running the call. `Stopped` derives from
`BaseException`, so `except Exception` does not catch it. Python code running
bytecode stops at once. A call blocked in native code, such as `time.sleep`
or a socket read, sees the stop when it returns; until then it has the
[stop grace](shutdown.md), after which it is abandoned like any blocking
native statement. Botwork installs no Python signal handlers: it handles
interrupts itself.

## Tests

`cargo test --locked --features python --test python_adapter` covers values in
both directions and the ones refused, exceptions and tracebacks, module state
within and across runs, `async def` statements, neighbouring imports, loading
failures, stops that end a busy call and a file looping as it loads, and the
CLI. A build without the
feature checks its refusal in `tests/local_imports.rs`.
