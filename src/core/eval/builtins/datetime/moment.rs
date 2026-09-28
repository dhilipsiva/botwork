use super::*;
use chrono::{
    format::{Fixed, Item, Numeric, StrftimeItems},
    DateTime, Datelike, FixedOffset, LocalResult, NaiveDateTime, Offset, TimeZone, Timelike, Utc,
};
use duration::SECOND;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn parse(context: &Context, text: &str) -> EvaluationResult<DateTime<Utc>> {
    let fail = || {
        invalid(context, "Expected YYYY-MM-DDTHH:MM:SS[.fraction]Z or an explicit ±HH:MM offset; leap seconds and unknown -00:00 offsets are unsupported")
    };
    let bytes = text.as_bytes();
    if bytes.len() < 20
        || !bytes[..19].iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            _ => c.is_ascii_digit(),
        })
    {
        return Err(fail());
    }
    let mut offset_start = 19;
    if bytes[offset_start] == b'.' {
        offset_start += 1;
        let length = bytes[offset_start..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if !(1..=9).contains(&length) {
            return Err(fail());
        }
        offset_start += length;
    }
    let offset = &text[offset_start..];
    if offset != "Z" {
        fixed(context, offset)?;
    }
    let parsed = DateTime::parse_from_rfc3339(text).map_err(|_| fail())?;
    if parsed.nanosecond() >= SECOND as u32 {
        return Err(fail());
    }
    let utc = parsed.with_timezone(&Utc);
    year(context, utc.year(), false)?;
    Ok(utc)
}

fn fixed(context: &Context, text: &str) -> EvaluationResult<FixedOffset> {
    let b = text.as_bytes();
    if b.len() != 6
        || !matches!(b[0], b'+' | b'-')
        || b[3] != b':'
        || ![b[1], b[2], b[4], b[5]].iter().all(u8::is_ascii_digit)
        || text == "-00:00"
    {
        return Err(invalid(
            context,
            "Expected an explicit ±HH:MM offset; -00:00 is unknown and unsupported",
        ));
    }
    let hours = i32::from(b[1] - b'0') * 10 + i32::from(b[2] - b'0');
    let minutes = i32::from(b[4] - b'0') * 10 + i32::from(b[5] - b'0');
    if hours > 23 || minutes > 59 {
        return Err(invalid(
            context,
            "Offset must be smaller than 24 hours with minutes below 60",
        ));
    }
    let seconds = (hours * 3600 + minutes * 60) * if b[0] == b'-' { -1 } else { 1 };
    Ok(FixedOffset::east_opt(seconds).expect("validated offset"))
}

pub(super) enum Zone {
    Fixed(FixedOffset),
    Named(chrono_tz::Tz),
}
impl Zone {
    pub(super) fn parse(context: &Context, text: &str) -> EvaluationResult<Self> {
        if matches!(text, "UTC" | "Z") {
            return Ok(Self::Fixed(FixedOffset::east_opt(0).unwrap()));
        }
        if text.starts_with(['+', '-']) {
            return fixed(context, text).map(Self::Fixed);
        }
        text.parse::<chrono_tz::Tz>().map(Self::Named).map_err(|_| {
            invalid(
                context,
                "Unknown timezone; use UTC, Z, ±HH:MM, or an exact IANA name",
            )
        })
    }

    pub(super) fn render(
        &self,
        context: &Context,
        instant: DateTime<Utc>,
        format: Option<&str>,
    ) -> TemporaryResult {
        match self {
            Self::Fixed(zone) => render(context, instant.with_timezone(zone), format, false),
            Self::Named(zone) => render(context, instant.with_timezone(zone), format, true),
        }
    }

    fn resolve(
        &self,
        context: &Context,
        naive: NaiveDateTime,
        choice: &str,
    ) -> EvaluationResult<DateTime<Utc>> {
        let named = matches!(self, Self::Named(_));
        year(context, naive.year(), named)?;
        let result = match self {
            Self::Fixed(zone) => zone
                .from_local_datetime(&naive)
                .map(|v| v.with_timezone(&Utc)),
            Self::Named(zone) => zone
                .from_local_datetime(&naive)
                .map(|v| v.with_timezone(&Utc)),
        };
        let instant = match result {
            LocalResult::Single(value) => value,
            LocalResult::None => {
                return Err(invalid(
                    context,
                    "The local time does not exist in this timezone",
                ))
            }
            LocalResult::Ambiguous(a, b) => match choice {
                "earlier" => a.min(b),
                "later" => a.max(b),
                _ => {
                    return Err(invalid(
                        context,
                        "The local time occurs twice; choose earlier or later",
                    ))
                }
            },
        };
        year(context, instant.year(), named)?;
        Ok(instant)
    }
}

fn year(context: &Context, year: i32, named: bool) -> EvaluationResult<()> {
    // chrono-tz's bundled transition generator expands recurring rules for
    // 1800..2100. Do not extend its terminal offset indefinitely into the future.
    let supported = if named {
        (1800..2100).contains(&year)
    } else {
        (0..=9999).contains(&year)
    };
    if supported {
        Ok(())
    } else {
        Err(overflow(context))
    }
}

fn format_valid(context: &Context, format: &str, local: bool) -> EvaluationResult<()> {
    for item in StrftimeItems::new(format) {
        context.checkpoint()?;
        let forbidden = match &item {
            Item::Error => true,
            Item::Numeric(Numeric::Timestamp, _) => local,
            Item::Fixed(spec) if local => !matches!(
                spec,
                Fixed::ShortMonthName
                    | Fixed::LongMonthName
                    | Fixed::ShortWeekdayName
                    | Fixed::LongWeekdayName
                    | Fixed::LowerAmPm
                    | Fixed::UpperAmPm
                    | Fixed::Nanosecond3
                    | Fixed::Nanosecond6
                    | Fixed::Nanosecond9
            ),
            _ => false,
        };
        if forbidden {
            return Err(invalid(context, "Invalid format; local parsing forbids epoch/zone fields and variable fractional precision (use %.3f, %.6f, or %.9f)"));
        }
    }
    Ok(())
}

pub(super) fn local(
    context: &Context,
    text: &str,
    format: &str,
    zone: &str,
    choice: &str,
) -> EvaluationResult<DateTime<Utc>> {
    if !matches!(choice, "reject" | "earlier" | "later") {
        return Err(invalid(
            context,
            "Local-time choice must be reject, earlier, or later",
        ));
    }
    let zone = Zone::parse(context, zone)?;
    format_valid(context, format, true)?;
    let naive = NaiveDateTime::parse_from_str(text, format).map_err(|_| {
        invalid(
            context,
            "Local date/time does not match the calendar format",
        )
    })?;
    if naive.nanosecond() >= SECOND as u32 {
        return Err(invalid(context, "Leap seconds are unsupported"));
    }
    zone.resolve(context, naive, choice)
}

fn render<T: TimeZone>(
    context: &Context,
    date: DateTime<T>,
    format: Option<&str>,
    named: bool,
) -> TemporaryResult
where
    T::Offset: fmt::Display,
{
    year(context, date.year(), named)?;
    year(context, date.naive_utc().year(), named)?;
    if let Some(format) = format {
        format_valid(context, format, false)?;
        // DelayedFormat::Display builds an unbounded intermediate String.
        // write_to streams each item through our pre-admission counter instead.
        // Its offset-name allocation is bounded by the bundled zone/fixed offset.
        let delayed = date.format_with_items(StrftimeItems::new(format));
        strings::formatted(context, |output| delayed.write_to(output))
    } else {
        let seconds = date.offset().fix().local_minus_utc();
        if seconds % 60 != 0 {
            return Err(context.detail_error(BWErr::ArithmeticError, "RFC 3339 cannot represent this historical offset exactly; use Format Date Time with %::z", None, false));
        }
        strings::formatted(context, |output| {
            write!(
                output,
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                date.year(),
                date.month(),
                date.day(),
                date.hour(),
                date.minute(),
                date.second()
            )?;
            duration::fraction(output, date.nanosecond())?;
            if seconds == 0 {
                output.write_char('Z')
            } else {
                let magnitude = seconds.unsigned_abs();
                write!(
                    output,
                    "{}{:02}:{:02}",
                    if seconds < 0 { '-' } else { '+' },
                    magnitude / 3600,
                    magnitude % 3600 / 60
                )
            }
        })
    }
}

pub(super) fn utc(context: &Context, value: DateTime<Utc>) -> TemporaryResult {
    render(context, value, None, false)
}

pub(super) fn nanos(value: DateTime<Utc>) -> i128 {
    i128::from(value.timestamp()) * SECOND + i128::from(value.timestamp_subsec_nanos())
}

pub(super) fn from_nanos(context: &Context, value: i128) -> EvaluationResult<DateTime<Utc>> {
    let seconds = i64::try_from(value.div_euclid(SECOND)).map_err(|_| overflow(context))?;
    let date = DateTime::from_timestamp(seconds, value.rem_euclid(SECOND) as u32)
        .ok_or_else(|| overflow(context))?;
    year(context, date.year(), false)?;
    Ok(date)
}

pub(super) fn system_time(context: &Context, time: SystemTime) -> EvaluationResult<DateTime<Utc>> {
    let value = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    };
    from_nanos(context, value)
}

#[cfg(test)]
mod tests;
