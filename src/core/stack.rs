//! Stack headroom. Depth limits bound recursion, but the stack each level takes
//! depends on the target and build profile, and embedding hosts choose their
//! threads' stack sizes. Evaluation levels, the parser, and JSON decoding
//! therefore also check the stack left on the current thread, so deep work stops
//! with a limit error instead of overflowing the stack. See docs/embedded-runs.md.

use super::value_limits::MAX_VALUE_DEPTH;
use std::cell::Cell;

/// The resource a headroom failure names.
pub(crate) const RESOURCE: &str = "stack headroom bytes";

/// Stack an evaluation level keeps free for its own frames and the value work
/// beneath it, which the value depth ceiling bounds.
pub(crate) const LEVEL: usize = if DEBUG { 256 * KIB } else { 128 * KIB };

// Measured on x86_64 at the ceilings, then given half again as much for other
// targets and grammar paths: nested arrays cost the most per syntax unit (17
// KiB unoptimized, 3.2 KiB optimized) and JSON 14 KiB or 1 KiB per level.
const PARSER_BASE: usize = if DEBUG { 128 * KIB } else { 48 * KIB };
const PARSER_UNIT: usize = if DEBUG { 26 * KIB } else { 6 * KIB };
const JSON_BASE: usize = if DEBUG { 128 * KIB } else { 48 * KIB };
const JSON_LEVEL: usize = if DEBUG { 21 * KIB } else { 2 * KIB };

/// Stack the parser and AST lowering need for a source whose peak combined
/// syntax complexity (twice the nesting plus the operators) is `complexity`.
pub(crate) const fn parser(complexity: usize) -> usize {
    PARSER_BASE + complexity * PARSER_UNIT
}

/// Stack JSON decoding needs for a document nested `depth` containers deep.
/// Conversion stops at the value depth ceiling; raw decoding below it is cheap.
pub(crate) const fn json(depth: usize) -> usize {
    let depth = if depth < MAX_VALUE_DEPTH {
        depth
    } else {
        MAX_VALUE_DEPTH
    };
    JSON_BASE + depth * JSON_LEVEL
}

/// Unoptimized builds take several times as much stack per frame.
const DEBUG: bool = cfg!(debug_assertions);
const KIB: usize = 1024;

thread_local! {
    /// The lowest address of this thread's stack, found on first use; zero
    /// when the platform cannot say.
    static LOWEST: Cell<Option<usize>> = const { Cell::new(None) };
}

#[cfg(test)]
thread_local! {
    /// Stack a unit test claims is left, so reserves can be checked exactly.
    static SIMULATED: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Run `work` as though `left` bytes of stack remained on this thread.
#[cfg(test)]
pub(crate) fn simulate<T>(left: usize, work: impl FnOnce() -> T) -> T {
    SIMULATED.with(|simulated| simulated.set(Some(left)));
    let result = work();
    SIMULATED.with(|simulated| simulated.set(None));
    result
}

/// Whether the current thread has less than `needed` bytes of stack left.
/// False where the stack's extent is unknown.
pub(crate) fn short_of(needed: usize) -> bool {
    remaining().is_some_and(|left| left < needed)
}

/// Bytes of stack left below the caller on the current thread.
pub(crate) fn remaining() -> Option<usize> {
    #[cfg(test)]
    if let Some(left) = SIMULATED.with(Cell::get) {
        return Some(left);
    }
    let lowest = LOWEST
        .try_with(|lowest| {
            lowest.get().unwrap_or_else(|| {
                let address = bounds().map_or(0, |(low, _)| low);
                lowest.set(Some(address));
                address
            })
        })
        .ok()
        .filter(|&address| address != 0)?;
    let marker = 0u8;
    let here = std::hint::black_box(&marker) as *const u8 as usize;
    Some(here.saturating_sub(lowest))
}

/// The current thread's stack, as its lowest and highest addresses.
#[cfg(target_os = "linux")]
fn bounds() -> Option<(usize, usize)> {
    // SAFETY: pthread_getattr_np initializes the attributes before they are
    // read, and they are destroyed once; getrlimit writes a zeroed rlimit.
    unsafe {
        let mut attributes = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
        if libc::pthread_getattr_np(libc::pthread_self(), attributes.as_mut_ptr()) != 0 {
            return None;
        }
        let mut attributes = attributes.assume_init();
        let (mut address, mut size) = (std::ptr::null_mut(), 0);
        let status = libc::pthread_attr_getstack(&attributes, &mut address, &mut size);
        libc::pthread_attr_destroy(&mut attributes);
        if status != 0 {
            return None;
        }
        let high = (address as usize).checked_add(size)?;
        // The main thread's stack grows on demand up to the soft limit; musl
        // reports only the part mapped so far.
        if libc::syscall(libc::SYS_gettid) == libc::c_long::from(libc::getpid()) {
            let mut limit: libc::rlimit = std::mem::zeroed();
            if libc::getrlimit(libc::RLIMIT_STACK, &mut limit) != 0
                || limit.rlim_cur == libc::RLIM_INFINITY
            {
                return None;
            }
            let size = usize::try_from(limit.rlim_cur).ok()?;
            return Some((high.saturating_sub(size), high));
        }
        Some((address as usize, high))
    }
}

#[cfg(target_os = "macos")]
fn bounds() -> Option<(usize, usize)> {
    // SAFETY: both calls only read the current thread's recorded stack.
    unsafe {
        let thread = libc::pthread_self();
        let high = libc::pthread_get_stackaddr_np(thread) as usize;
        let size = libc::pthread_get_stacksize_np(thread);
        Some((high.checked_sub(size)?, high))
    }
}

#[cfg(windows)]
fn bounds() -> Option<(usize, usize)> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
    }
    let (mut low, mut high) = (0, 0);
    // SAFETY: the call writes the current thread's two stack limits.
    unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
    (low < high).then_some((low, high))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn bounds() -> Option<(usize, usize)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        ast::{native_signature, Program},
        input::parse_variable,
        run::{Engine, RunOptions},
    };

    fn headroom(error: &impl std::fmt::Display, needed: usize) -> bool {
        let message = error.to_string();
        message.contains(RESOURCE) && message.contains(&format!("(limit {needed})"))
    }

    #[test]
    fn reserves_match_the_documented_table() {
        // docs/embedded-runs.md#stack-headroom, in KiB: a level's reserve, then
        // the parser's and JSON's base and increment, then their largest.
        let kib = |bytes: usize| bytes / KIB;
        let table = [
            kib(LEVEL),
            kib(parser(0)),
            kib(parser(1) - parser(0)),
            kib(json(0)),
            kib(json(1) - json(0)),
            kib(parser(crate::core::syntax_limits::MAX_SYNTAX_COMPLEXITY)),
            kib(json(crate::core::input::MAX_JSON_DEPTH)),
        ];
        let expected = if DEBUG {
            [256, 128, 26, 128, 21, 1844, 1472]
        } else {
            [128, 48, 6, 48, 2, 444, 176]
        };
        assert_eq!(table, expected);
    }

    #[test]
    fn evaluation_levels_keep_their_reserve_free() {
        // The source parses within less than a level's reserve, so only
        // evaluation can fail.
        assert!(parser(2) < LEVEL);
        let run = |left| {
            simulate(left, || {
                Engine::default().run_source("level", "|x| = |1|", RunOptions::default())
            })
        };
        assert!(run(LEVEL).result.is_ok());
        let error = run(LEVEL - 1).result.unwrap_err();
        assert!(headroom(&error, LEVEL), "{error}");
    }

    #[test]
    fn parsing_keeps_a_reserve_for_the_peak_complexity() {
        // Twice the nesting plus the operators: a pipe holding two brackets
        // nests three deep, and one pipe may hold two operators.
        for (source, peak) in [
            ("|x| = |[[1]]|", 6),
            ("|x| = |1 + 2 * 3|", 4),
            ("Log |1|", 2),
        ] {
            let parse = |left| simulate(left, || Program::parse_detailed("peak", source));
            assert!(parse(parser(peak)).is_ok(), "{source}");
            let error = parse(parser(peak) - 1).unwrap_err();
            assert!(headroom(&error, parser(peak)), "{source}: {error}");
        }
    }

    #[test]
    fn signature_headers_parse_without_the_check() {
        let parsed = simulate(0, || {
            native_signature("signature", "Create Map From |entries|")
        });
        assert!(parsed.is_ok());
    }

    #[test]
    fn json_keeps_a_reserve_for_its_nesting_up_to_the_value_depth_ceiling() {
        let decode =
            |left, text: &str| simulate(left, || parse_variable("json", &format!("x={text}")));
        assert!(decode(json(2), "[[1]]").is_ok());
        let error = decode(json(2) - 1, "[[1]]").unwrap_err();
        assert!(headroom(&error, json(2)), "{error}");
        // Conversion stops at the value depth ceiling, so a deeper document
        // needs no more than the ceiling's reserve to be refused for its depth.
        let deep = format!("{}1{}", "[".repeat(100), "]".repeat(100));
        let error = decode(json(MAX_VALUE_DEPTH), &deep).unwrap_err();
        assert!(error.to_string().contains("value depth"), "{error}");
    }

    fn descend(levels: usize) -> usize {
        let frame = std::hint::black_box([0u8; 4096]);
        if levels == 0 {
            return remaining().unwrap() + usize::from(frame[0]);
        }
        descend(levels - 1)
    }

    #[test]
    fn the_stack_is_known_and_holds_the_caller() {
        let (low, high) = bounds().expect("supported platform");
        let marker = 0u8;
        let here = std::hint::black_box(&marker) as *const u8 as usize;
        assert!(
            low < here && here < high,
            "{low:#x} < {here:#x} < {high:#x}"
        );
        let left = remaining().unwrap();
        assert!(0 < left && left < high - low);
        assert!(!short_of(0) && short_of(left + 1));
    }

    #[test]
    fn deeper_frames_leave_less_stack() {
        let shallow = remaining().unwrap();
        let deep = descend(16);
        assert!(shallow - deep >= 16 * 4096, "{shallow} then {deep}");
    }

    #[test]
    fn spawned_threads_report_the_size_they_asked_for() {
        for size in [256 * 1024, 1024 * 1024, 4 * 1024 * 1024] {
            let left = std::thread::Builder::new()
                .stack_size(size)
                .spawn(|| remaining().unwrap())
                .unwrap()
                .join()
                .unwrap();
            // At least what was asked for, less a guard page; glibc may
            // reuse a cached stack up to four times as large.
            assert!(left + 64 * 1024 >= size, "{size}: {left}");
            assert!(left <= 4 * size + 64 * 1024, "{size}: {left}");
        }
    }
}
