// Force a panic outside the callback's catch boundary. Dropping the caught
// payload raises the replacement String, which Tokio reports as a JoinError.
// The message already exists before the interpreter formats that join failure.
pub struct EscapingPanic(pub Option<String>);

impl Drop for EscapingPanic {
    fn drop(&mut self) {
        std::panic::panic_any(self.0.take().unwrap());
    }
}
