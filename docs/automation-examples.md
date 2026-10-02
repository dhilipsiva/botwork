# Complete automation examples

These tasks combine input variables, imported custom statements, assertions, and
standard statements. Run commands from the repository root. The scripts and
helper modules linked below are the complete executable sources; the CLI tests
run those same files with their bundled JSON configuration.

## Build a verified catalogue

[Example 35](../examples/35-build-catalogue.botwork) turns a text list into a
sorted, deduplicated catalogue. This can prepare an approved product list or a
deployment inventory. It requires Linux and `/usr/bin/sort` (GNU coreutils).

```sh
cargo run -- --file examples/35-build-catalogue.botwork \
  --vars-file examples/inputs/catalogue.json
```

The supplied [input](../examples/inputs/catalogue.txt) has duplicates, surrounding
whitespace, and a blank line. Successful execution prints `Published 3 entries`
and creates `catalogue.txt` in the current directory with these contents:

```text
coffee
tea
water
```

There is a newline after every entry, including the last. The source stays intact.
The destination must be new: repeating the command with the same destination
fails with BW4002 and preserves the existing file. Select a new destination for
another run:

```sh
cargo run -- --file examples/35-build-catalogue.botwork \
  --vars-file examples/inputs/catalogue.json \
  --var 'destination_file="catalogue-second.txt"'
```

The [configuration file](../examples/inputs/catalogue.json) supplies:

| Variable | Purpose |
| --- | --- |
| `source_file` | Existing UTF-8 list, one entry per line |
| `destination_file` | New output file; its parent must exist |
| `scratch_directory` | Existing directory for a unique staging directory |
| `sort_program` | Trusted executable implementing `sort -u`; default `/usr/bin/sort` |
| `expected_entries` | Exact expected array, already in byte-sorted order |

Override variables with `--var` JSON values, or copy the JSON file for a new task.
Changing the input list also requires updating `expected_entries`. The assertion
checks the actual entries against that independently supplied expectation.

The imported [catalogue module](../examples/modules/catalogue.botwork) trims
Unicode whitespace, drops blank lines, and rejects an empty catalogue. LF and
CRLF input are accepted. It passes the cleaned text on stdin to `sort -u`, with
an explicit `LC_ALL=C` environment and no inherited environment. This makes
ordering byte-based and preserves case and Unicode spellings. It neither folds
case nor normalizes Unicode. The executable is invoked with a literal argument
array. The process has a ten-second timeout, a 64 KiB stdout cap and a 4 KiB
stderr cap. Nonzero exit or any stderr output fails validation.

The main script asserts the expected entries, writes and reads back a staged
file, exclusively copies it to the destination, then checks the published bytes.
`Finally` removes the staging directory after success, assertion failure,
process failure, or copy failure. A failure before copying creates no destination.
As with the underlying [filesystem statements](operating-system.md), a copy or
readback failure can leave a new partial or complete destination; publication is
not an atomic transaction. Terminating the host can interrupt cleanup.

To see a validation failure without publishing a file:

```sh
cargo run -- --file examples/35-build-catalogue.botwork \
  --vars-file examples/inputs/catalogue.json \
  --var 'expected_entries=["unexpected"]' \
  --var 'destination_file="rejected-catalogue.txt"'
```

The command exits nonzero with BW9001 and a source location. It prints no success
message, leaves `rejected-catalogue.txt` absent, and removes its staging directory.

## Check an HTTP response contract

[Example 36](../examples/36-http-contract.botwork) sends one GET and checks a
configured status, required response headers, and exact body text. To try it
locally, start this fixture server in another terminal (Python 3 required):

```sh
python3 -m http.server 8765 --bind 127.0.0.1 \
  --directory examples/fixtures/http-contract
```

Then run:

```sh
cargo run -- --file examples/36-http-contract.botwork \
  --vars-file examples/inputs/http-contract.json \
  --var 'url="http://127.0.0.1:8765/hello.json"'
```

Success prints `HTTP contract passed (200)`. Stop the fixture server with Ctrl-C.
The [fixture body](../examples/fixtures/http-contract/hello.json) has no trailing
newline, because body comparison is exact.

Copy [the contract configuration](../examples/inputs/http-contract.json) and
change its `contract` map to check your own endpoint:

| Field | Meaning |
| --- | --- |
| `status` | Exact expected HTTP status, including a deliberate error status |
| `request_headers` | Request header names and values |
| `headers` | Required lowercase response names mapped to arrays of exact values |
| `body` | Exact expected UTF-8 response text |
| `timeout_ms` | Total request allowance; bundled value is 10000 ms |
| `max_body_bytes` | Maximum response body; bundled value is 4096 bytes |

The imported [HTTP module](../examples/modules/http-contract.botwork) receives
the URL and contract explicitly. It checks required headers while allowing extra
response headers, and compares repeated values in their received order. Redirects
and retries are explicitly disabled. A 3xx response is checked as received;
transport errors and timeouts fail the run. See [HTTP statements](http.md) for
TLS, resource limits, and complete transport behavior.

This is a text contract: JSON whitespace, key order, and trailing newlines matter.
It does not parse or compare JSON structures. Use it for endpoints with stable
text, or replace the expected body with your endpoint's exact representation.

For a failing demonstration, change the URL path to `/missing.json`. The local
server responds with 404; the imported status assertion emits BW9001 with its
module source location, the CLI exits nonzero, and no success message is printed.

## Drive a browser or a device

[`examples/browser`](../examples/browser/README.md) holds one runnable example
for each browser and device integration: it signs in on a sample page with
Chrome through [WebDriver](webdriver.md) or with Chromium through
[Playwright](playwright.md), or opens Android's Settings through
[Appium](appium.md). Each needs its browser, driver, or device, which its
README lists with how to install them, and each closes what it opened in
`Finally`, as the run itself would. Run them from that directory:

```sh
cd examples/browser
botwork --file webdriver.botwork --vars-file inputs.json
```

`cargo test --locked --test browser_examples` checks every example, and runs
each one where its prerequisites are configured: `BOTWORK_WEBDRIVER`
(chromedriver), `BOTWORK_PLAYWRIGHT` (a directory with Playwright), or
`BOTWORK_APPIUM` (Appium, with a device). CI sets each in the job that has it.
A run must log what the example promises, write its screenshot, and leave no
process behind.

## Paths and verification

Imports resolve relative to the `.botwork` source file. Filesystem inputs and
outputs resolve relative to the CLI working directory, including paths supplied
inside a JSON variables file. Use absolute file paths when launching from another
directory. Module variables are separate from caller variables, so the helpers
take their needed configuration as arguments.

```sh
cargo test --locked --test automation_examples
```

The checks cover bundled inputs, overrides, Unicode and CRLF text, literal paths
with spaces and shell punctuation, retained source/destination contents, temporary
cleanup on success and failure, process launch and exit failures, exact HTTP
status/header/body assertions, and a configured deadline. HTTP tests use local
loopback fixtures and require no internet service. [Assertion diagnostics](assertion-diagnostics.md),
[polling](polling.md), and [structured data](structured-data.md) now cover
differences, eventual conditions, and dataset-driven JSON/HTTP checks. Complete
reference workflows retain their separate roadmap item.
