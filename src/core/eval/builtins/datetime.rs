//! Explicit-zone timestamps and exact elapsed durations; no process-local zone.
use super::*;
use std::fmt;

mod duration;
mod moment;

#[derive(Clone, Copy)]
pub(in crate::core::eval) enum DateTimeOp {
    Now,
    Parse,
    ParseLocal,
    Format,
    Convert,
    Add,
    Subtract,
    Difference,
    Compare,
    ParseDuration,
    CreateDuration,
    Seconds,
    Nanoseconds,
    AddDurations,
    SubtractDurations,
    CompareDurations,
}

impl DateTimeOp {
    pub(super) fn signature(self) -> StatementSignature {
        let s = Kind::String;
        let (header, description, parameters, returns) = match self {
            Self::Now => ("Current Date Time In |zone|", "Read the wall clock once and return an RFC 3339 timestamp in the explicit zone.", vec![("zone", s)], s),
            Self::Parse => ("Parse Date Time |text|", "Parse explicit-offset RFC 3339 with up to nine fractional digits; return canonical UTC.", vec![("text", s)], s),
            Self::ParseLocal => ("Parse Date Time |text| Using |format| In |zone| Choosing |ambiguity|", "Parse a local calendar time using strftime fields and an explicit zone. Ambiguity is reject, earlier, or later; gaps fail.", vec![("text", s), ("format", s), ("zone", s), ("ambiguity", s)], s),
            Self::Format => ("Format Date Time |datetime| Using |format| In |zone|", "Format an RFC 3339 instant with strftime in an explicit zone, using English names.", vec![("datetime", s), ("format", s), ("zone", s)], s),
            Self::Convert => ("Convert Date Time |datetime| To |zone|", "Preserve the instant while returning RFC 3339 in the target zone; second-resolution offsets are rejected.", vec![("datetime", s), ("zone", s)], s),
            Self::Add => ("Add Duration |duration| To Date Time |datetime|", "Add an elapsed duration to an instant; return UTC. A day is exactly 86400 seconds.", vec![("duration", s), ("datetime", s)], s),
            Self::Subtract => ("Subtract Duration |duration| From Date Time |datetime|", "Subtract an elapsed duration from an instant; return UTC.", vec![("duration", s), ("datetime", s)], s),
            Self::Difference => ("Difference Between Date Times |left| And |right|", "Return the exact signed elapsed duration left minus right.", vec![("left", s), ("right", s)], s),
            Self::Compare => ("Compare Date Times |left| And |right|", "Compare instants, returning -1, 0, or 1 independent of displayed offsets.", vec![("left", s), ("right", s)], Kind::Int),
            Self::ParseDuration => ("Parse Duration |text|", "Normalize an exact signed ISO duration of weeks or days/hours/minutes/seconds; calendar months/years are invalid.", vec![("text", s)], s),
            Self::CreateDuration => ("Create Duration |amount| In |unit|", "Create an elapsed duration from an Int and weeks, days, hours, minutes, seconds, milliseconds, microseconds, or nanoseconds.", vec![("amount", Kind::Int), ("unit", s)], s),
            Self::Seconds => ("Duration Seconds |duration|", "Return exact signed decimal seconds as a String, retaining nanosecond precision.", vec![("duration", s)], s),
            Self::Nanoseconds => ("Duration Nanoseconds |duration|", "Return the exact signed integer nanosecond count as a String.", vec![("duration", s)], s),
            Self::AddDurations => ("Add Durations |left| And |right|", "Add two exact signed elapsed durations with checked overflow.", vec![("left", s), ("right", s)], s),
            Self::SubtractDurations => ("Subtract Durations |left| Minus |right|", "Subtract right from left with checked overflow.", vec![("left", s), ("right", s)], s),
            Self::CompareDurations => ("Compare Durations |left| And |right|", "Compare exact signed durations, returning -1, 0, or 1.", vec![("left", s), ("right", s)], Kind::Int),
        };
        let mut signature = StatementSignature::native_at("<datetime>", header)
            .expect("fixed date/time signature")
            .description(description)
            .returns(returns)
            .documents_error(Code::IncompatibleType, "Invalid timestamp, duration, format, zone, or local-time choice; nonexistent or unresolved repeated time.")
            .expect("fixed error")
            .documents_error(Code::Arithmetic, "Value exceeds the timestamp, named-zone, duration, or RFC 3339 offset range.")
            .expect("fixed error");
        for (name, kind) in parameters {
            signature = signature.parameter(name, kind).expect("fixed parameter");
        }
        signature
    }

    pub(super) fn invoke(
        self,
        arguments: Vec<TemporaryValue>,
        context: &Context,
    ) -> TemporaryResult {
        context.checkpoint()?;
        let text = |index: usize| match &*arguments[index] {
            Literal::String(text) => text.as_str(),
            _ => unreachable!("validated String"),
        };
        match self {
            Self::Now => {
                let zone = moment::Zone::parse(context, text(0))?;
                let now = moment::system_time(context, std::time::SystemTime::now())?;
                zone.render(context, now, None)
            }
            Self::Parse => moment::utc(context, moment::parse(context, text(0))?),
            Self::ParseLocal => moment::utc(
                context,
                moment::local(context, text(0), text(1), text(2), text(3))?,
            ),
            Self::Format | Self::Convert => {
                let instant = moment::parse(context, text(0))?;
                let (zone, format) = if matches!(self, Self::Format) {
                    (text(2), Some(text(1)))
                } else {
                    (text(1), None)
                };
                moment::Zone::parse(context, zone)?.render(context, instant, format)
            }
            Self::Add | Self::Subtract => {
                let delta = duration::parse(context, text(0))?;
                let instant = moment::nanos(moment::parse(context, text(1))?);
                let result = if matches!(self, Self::Add) {
                    instant.checked_add(delta)
                } else {
                    instant.checked_sub(delta)
                };
                moment::utc(
                    context,
                    moment::from_nanos(context, result.ok_or_else(|| overflow(context))?)?,
                )
            }
            Self::Difference | Self::Compare => {
                let left = moment::nanos(moment::parse(context, text(0))?);
                let right = moment::nanos(moment::parse(context, text(1))?);
                if matches!(self, Self::Compare) {
                    compare(context, left, right)
                } else {
                    duration::render(context, left - right)
                }
            }
            Self::CreateDuration => {
                let Literal::Int(amount) = *arguments[0] else {
                    unreachable!("validated Int")
                };
                duration::render(
                    context,
                    i128::from(amount) * duration::unit(context, text(1))?,
                )
            }
            Self::ParseDuration | Self::Seconds | Self::Nanoseconds => {
                let value = duration::parse(context, text(0))?;
                match self {
                    Self::Seconds => {
                        strings::formatted(context, |output| duration::seconds(output, value))
                    }
                    Self::Nanoseconds => {
                        strings::formatted(context, |output| write!(output, "{value}"))
                    }
                    _ => duration::render(context, value),
                }
            }
            Self::AddDurations | Self::SubtractDurations | Self::CompareDurations => {
                let left = duration::parse(context, text(0))?;
                let right = duration::parse(context, text(1))?;
                if matches!(self, Self::CompareDurations) {
                    return compare(context, left, right);
                }
                let result = if matches!(self, Self::AddDurations) {
                    left.checked_add(right)
                } else {
                    left.checked_sub(right)
                };
                duration::render(context, result.ok_or_else(|| overflow(context))?)
            }
        }
    }
}

fn compare(context: &Context, left: i128, right: i128) -> TemporaryResult {
    context.temporary(Literal::Int(match left.cmp(&right) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }))
}
fn invalid(context: &Context, reason: &str) -> RuntimeDiagnostic {
    context.detail_error(BWErr::OperationIncompatibleError, reason, None, false)
}
fn overflow(context: &Context) -> RuntimeDiagnostic {
    context.detail_error(
        BWErr::ArithmeticError,
        "Date/time or duration exceeds its supported range",
        None,
        false,
    )
}

lazy_static::lazy_static! {
    pub(super) static ref FIXED: Vec<(Builtin, Arc<StatementSignature>)> = [
        DateTimeOp::Now, DateTimeOp::Parse, DateTimeOp::ParseLocal, DateTimeOp::Format,
        DateTimeOp::Convert, DateTimeOp::Add, DateTimeOp::Subtract, DateTimeOp::Difference,
        DateTimeOp::Compare, DateTimeOp::ParseDuration, DateTimeOp::CreateDuration,
        DateTimeOp::Seconds, DateTimeOp::Nanoseconds, DateTimeOp::AddDurations,
        DateTimeOp::SubtractDurations, DateTimeOp::CompareDurations,
    ].into_iter().map(|kind| (Builtin::DateTime(kind), Arc::new(kind.signature()))).collect();
}
