# HTTP statements

Four fixed statements make HTTP/1.1 requests with verified HTTPS:

| Statement | Request and response representation |
| --- | --- |
| `HTTP Request \|method\| To \|url\|` | UTF-8 text |
| `HTTP Request \|method\| To \|url\| Options \|options\|` | UTF-8 text with options |
| `HTTP Binary Request \|method\| To \|url\|` | Arrays of Int bytes, 0–255 |
| `HTTP Binary Request \|method\| To \|url\| Options \|options\|` | Byte Arrays with options |

Calls require asynchronous execution. The CLI enables Tokio I/O and time; embedding
hosts must enable both drivers and use `Engine::run_source_async` or another async
entry point. Synchronous dispatch fails with BW5003 before evaluating arguments.
Host registrations can override these signatures. Statement names are case
insensitive; method strings, option keys and variables retain their case.

`method` is a nonempty HTTP token of at most 64 bytes; standard methods should use
uppercase. Extension methods are accepted. CONNECT tunnels and protocol upgrades
are unsupported. `url` must be an absolute HTTP or HTTPS URL with a host and no
credentials, whitespace, controls or backslashes. Fragments are removed; URLs
are normalized with the `url` crate, including IDNA hostnames. Both supplied and
normalized URLs are limited to 8,192 bytes. Relative URLs are allowed only in
redirect responses. Network destinations use the host's ordinary network access;
these statements do not establish a network sandbox.

## Result

Every fully received HTTP status, including 4xx and 5xx, returns a seven-field Map:

| Key | Value |
| --- | --- |
| `status` | Int HTTP status |
| `success` | Bool: status is 200–299 |
| `headers` | Map of lowercase header names to Arrays of values; duplicates and trailers are retained |
| `body` | String for text calls; byte Array for binary calls |
| `url` | String: normalized final URL without a fragment |
| `redirects` | Int count of followed redirects |
| `attempts` | Int count of HTTP exchanges, including redirects and status retries |

Header values are strict UTF-8 Strings in text calls and byte Arrays in binary
calls. For example, a binary `x-tag: abc` becomes `[[97, 98, 99]]` at
`result.headers["x-tag"]`. Leading/trailing HTTP header whitespace is normalized
by the HTTP parser. Header field ordering is not exposed; duplicate values retain
their received order. Trailers join the same map and share its limits. Text does
not guess a charset, substitute invalid bytes, or strip a BOM. There is no
automatic decompression: compressed content remains bytes, even when a server
sends a Content-Encoding header. HEAD has no response body even if Content-Length
describes a larger resource.

An incomplete body, invalid framing or failed TLS verification is an error;
partially received data never becomes a successful response value. A failed call
does not replace its assignment destination. A server may have processed a
request before its response fails or a caller stops waiting.

## Options

Unknown keys, wrong kinds and out-of-range values fail before connecting.

| Key | Default | Contract |
| --- | --- | --- |
| `headers` | `{}` | Map: each value is a String or Array of Strings; empty Arrays add nothing |
| `body` | empty | String for text calls; byte Array for binary calls; at most 1 MiB |
| `timeout_ms` | 30000 | Int 0–86400000; a single transport deadline; zero stops before connecting |
| `max_body_bytes` | 1048576 text; 16384 binary | Int 0–1048576; complete response body limit |
| `max_headers` | 128 | Int 0–128; header occurrences, including duplicate values and trailers |
| `max_header_bytes` | 16384 | Int 0–65536; sum of names and raw value bytes, counting duplicate names |
| `redirects` | `"none"` | `"none"`, `"same-origin"`, or `"any-origin"` |
| `max_redirects` | 10 | Int 0–10; total followed redirects across this call |
| `max_retries` | 0 | Int 0–5; total additional exchanges after retryable statuses |
| `retry_delay_ms` | 100 | Int 0–60000; fixed delay before each status retry |
| `ca_pem` | omitted | String PEM certificates, at most 64 KiB; additional trusted roots for this call |

Request headers are limited to 128 occurrences and 16 KiB of names plus values;
individual names are limited to 256 bytes. Invalid token names and CR/LF/NUL in
values are rejected. Host, Content-Length, Transfer-Encoding, Connection,
Trailer, Upgrade, Proxy-Authorization, Proxy-Connection and TE are owned by the
transport and cannot be supplied. Content-Type is not inferred; set it when needed.
The transport sends Connection: close and uses a new connection for every exchange.

Rustls verifies certificate chains and the URL hostname against the bundled
Mozilla roots plus any `ca_pem` certificates. There is no verification bypass.
The client does not read proxy environment variables, use platform trust stores,
share connections or cookie jars, save Set-Cookie values, or add Referer headers.
An explicit Cookie or Authorization header is an ordinary per-call header.
HTTP/2, HTTP/3, client certificates, streaming outputs and multipart builders are
outside this contract.

## Redirects and retries

With redirects off, a 3xx is returned directly. Opted-in redirects follow only
301, 302, 303, 307 and 308 with one valid Location. A missing Location returns the
3xx; multiple or invalid locations fail. Same-origin means equal scheme,
normalized hostname and effective port. Exceeding the redirect count fails.
HTTPS-to-HTTP redirects always fail, including with `any-origin`.

301/302 change POST to GET; 303 changes methods other than HEAD to GET. These
rewrites discard the body and user content-* headers, Digest and Expect. 307/308
preserve the method and body. Across origins, the resulting request must be GET
or HEAD with an empty body; **all user headers are removed**, including arbitrary
API-key headers. Same-origin redirects preserve remaining user headers.

Retries require an original GET or HEAD with an empty body, validated before the
first exchange. Only statuses 429, 502, 503 and 504 are retried. The configured
fixed delay is used; Retry-After is not interpreted. DNS, TLS, connection, framing,
UTF-8, policy and resource errors are not retried. Retries target the current URL
after any redirects and preserve its current headers. When retries are exhausted,
the last complete HTTP response is returned normally. Redirect and retry counts
are global to the call, so at most 16 exchanges are possible. Intermediate bodies
must be received completely within the same response limits before continuing.

## Timeouts, cancellation and admission

The local timeout begins after parameter and resource admission and covers DNS,
TCP/TLS setup, headers, body, all redirects, and retry delays. It cannot extend an
inherited run deadline. Local timeouts latch BW5002 on the containing run;
cancellation yields BW5001. Catch cannot consume these stops. Finally receives
its existing independent cleanup allowance. Dropping a suspended run closes its
socket; dropping Rust futures does not execute DSL Finally blocks.

Hyper's connection future is polled inside the request, not on a detached task.
Sockets and TLS state drop with that future. OS DNS lookups run on blocking
workers because the platform resolver can block. A queued lookup is aborted on
drop; an already started lookup cannot be forcibly interrupted and retains its
admission leases until it returns, including while its result is undelivered.
Tokio runtime shutdown may wait for such platform work. At most 64 returned
addresses are accepted; TCP addresses are tried sequentially under the same
deadline. An unresponsive earlier address can consume the remaining deadline.

Shared admission allows at most 32 calls and 64 MiB of logical in-flight bytes;
exhaustion fails immediately with BW8001. Each call reserves a conservative 4 MiB
workspace before owned configuration copies and the maximum output shape before
network effects. The shared byte limit can bind before the slot limit. Workspace
and output also count against the run's temporary budgets. Binary bytes count as
Int nodes and four logical payload bytes each. The planned maximum header and URL
sizes participate even for small eventual responses; reduce option maxima when
using smaller value budgets. Unused output reservation is released before return.
These limits account for logical owned payload and bounded transport buffers,
not allocator overhead, OS resolver internals, socket kernel memory or exact RSS.

A separate 64 KiB Hyper parsing buffer bounds response head/framing storage;
header parser exhaustion is BW8001. This buffer also includes HTTP framing, so it
can reject a response below the configured names-plus-values limit. Every final
header name is limited to 256 bytes. Response limits apply before body accumulation
or output conversion; no truncated success value is returned.

BW3003 reports invalid parameters or UTF-8. BW4002 reports transport, TLS, framing
or redirect-policy failure. BW5003 reports unavailable async/runtime support.
BW8001 reports admission or response bounds. Transport diagnostics omit URLs,
headers, bodies and TLS material in their error messages. Ordinary source and
call diagnostics still show source text, including literal arguments. These
statements do not mark or redact secrets in source diagnostics or explicit logs.

## Executed example

Set `BOTWORK_HTTP_URL` to an endpoint returning status 200,
`Content-Type: application/json` and the exact body `{"message":"hello"}`.
For a local fixture, place that content in a file named `hello` and run an HTTP
server configured to give it that media type. The automated example and
reference tests supply their own loopback endpoint; they require no public service.

<!-- botwork-test: http-statements -->
```botwork
|url| = Get Environment Variable |"BOTWORK_HTTP_URL"|
|response| = HTTP Request |"GET"| To |url| Options |{"headers": {"accept": "application/json"}, "timeout_ms": 3000, "max_body_bytes": 4096}|
Assert |response.status| Equals |200|
Assert |response.headers["content-type"]| Equals |["application/json"]|
Assert |response.body| Equals |"{\"message\":\"hello\"}"|
Log |response.status|
Log |response.body|
```

Output:

```text
200
{"message":"hello"}
```

The other forms, against the same endpoint, with their output in comments:

<!-- botwork-test: http-variants -->
```botwork
|url| = Get Environment Variable |"BOTWORK_HTTP_URL"|
Log |@{ HTTP Request |"GET"| To |url| }.status|                             # 200
|binary| = HTTP Binary Request |"GET"| To |url|
Log |@{ Length Of |binary.body| }|                                          # 19 bytes
|binary| = HTTP Binary Request |"GET"| To |url| Options |{"timeout_ms": 3000}|
Log |binary.success|                                                        # true
```

The equivalent file is [example 34](../examples/34-http.botwork).
The transport uses [Hyper's connection builder](https://docs.rs/hyper/1.11.1/hyper/client/conn/http1/struct.Builder.html)
and [Tokio Rustls](https://docs.rs/tokio-rustls/0.26.6/tokio_rustls/client/struct.TlsConnector.html).
