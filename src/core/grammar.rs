use pest::pratt_parser::PrattParser;
use std::collections::HashMap;
use std::fmt;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub use super::parser::{BWParser, Rule};

lazy_static::lazy_static! {
    pub static ref PRATT_PARSER: PrattParser<Rule> = {
        use pest::pratt_parser::{Assoc::*, Op};
        use Rule::*;
        // Successive levels bind more tightly; unary pairs are evaluated separately.
        PrattParser::new()
            .op(Op::infix(logical_or, Left))
            .op(Op::infix(logical_and, Left))
            .op(Op::infix(equal, Left) | Op::infix(not_equal, Left))
            .op(
                Op::infix(less_than, Left)
                | Op::infix(less_than_or_equal, Left)
                | Op::infix(greater_than, Left)
                | Op::infix(greater_than_or_equal, Left)
            )
            .op(Op::infix(plus, Left) | Op::infix(minus, Left))
            .op(Op::infix(multiply, Left) | Op::infix(divide, Left) | Op::infix(modulus, Left))
            .op(Op::infix(exponent, Right))
    };
}

/// botwork Err
#[derive(Error, Debug, Clone)]
pub enum BWErr {
    #[error("Variable not defined: {0}")]
    VariableNotDefined(String),
    #[error("Statement not defined: {0}")]
    StatementNotDefined(String),
    #[error("Duplicate statement `{signature}` at {duplicate}; first defined at {original}")]
    DuplicateStatement {
        signature: String,
        original: String,
        duplicate: String,
    },
    #[error("Duplicate parameter `{name}` at {duplicate}; first declared at {original}")]
    DuplicateParameter {
        name: String,
        original: String,
        duplicate: String,
    },
    #[error("Parameter missing: {0}")]
    ParameterMissingError(String),
    #[error("Parsing error: {0}")]
    ParsingError(String),
    #[error("Invalid signature metadata: {0}")]
    SignatureError(String),
    #[error("Parsing number failed: {0}")]
    ParsingIntegerError(String),
    #[error("Operation performed on incompatible types: {0}")]
    OperationIncompatibleError(String),
    #[error("Invalid control flow: {0}")]
    ControlFlowError(String),
    #[error("Arithmetic error: {0}")]
    ArithmeticError(String),
    #[error("Collection access failed: {path} at `{segment}`: {reason}")]
    CollectionAccessError {
        path: String,
        segment: String,
        reason: String,
    },
    #[error("Writing output failed: {0}")]
    OutputError(String),
    #[error("Native operation failed: {0}")]
    NativeError(String),
    #[error("Native callback panicked: {0}")]
    NativePanic(String),
    #[error("Operation cancelled: {0}")]
    Cancelled(String),
    #[error("Operation timed out: {0}")]
    Timeout(String),
    #[error("Async runtime failure: {0}")]
    AsyncRuntime(String),
    #[error("Invalid input variables: {0}")]
    InputError(String),
    #[error("Invalid run configuration: {0}")]
    RunConfiguration(String),
    #[error("Reading source failed: {0}")]
    SourceRead(String),
    #[error("Resource limit exceeded: {resource} (limit {limit})")]
    ResourceLimit { resource: &'static str, limit: u64 },
    #[error("Loading module failed: {0}")]
    ImportRead(String),
    #[error("Import cycle: {0}")]
    ImportCycle(String),
    #[error("Duplicate namespace `{namespace}` at {duplicate}; first occupied at {original}")]
    DuplicateNamespace {
        namespace: String,
        original: String,
        duplicate: String,
    },
}

#[derive(Clone, Debug, Default)]
pub enum Literal {
    #[default]
    None,
    Int(i32),
    Float(f32),
    Bool(bool),
    String(String),
    Array(Vec<Literal>),
    Map(HashMap<String, Literal>),
}

struct CollectionValue<'a>(&'a Literal);

struct QuotedString<'a>(&'a str);

impl fmt::Display for QuotedString<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("\"")?;
        for character in self.0.chars() {
            // Preserve combining marks needed to read scripts such as Tamil.
            // Single quotes need no escape inside a double-quoted value.
            if character == '\'' || pest::unicode::MARK(character) {
                write!(formatter, "{character}")?;
            } else {
                write!(formatter, "{}", character.escape_debug())?;
            }
        }
        formatter.write_str("\"")
    }
}

impl fmt::Display for CollectionValue<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Literal::String(value) => write!(formatter, "{}", QuotedString(value)),
            value => write!(formatter, "{value}"),
        }
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("none"),
            Self::Int(value) => write!(formatter, "{value}"),
            Self::Float(value) => write!(formatter, "{value}"),
            Self::Bool(value) => write!(formatter, "{value}"),
            Self::String(value) => formatter.write_str(value),
            Self::Array(values) => {
                formatter.write_str("[")?;
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{}", CollectionValue(value))?;
                }
                formatter.write_str("]")
            }
            Self::Map(values) => {
                let mut entries: Vec<_> = values.iter().collect();
                entries.sort_unstable_by_key(|(key, _)| *key);
                formatter.write_str("{")?;
                for (index, (key, value)) in entries.into_iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(
                        formatter,
                        "{}: {}",
                        QuotedString(key),
                        CollectionValue(value)
                    )?;
                }
                formatter.write_str("}")
            }
        }
    }
}

type ORResult<O, E = BWErr> = Result<O, E>;
pub type LiteralResult = ORResult<Literal>;

fn checked_integer(value: Option<i32>, operation: &str) -> LiteralResult {
    value
        .map(Literal::Int)
        .ok_or_else(|| BWErr::ArithmeticError(format!("{operation} exceeds the i32 range")))
}

pub(super) fn finite_float(value: f32, operation: &str) -> LiteralResult {
    if value.is_finite() {
        Ok(Literal::Float(value))
    } else {
        Err(BWErr::ArithmeticError(format!(
            "{operation} produced a non-finite result"
        )))
    }
}

fn validate_numeric_operand(value: &Literal) -> Result<(), BWErr> {
    if matches!(value, Literal::Float(number) if !number.is_finite()) {
        return Err(BWErr::ArithmeticError(
            "Non-finite floating-point operand".into(),
        ));
    }
    Ok(())
}

fn numeric_pair(left: &Literal, right: &Literal) -> Option<(f64, f64)> {
    fn widen(value: &Literal) -> Option<f64> {
        match value {
            Literal::Int(value) => Some(f64::from(*value)),
            Literal::Float(value) => Some(f64::from(*value)),
            _ => None,
        }
    }
    // f64 represents every i32 and every finite f32 exactly. Comparisons must
    // not first round an integer to f32 as mixed arithmetic deliberately does.
    Some((widen(left)?, widen(right)?))
}

pub(crate) fn validate_value(value: &Literal) -> Result<(), BWErr> {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        validate_numeric_operand(value)?;
        match value {
            Literal::Array(values) => pending.extend(values),
            Literal::Map(values) => pending.extend(values.values()),
            _ => (),
        }
    }
    Ok(())
}

fn values_equal(left: &Literal, right: &Literal) -> Result<bool, BWErr> {
    // Validate complete operands before any shape/value mismatch can return false.
    validate_value(left)?;
    validate_value(right)?;

    // Use a work list rather than adding recursive comparison stack frames.
    let mut pairs = vec![(left, right)];
    while let Some((left, right)) = pairs.pop() {
        if let Some((left, right)) = numeric_pair(left, right) {
            if left != right {
                return Ok(false);
            }
            continue;
        }
        match (left, right) {
            (Literal::None, Literal::None) => (),
            (Literal::Bool(left), Literal::Bool(right)) if left == right => (),
            (Literal::String(left), Literal::String(right)) if left == right => (),
            (Literal::Array(left), Literal::Array(right)) if left.len() == right.len() => {
                pairs.extend(left.iter().zip(right));
            }
            (Literal::Map(left), Literal::Map(right)) if left.len() == right.len() => {
                for (key, left) in left {
                    let Some(right) = right.get(key) else {
                        return Ok(false);
                    };
                    pairs.push((left, right));
                }
            }
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn float_power(base: f64, exponent: i32) -> LiteralResult {
    if base == 0.0 && exponent < 0 {
        return Err(BWErr::ArithmeticError(
            "Zero cannot have a negative exponent".into(),
        ));
    }
    // Keep exponent parity exact, and invert first to preserve tiny reciprocals.
    // Wider intermediates are rounded to f32 once. At most 32 iterations run.
    let mut factor = base;
    if exponent < 0 {
        factor = 1.0 / factor;
    }
    let mut remaining = exponent.unsigned_abs();
    let mut result = 1.0;
    while remaining != 0 {
        if remaining & 1 != 0 {
            result *= factor;
        }
        remaining >>= 1;
        if remaining != 0 {
            factor *= factor;
        }
    }
    finite_float(result as f32, "Exponentiation")
}

pub trait Operate {
    fn operate_unary(&self, rhs: Literal) -> LiteralResult;
    fn operate_binary(&self, lhs: Literal, rhs: Literal) -> LiteralResult;
}

impl Operate for Rule {
    fn operate_binary(&self, lhs: Literal, rhs: Literal) -> LiteralResult {
        if matches!(self, Rule::equal | Rule::not_equal) {
            let are_equal = values_equal(&lhs, &rhs)?;
            return Ok(Literal::Bool(if *self == Rule::equal {
                are_equal
            } else {
                !are_equal
            }));
        }
        let err = format!("{:?} {:?} {:?}", lhs, self, rhs);
        let err = Err(BWErr::OperationIncompatibleError(err));
        use Literal::*;
        use Rule::*;
        let numeric_operands = match self {
            exponent => matches!((&lhs, &rhs), (Int(_) | Float(_), Int(_))),
            plus
            | minus
            | multiply
            | divide
            | modulus
            | less_than
            | less_than_or_equal
            | greater_than
            | greater_than_or_equal => {
                matches!((&lhs, &rhs), (Int(_) | Float(_), Int(_) | Float(_)))
            }
            _ => false,
        };
        if numeric_operands {
            validate_numeric_operand(&lhs)?;
            validate_numeric_operand(&rhs)?;
        }
        if matches!(self, divide | modulus)
            && matches!(lhs, Int(_) | Float(_))
            && match &rhs {
                Int(value) => *value == 0,
                Float(value) => *value == 0.0,
                _ => false,
            }
        {
            return Err(BWErr::ArithmeticError(format!("{self:?} by zero")));
        }
        match self {
            // Arithmetic operations
            multiply => match (lhs, rhs) {
                (Int(a), Int(b)) => checked_integer(a.checked_mul(b), "Multiplication"),
                (Float(a), Int(b)) => finite_float(a * b as f32, "Multiplication"),
                (Int(a), Float(b)) => finite_float(a as f32 * b, "Multiplication"),
                (Float(a), Float(b)) => finite_float(a * b, "Multiplication"),
                _ => err,
            },
            divide => match (lhs, rhs) {
                (Int(a), Int(b)) => finite_float(a as f32 / b as f32, "Division"),
                (Float(a), Int(b)) => finite_float(a / b as f32, "Division"),
                (Int(a), Float(b)) => finite_float(a as f32 / b, "Division"),
                (Float(a), Float(b)) => finite_float(a / b, "Division"),
                _ => err,
            },
            modulus => match (lhs, rhs) {
                (Int(i32::MIN), Int(-1)) => Ok(Int(0)),
                (Int(a), Int(b)) => checked_integer(a.checked_rem(b), "Remainder"),
                (Float(a), Int(b)) => finite_float(a % b as f32, "Remainder"),
                (Int(a), Float(b)) => finite_float(a as f32 % b, "Remainder"),
                (Float(a), Float(b)) => finite_float(a % b, "Remainder"),
                _ => err,
            },
            plus => match (lhs, rhs) {
                (Int(a), Int(b)) => checked_integer(a.checked_add(b), "Addition"),
                (Float(a), Int(b)) => finite_float(a + b as f32, "Addition"),
                (Int(a), Float(b)) => finite_float(a as f32 + b, "Addition"),
                (Float(a), Float(b)) => finite_float(a + b, "Addition"),
                (String(a), String(b)) => Ok(String(format!("{}{}", a, b))),
                (Array(a), Array(b)) => {
                    Ok(Array(a.iter().cloned().chain(b.iter().cloned()).collect()))
                }
                _ => err,
            },
            minus => match (lhs, rhs) {
                (Int(a), Int(b)) => checked_integer(a.checked_sub(b), "Subtraction"),
                (Float(a), Int(b)) => finite_float(a - b as f32, "Subtraction"),
                (Int(a), Float(b)) => finite_float(a as f32 - b, "Subtraction"),
                (Float(a), Float(b)) => finite_float(a - b, "Subtraction"),
                _ => err,
            },

            // Binary Operations
            less_than | less_than_or_equal | greater_than | greater_than_or_equal => {
                match numeric_pair(&lhs, &rhs) {
                    Some((left, right)) => Ok(Bool(match self {
                        less_than => left < right,
                        less_than_or_equal => left <= right,
                        greater_than => left > right,
                        _ => left >= right,
                    })),
                    Option::None => err,
                }
            }

            exponent => match (lhs, rhs) {
                (Int(a), Int(b)) if b >= 0 => {
                    checked_integer(a.checked_pow(b as u32), "Exponentiation")
                }
                (Int(a), Int(b)) => float_power(f64::from(a), b),
                (Float(a), Int(b)) => float_power(f64::from(a), b),
                _ => err,
            },
            logical_and => match (lhs, rhs) {
                (Bool(a), Bool(b)) => Ok(Bool(a && b)),
                _ => err,
            },
            logical_or => match (lhs, rhs) {
                (Bool(a), Bool(b)) => Ok(Bool(a || b)),
                _ => err,
            },
            _ => err,
        }
    }

    fn operate_unary(&self, rhs: Literal) -> LiteralResult {
        if matches!(self, Rule::minus) && matches!(rhs, Literal::Int(_) | Literal::Float(_)) {
            validate_numeric_operand(&rhs)?;
        }
        let err = format!("{:?} {:?}", self, rhs);
        let err = Err(BWErr::OperationIncompatibleError(err));
        match self {
            Rule::minus => match rhs {
                Literal::Int(a) => checked_integer(a.checked_neg(), "Negation"),
                Literal::Float(a) => finite_float(-a, "Negation"),
                _ => err,
            },
            Rule::logical_not => match rhs {
                Literal::Bool(a) => Ok(Literal::Bool(!a)),
                _ => err,
            },
            _ => err,
        }
    }
}
