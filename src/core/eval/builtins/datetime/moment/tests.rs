use super::*;

#[test]
fn wall_clock_conversion_preserves_nanoseconds_on_both_sides_of_epoch() {
    let context = Context::default();
    // Windows keeps system time in 100 ns ticks.
    let tick: i128 = if cfg!(windows) { 100 } else { 1 };
    for value in [-1_000_000_000 - tick, -tick, 0, tick, 1_000_000_000 + tick] {
        let delta = std::time::Duration::from_nanos(value.unsigned_abs() as u64);
        let time = if value < 0 {
            UNIX_EPOCH - delta
        } else {
            UNIX_EPOCH + delta
        };
        assert_eq!(nanos(system_time(&context, time).unwrap()), value);
    }
}

#[test]
fn timestamp_construction_checks_integer_and_calendar_boundaries() {
    let context = Context::default();
    for value in [
        i128::MIN,
        i128::MAX,
        -62_167_219_200_000_000_001,
        253_402_300_800_000_000_000,
    ] {
        assert!(from_nanos(&context, value).is_err(), "{value}");
    }
    assert_eq!(
        nanos(parse(&context, "0000-01-01T00:00:00Z").unwrap()),
        -62_167_219_200_000_000_000
    );
    assert_eq!(
        chrono_tz::IANA_TZDB_VERSION,
        "2025b",
        "Review documented timezone data and supported transition horizon on updates"
    );
}
