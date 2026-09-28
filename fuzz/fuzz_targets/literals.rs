#![no_main]
#[path = "../../tests/support/generated.rs"]
mod generated;
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    generated::literals(data);
});
