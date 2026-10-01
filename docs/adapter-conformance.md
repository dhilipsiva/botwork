# Adapter conformance

Every language adapter a build advertises is held to one suite,
`tests/adapter_conformance.rs` ([D6](decisions.md#d6-adapters)): each loads a
module with the same statements, and each scenario runs against all of them.

| Scenario | What every adapter must do |
| --- | --- |
| Unicode | Return text unchanged: empty, accented, CJK, emoji with modifiers, combining marks, right-to-left, and the edges of the code-point range |
| Numeric boundaries | Return the extreme 32-bit Ints, a fraction, and the largest, smallest normal, and smallest subnormal 32-bit Floats, each bit for bit and of the same kind |
| Nested values and None | Return maps, arrays, empty containers, None, and ten levels of nesting unchanged |
| Async results | Await an asynchronous function's result |
| Errors | Report a failure as BW4002 with its message, and a failed assertion as BW9001; a script catches both, and its `Finally` runs |
| Cancellation | End a busy call at a deadline within the stop grace, abandoning nothing; the next run uses the adapter normally |
| Ownership | Give the module copies: a module that appends to an array it was passed changes neither the caller's array nor its next call's |
| Unsupported values | Fail with BW4002 naming the value, and bind nothing, for any value without an exact Botwork equivalent |

The suite covers [WebAssembly](wasm.md) in builds with the default `wasm`
feature, [JavaScript](javascript.md) where Node is on `PATH`, and
[Python](python.md) in builds with `python`. CI runs it in every Rust job, with
Node required, and in the Python jobs, so all three run on Linux, macOS, and
Windows.

## Where adapters differ

The languages differ, and the suite records how:

- **WebAssembly has no asynchronous calls.** A component returns a value, so
  the async scenario leaves it out.
- **JavaScript has one number type.** A whole number comes back as an Int, so
  the Float `3.0` returns as the Int `3`; the suite asserts this. A whole
  number beyond 32 bits returns as a Float only when a 32-bit Float holds it
  exactly, and otherwise fails, rather than rounding to another number.
- **Unsupported values differ by language.** WebAssembly refuses values that
  are not trees and Floats that are not finite; JavaScript, whole numbers it
  would round, `NaN`, unpaired surrogates, and class instances; Python, Ints
  beyond 32 bits, `nan`, unpaired surrogates, and objects. A fraction rounds
  to the nearest 32-bit Float in JavaScript and Python, as a Float does in
  any 32-bit arithmetic.
