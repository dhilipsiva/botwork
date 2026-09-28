use super::*;

pub(super) const SECOND: i128 = 1_000_000_000;
const MINUTE: i128 = 60 * SECOND;
const HOUR: i128 = 60 * MINUTE;
const DAY: i128 = 24 * HOUR;
const WEEK: i128 = 7 * DAY;

pub(super) fn unit(context: &Context, unit: &str) -> EvaluationResult<i128> {
    match unit {
        "weeks" => Ok(WEEK),
        "days" => Ok(DAY),
        "hours" => Ok(HOUR),
        "minutes" => Ok(MINUTE),
        "seconds" => Ok(SECOND),
        "milliseconds" => Ok(1_000_000),
        "microseconds" => Ok(1_000),
        "nanoseconds" => Ok(1),
        _ => Err(invalid(
            context,
            "Unknown duration unit; use a plural unit from weeks through nanoseconds",
        )),
    }
}

pub(super) fn parse(context: &Context, text: &str) -> EvaluationResult<i128> {
    let fail = || {
        invalid(context, "Expected a signed ISO duration of weeks or days/hours/minutes/seconds, with at most nine fractional second digits")
    };
    let (negative, text) = if let Some(rest) = text.strip_prefix('-') {
        (true, rest)
    } else {
        (false, text.strip_prefix('+').unwrap_or(text))
    };
    let mut rest = text.strip_prefix('P').ok_or_else(fail)?;
    let mut total = 0u128;
    let mut time = false;
    let mut rank = 0;
    let mut count = 0;
    while !rest.is_empty() {
        context.checkpoint()?;
        if let Some(after) = rest.strip_prefix('T') {
            if time || after.is_empty() {
                return Err(fail());
            }
            time = true;
            rest = after;
        }
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return Err(fail());
        }
        let integer = rest[..digits]
            .parse::<u128>()
            .map_err(|_| overflow(context))?;
        rest = &rest[digits..];
        let mut fraction = 0;
        let mut fractional = false;
        if let Some(after) = rest.strip_prefix('.') {
            fractional = true;
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            if !(1..=9).contains(&digits) {
                return Err(fail());
            }
            fraction = after[..digits]
                .parse::<u128>()
                .expect("nine decimal digits")
                * 10u128.pow(9 - digits as u32);
            rest = &after[digits..];
        }
        let Some(suffix) = rest.bytes().next() else {
            return Err(fail());
        };
        if !suffix.is_ascii() {
            return Err(fail());
        }
        rest = &rest[1..];
        let (next, scale) = match (time, suffix) {
            (false, b'W') if count == 0 && rest.is_empty() => (1, WEEK),
            (false, b'D') => (1, DAY),
            (true, b'H') => (2, HOUR),
            (true, b'M') => (3, MINUTE),
            (true, b'S') => (4, SECOND),
            _ => return Err(fail()),
        };
        if next <= rank || fractional && suffix != b'S' {
            return Err(fail());
        }
        rank = next;
        count += 1;
        total = integer
            .checked_mul(scale as u128)
            .and_then(|v| v.checked_add(fraction))
            .and_then(|v| total.checked_add(v))
            .ok_or_else(|| overflow(context))?;
    }
    if count == 0 {
        return Err(fail());
    }
    if negative && total == i128::MIN.unsigned_abs() {
        return Ok(i128::MIN);
    }
    let total = i128::try_from(total).map_err(|_| overflow(context))?;
    Ok(if negative { -total } else { total })
}

pub(super) fn fraction(output: &mut dyn fmt::Write, nanos: u32) -> fmt::Result {
    if nanos == 0 {
        return Ok(());
    }
    let mut value = nanos;
    let mut width = 9;
    while value.is_multiple_of(10) {
        value /= 10;
        width -= 1;
    }
    write!(output, ".{value:0width$}")
}

pub(super) fn seconds(output: &mut dyn fmt::Write, value: i128) -> fmt::Result {
    if value < 0 {
        output.write_char('-')?;
    }
    let value = value.unsigned_abs();
    write!(output, "{}", value / SECOND as u128)?;
    fraction(output, (value % SECOND as u128) as u32)
}

pub(super) fn render(context: &Context, value: i128) -> TemporaryResult {
    strings::formatted(context, |output| canonical(output, value))
}

fn canonical(output: &mut dyn fmt::Write, value: i128) -> fmt::Result {
    if value == 0 {
        return output.write_str("PT0S");
    }
    if value < 0 {
        output.write_char('-')?;
    }
    output.write_char('P')?;
    let value = value.unsigned_abs();
    let days = value / DAY as u128;
    if days != 0 {
        write!(output, "{days}D")?;
    }
    let remaining = value % DAY as u128;
    if remaining == 0 {
        return Ok(());
    }
    output.write_char('T')?;
    let hours = remaining / HOUR as u128;
    let minutes = remaining % HOUR as u128 / MINUTE as u128;
    let seconds = remaining % MINUTE as u128 / SECOND as u128;
    let nanos = (remaining % SECOND as u128) as u32;
    if hours != 0 {
        write!(output, "{hours}H")?;
    }
    if minutes != 0 {
        write!(output, "{minutes}M")?;
    }
    if seconds != 0 || nanos != 0 {
        write!(output, "{seconds}")?;
        fraction(output, nanos)?;
        output.write_char('S')?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
