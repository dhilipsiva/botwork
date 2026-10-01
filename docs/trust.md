# Extension trust

Botwork runs code from several places: scripts and their modules, statements
in Rust, Python, and JavaScript, WebAssembly components, and packages that
bring any of these. Only WebAssembly runs in a sandbox. Everything else runs
with the run's full authority, so it must be code you trust.

| Extension | Where it runs | What it can reach | Bounded by |
| --- | --- | --- | --- |
| [Botwork modules](extending.md#botwork-modules), local, [packaged](packages.md), or [by URL](packages.md#files-by-url) | In the run | Every statement the run offers: files, processes, the network, the environment | The run's [limits](configuration.md) and stops |
| [Rust statements](extending.md#rust-statements) | In the process | Everything the process can | Their own code; stops reach them at checkpoints |
| [Python statements](python.md) | In the process, on a worker thread | Everything the process can | Stops, which raise `botwork.Stopped` in Python code |
| [JavaScript statements](javascript.md) | A Node process per call | Everything the user can: Node has no sandbox here | The [worker limits](isolated-workers.md): 30 seconds and 1 MiB of output per call; a stop ends the process |
| [WebAssembly statements](wasm.md) | In the process, in a Wasmtime instance per call | The clocks and random numbers only: no files, network, environment, or arguments | Fuel, a memory cap, bounded output, and epoch interruption |
| [WebDriver drivers](webdriver.md), such as chromedriver | A process `Open Browser` starts, or a server it connects to | Everything the user can, and the browser it starts | Each command's `timeout_ms`; the run's end closes its sessions and ends the drivers it started |

## The policy

- **Trusted extensions** (Botwork, Rust, Python, and JavaScript code) act with
  the authority of the user who runs Botwork. Botwork bounds their time and
  output, but not what they touch. Review them as you review the scripts that
  use them.
- **WebAssembly components** are capability-restricted. They receive only the
  WASI Preview 2 clocks and randomness; the host refuses files, sockets, and
  the environment. Fuel and memory caps bound each call, and a stop
  interrupts it. A component cannot reach the run's variables, statements, or
  files, except through the values its statements are passed and return.
- **Packages and URL files** inherit the trust of what they contain: a
  package's Botwork, Python, and JavaScript modules are trusted code, and its
  WebAssembly modules are sandboxed. The lockfile's hashes pin exactly which
  files run, so a reviewed version stays the version that runs, but a hash does
  not vouch for the code.

To run code you do not trust, ship it as a WebAssembly component. To use a
package, read its modules, or take only its components.

## Tests

- `a_package_runs_its_botwork_code_trusted_and_its_webassembly_sandboxed`
  (`tests/packages.rs`) imports one package's Botwork module and WebAssembly
  component: the module reads a file that the component is refused.
- `modules_get_clocks_but_no_files_network_or_environment`
  (`tests/wasm_adapter.rs`) holds the sandbox to its grants: a component
  reads the clock, but no file, listening socket, or environment variable.
- `calls_see_the_runs_environment` (`tests/javascript_adapter.rs`) shows a
  trusted extension reaching the run's environment.
- The [package tests](packages.md#tests) cover fetching packages from clean
  through cached offline runs, integrity failures, conflicts, version
  incompatibilities, and namespace collisions.
