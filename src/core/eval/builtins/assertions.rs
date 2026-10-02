//! Borrowed assertion evidence: bounded previews, full operands admitted before copying.
use super::data::JsonString;
use super::*;
use crate::core::{
    diagnostic::FormattedDetail, grammar::QuotedString, value_limits::MAX_VALUE_DEPTH,
};
use std::fmt::{self, Write};

const PREVIEW_BYTES: usize = 256;
const TRUNCATED: &str = "…[truncated]";

pub(super) fn failure(
    actual: &Literal,
    expected: &Literal,
    condition: bool,
    context: &Context,
) -> RuntimeDiagnostic {
    // The reason and previews use masked copies, so neither a cut preview nor a
    // character-level difference reveals a secret; the full typed operands stay
    // exact for hosts, and CLI artifacts mask them.
    let secrets = context.secrets();
    let masked_actual = secrets.and_then(|secrets| secrets.mask_value(actual));
    let masked_expected = secrets.and_then(|secrets| secrets.mask_value(expected));
    let (full_actual, full_expected) = (actual, expected);
    let actual = masked_actual.as_ref().unwrap_or(actual);
    let expected = masked_expected.as_ref().unwrap_or(expected);
    let reason = Reason {
        actual,
        expected,
        condition,
        difference: difference(actual, expected),
    };
    context.constructed_fields(
        |[reason, actual, expected]| BWErr::AssertionMismatch {
            reason,
            actual,
            expected,
        },
        [
            FormattedDetail::exact(format_args!("{reason}")),
            FormattedDetail {
                full: format_args!("{}", Typed(full_actual)),
                summary: Some(format_args!("{}", Preview(Human(actual)))),
            },
            FormattedDetail {
                full: format_args!("{}", Typed(full_expected)),
                summary: Some(format_args!("{}", Preview(Human(expected)))),
            },
        ],
        None,
        None,
    )
}

/// A value as an assertion's typed operand, for adapters whose own
/// assertions compare values.
pub(in crate::core::eval) fn typed(value: &Literal) -> String {
    Typed(value).to_string()
}

/// No width-sized sorting buffers are allocated while measuring or rendering.
struct Human<'a>(&'a Literal);
impl fmt::Display for Human<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Literal::String(value) => write!(f, "{}", QuotedString(value)),
            Literal::Array(values) => {
                f.write_str("[")?;
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", Human(value))?;
                }
                f.write_str("]")
            }
            Literal::Map(values) => {
                f.write_str("{")?;
                let mut previous: Option<&str> = None;
                while let Some((key, value)) = values
                    .iter()
                    .filter(|(key, _)| previous.is_none_or(|old| key.as_str() > old))
                    .min_by_key(|(key, _)| *key)
                {
                    if previous.is_some() {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}: {}", QuotedString(key), Human(value))?;
                    previous = Some(key);
                }
                f.write_str("}")
            }
            value => write!(f, "{value}"),
        }
    }
}

struct Capped<'a, W> {
    output: &'a mut W,
    remaining: usize,
    truncated: bool,
}
impl<W: Write> Write for Capped<'_, W> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if text.len() <= self.remaining {
            self.output.write_str(text)?;
            self.remaining -= text.len();
            return Ok(());
        }
        let mut end = self.remaining;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.output.write_str(&text[..end])?;
        self.truncated = true;
        Err(fmt::Error)
    }
}
struct Preview<T>(T);
impl<T: fmt::Display> fmt::Display for Preview<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut output = Capped {
            output: f,
            remaining: PREVIEW_BYTES,
            truncated: false,
        };
        let result = write!(output, "{}", self.0);
        if output.truncated {
            output.output.write_str(TRUNCATED)
        } else {
            result
        }
    }
}

/// Every node retains its kind, including nested Int/Float values and None.
struct Typed<'a>(&'a Literal);
impl fmt::Display for Typed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{\"kind\":\"{}\",\"value\":", self.0.kind().as_str())?;
        match self.0 {
            Literal::None => f.write_str("null")?,
            Literal::Int(value) => write!(f, "{value}")?,
            Literal::Float(value) => write!(f, "{value:?}")?,
            Literal::Bool(value) => write!(f, "{value}")?,
            Literal::String(value) => write!(f, "{}", JsonString(value))?,
            Literal::Array(values) => {
                f.write_char('[')?;
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        f.write_char(',')?;
                    }
                    write!(f, "{}", Typed(value))?;
                }
                f.write_char(']')?;
            }
            Literal::Map(values) => {
                f.write_char('{')?;
                // Object order is immaterial; previews and difference selection are sorted.
                for (index, (key, value)) in values.iter().enumerate() {
                    if index != 0 {
                        f.write_char(',')?;
                    }
                    write!(f, "{}:{}", JsonString(key), Typed(value))?;
                }
                f.write_char('}')?;
            }
        }
        f.write_char('}')
    }
}

// The ordinary equality check already validated all numeric inputs. This borrowed
// walk locates evidence without the comparison engine's width-sized work list.
fn same(left: &Literal, right: &Literal) -> bool {
    match (left, right) {
        (Literal::Int(a), Literal::Int(b)) => a == b,
        (Literal::Float(a), Literal::Float(b)) => a == b,
        (Literal::Int(a), Literal::Float(b)) => f64::from(*a) == f64::from(*b),
        (Literal::Float(a), Literal::Int(b)) => f64::from(*a) == f64::from(*b),
        (Literal::None, Literal::None) => true,
        (Literal::Bool(a), Literal::Bool(b)) => a == b,
        (Literal::String(a), Literal::String(b)) => a == b,
        (Literal::Array(a), Literal::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Literal::Map(a), Literal::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| same(a, b)))
        }
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum Segment<'a> {
    Key(&'a str),
    Index(usize),
}
struct Difference<'a> {
    path: [Option<Segment<'a>>; MAX_VALUE_DEPTH],
    depth: usize,
    actual: Option<&'a Literal>,
    expected: Option<&'a Literal>,
    lengths: Option<(usize, usize)>,
}
fn difference<'a>(actual: &'a Literal, expected: &'a Literal) -> Difference<'a> {
    let mut result = Difference {
        path: [None; MAX_VALUE_DEPTH],
        depth: 0,
        actual: Some(actual),
        expected: Some(expected),
        lengths: None,
    };
    while result.depth < MAX_VALUE_DEPTH {
        let (actual, expected) = match (result.actual, result.expected) {
            (Some(actual), Some(expected)) => (actual, expected),
            _ => break,
        };
        let (segment, actual, expected) = match (actual, expected) {
            (Literal::Array(a), Literal::Array(b)) => {
                let index = a.iter().zip(b).position(|(a, b)| !same(a, b));
                let index = match index {
                    Some(index) => index,
                    None if a.len() != b.len() => {
                        result.lengths = Some((a.len(), b.len()));
                        a.len().min(b.len())
                    }
                    None => break,
                };
                (Segment::Index(index), a.get(index), b.get(index))
            }
            (Literal::Map(a), Literal::Map(b)) => {
                let key = a
                    .keys()
                    .chain(b.keys())
                    .filter(|key| match (a.get(*key), b.get(*key)) {
                        (Some(a), Some(b)) => !same(a, b),
                        _ => true,
                    })
                    .min();
                let Some(key) = key else {
                    break;
                };
                (Segment::Key(key), a.get(key), b.get(key))
            }
            _ => break,
        };
        result.path[result.depth] = Some(segment);
        result.depth += 1;
        result.actual = actual;
        result.expected = expected;
    }
    result
}
struct DifferencePath<'a, 'b>(&'a Difference<'b>);
impl fmt::Display for DifferencePath<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('$')?;
        for segment in self.0.path[..self.0.depth].iter().flatten() {
            match segment {
                Segment::Key(key) => write!(f, "[{}]", QuotedString(key))?,
                Segment::Index(index) => write!(f, "[{index}]")?,
            }
        }
        Ok(())
    }
}
struct Observed<'a>(Option<&'a Literal>);
impl fmt::Display for Observed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(value) => write!(f, "{} ({})", Preview(Human(value)), value.kind().as_str()),
            None => f.write_str("<missing>"),
        }
    }
}
struct Scalar(Option<char>);
impl fmt::Display for Scalar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(ch) => write!(f, "'{}' (U+{:04X})", ch.escape_debug(), ch as u32),
            None => f.write_str("<end of string>"),
        }
    }
}
struct Reason<'a> {
    actual: &'a Literal,
    expected: &'a Literal,
    condition: bool,
    difference: Difference<'a>,
}
impl fmt::Display for Reason<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.condition {
            return f.write_str("Expected true, got false");
        }
        write!(
            f,
            "Expected {}, got {}",
            Observed(Some(self.expected)),
            Observed(Some(self.actual))
        )?;
        let difference = &self.difference;
        write!(
            f,
            "\n  difference at {}: expected {}, got {}",
            Preview(DifferencePath(difference)),
            Observed(difference.expected),
            Observed(difference.actual)
        )?;
        if let Some((actual, expected)) = difference.lengths {
            write!(f, "; array lengths: expected {expected}, got {actual}")?;
        }
        if let (Some(Literal::String(actual)), Some(Literal::String(expected))) =
            (difference.actual, difference.expected)
        {
            let mut a = actual.chars();
            let mut b = expected.chars();
            let mut index = 0;
            loop {
                let pair = (a.next(), b.next());
                if pair.0 != pair.1 {
                    write!(
                        f,
                        "; Unicode scalar index {index}: expected {}, got {}",
                        Scalar(pair.1),
                        Scalar(pair.0)
                    )?;
                    break;
                }
                if pair.0.is_none() {
                    break;
                }
                index += 1;
            }
        }
        f.write_str("\n  full operands: details.expected / details.actual (typed JSON)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_comparison_matches_language_equality_across_kinds_and_boundaries() {
        let mut values = vec![
            Literal::None,
            Literal::Bool(false),
            Literal::Bool(true),
            Literal::String(String::new()),
            Literal::String("é".into()),
        ];
        values.extend([i32::MIN, -1, 0, 1, 16777217, i32::MAX].map(Literal::Int));
        values.extend(
            [
                i32::MIN as f32,
                -0.0,
                0.0,
                1.0,
                16777216.0,
                i32::MAX as f32,
                f32::MIN_POSITIVE,
                f32::MAX,
            ]
            .map(Literal::Float),
        );
        let leaves = values.clone();
        values.extend(
            leaves
                .iter()
                .map(|value| Literal::Array(vec![value.clone()])),
        );
        values.extend(
            leaves
                .iter()
                .map(|value| Literal::Map([("key".into(), value.clone())].into())),
        );
        values.push(Literal::Array(Vec::new()));
        values.push(Literal::Map(Default::default()));
        for actual in &values {
            for expected in &values {
                assert_eq!(
                    same(actual, expected),
                    crate::core::grammar::values_equal(actual, expected).unwrap(),
                    "{actual:?} / {expected:?}"
                );
            }
        }
    }

    #[test]
    fn preview_propagates_destination_errors_instead_of_mislabeling_them_as_truncation() {
        struct Reject;
        impl Write for Reject {
            fn write_str(&mut self, _: &str) -> fmt::Result {
                Err(fmt::Error)
            }
        }
        assert!(write!(Reject, "{}", Preview(Human(&Literal::Int(1)))).is_err());
    }
}
