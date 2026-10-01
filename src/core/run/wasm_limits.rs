use super::*;

/// The most memory a WebAssembly instance can address: 4 GiB, all of wasm32.
pub const MAX_WASM_MEMORY_BYTES: u64 = 1 << 32;

/// Bounds on WebAssembly modules and each call into one (decision D8).
#[derive(Clone, Debug)]
pub struct WasmLimits {
    /// The largest module file an import reads.
    pub module_bytes: usize,
    /// Fuel for one call: about one unit for each WebAssembly instruction run.
    pub fuel: u64,
    /// The memory one call's instance may grow its linear memories to, in all.
    pub memory_bytes: usize,
    /// What one call may write to stdout, and to stderr.
    pub output_bytes: usize,
}

impl Default for WasmLimits {
    fn default() -> Self {
        Self {
            module_bytes: 32 * 1024 * 1024,
            fuel: 10_000_000_000,
            memory_bytes: 256 * 1024 * 1024,
            output_bytes: 1024 * 1024,
        }
    }
}

impl WasmLimits {
    pub(crate) fn validate(&self) -> DiagnosticResult<()> {
        if self.memory_bytes as u64 > MAX_WASM_MEMORY_BYTES {
            return Err(Diagnostic::formatted(
                BWErr::RunConfiguration,
                format_args!("WASM memory cannot exceed {MAX_WASM_MEMORY_BYTES} bytes"),
            ));
        }
        if self.fuel == 0 {
            return Err(Diagnostic::formatted(
                BWErr::RunConfiguration,
                format_args!("WASM fuel must be at least 1"),
            ));
        }
        Ok(())
    }
}
