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
        PrattParser::new()
            .op(Op::infix(plus, Left) | Op::infix(minus, Left) | Op::infix(logical_or, Left) )
            .op(Op::infix(multiply, Left) | Op::infix(divide, Left) | Op::infix(modulus, Left) |Op::infix(logical_and, Left) )
            .op(Op::prefix(minus) | Op::prefix(logical_not))
            .op(
            Op::infix(less_than, Left)
            | Op::infix(less_than_or_equal, Left)
            | Op::infix(greater_than, Left)
            | Op::infix(greater_than_or_equal, Left)
            | Op::infix(not_equal, Left)
            | Op::infix(equal, Left)
            | Op::infix(exponent, Left)
        )
    };
}

/// botwork Err
#[derive(Error, Debug)]
pub enum BWErr {
    #[error("Variable not defined: {0}")]
    VariableNotDefined(String),
    #[error("Statement not defined: {0}")]
    StatementNotDefined(String),
    #[error("Parameter missing: {0}")]
    ParameterMissingError(String),
    #[error("Parsing error: {0}")]
    ParsingError(String),
    #[error("Parsing number failed: {0}")]
    ParsingIntegerError(String),
    #[error("Operation performed on incompatible types: {0}")]
    OperationIncompatibleError(String),
    #[error("Writing output failed: {0}")]
    OutputError(String),
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

impl fmt::Display for CollectionValue<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Literal::String(value) => write!(formatter, "{value:?}"),
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
                    write!(formatter, "{key:?}: {}", CollectionValue(value))?;
                }
                formatter.write_str("}")
            }
        }
    }
}

type ORResult<O, E = BWErr> = Result<O, E>;
pub type LiteralResult = ORResult<Literal>;

pub trait Operate {
    fn operate_unary(&self, rhs: Literal) -> LiteralResult;
    fn operate_binary(&self, lhs: Literal, rhs: Literal) -> LiteralResult;
}

impl Operate for Rule {
    fn operate_binary(&self, lhs: Literal, rhs: Literal) -> LiteralResult {
        let err = format!("{:?} {:?} {:?}", lhs, self, rhs);
        let err = Err(BWErr::OperationIncompatibleError(err));
        use Literal::*;
        use Rule::*;
        match self {
            // Arithmatic Operations
            multiply => match (lhs, rhs) {
                (Int(a), Int(b)) => Ok(Int(a * b)),
                (Float(a), Int(b)) => Ok(Float(a * b as f32)),
                (Int(a), Float(b)) => Ok(Float(a as f32 * b)),
                (Float(a), Float(b)) => Ok(Float(a * b)),
                _ => err,
            },
            divide => match (lhs, rhs) {
                (Int(a), Int(b)) => Ok(Float(a as f32 / b as f32)),
                (Float(a), Int(b)) => Ok(Float(a / b as f32)),
                (Int(a), Float(b)) => Ok(Float(a as f32 / b)),
                (Float(a), Float(b)) => Ok(Float(a / b)),
                _ => err,
            },
            modulus => match (lhs, rhs) {
                (Int(a), Int(b)) => Ok(Int(a % b)),
                (Float(a), Int(b)) => Ok(Float(a % b as f32)),
                (Int(a), Float(b)) => Ok(Float(a as f32 % b)),
                (Float(a), Float(b)) => Ok(Float(a % b)),
                _ => err,
            },
            plus => match (lhs, rhs) {
                (Int(a), Int(b)) => Ok(Int(a + b)),
                (Float(a), Int(b)) => Ok(Float(a + b as f32)),
                (Int(a), Float(b)) => Ok(Float(a as f32 + b)),
                (Float(a), Float(b)) => Ok(Float(a + b)),
                (String(a), String(b)) => Ok(String(format!("{}{}", a, b))),
                (Array(a), Array(b)) => {
                    Ok(Array(a.iter().cloned().chain(b.iter().cloned()).collect()))
                }
                _ => err,
            },
            minus => match (lhs, rhs) {
                (Int(a), Int(b)) => Ok(Int(a - b)),
                (Float(a), Int(b)) => Ok(Float(a - b as f32)),
                (Int(a), Float(b)) => Ok(Float(a as f32 - b)),
                (Float(a), Float(b)) => Ok(Float(a - b)),
                _ => err,
            },

            // Binary Operations
            less_than => match (lhs, rhs) {
                (Int(a), Int(b)) => Ok(Bool(a < b)),
                (Float(a), Int(b)) => Ok(Bool(a < b as f32)),
                (Int(a), Float(b)) => Ok(Bool((a as f32) < b)),
                (Float(a), Float(b)) => Ok(Bool(a < b)),
                _ => err,
            },
            less_than_or_equal => match (lhs, rhs) {
                (Int(a), Int(b)) => Ok(Bool(a <= b)),
                (Float(a), Int(b)) => Ok(Bool(a <= b as f32)),
                (Int(a), Float(b)) => Ok(Bool((a as f32) <= b)),
                (Float(a), Float(b)) => Ok(Bool(a <= b)),
                _ => err,
            },
            greater_than => match (lhs, rhs) {
                (Int(a), Int(b)) => Ok(Bool(a > b)),
                (Float(a), Int(b)) => Ok(Bool(a > b as f32)),
                (Int(a), Float(b)) => Ok(Bool((a as f32) > b)),
                (Float(a), Float(b)) => Ok(Bool(a > b)),
                _ => err,
            },
            greater_than_or_equal => match (lhs, rhs) {
                (Int(a), Int(b)) => Ok(Bool(a >= b)),
                (Float(a), Int(b)) => Ok(Bool(a >= b as f32)),
                (Int(a), Float(b)) => Ok(Bool((a as f32) >= b)),
                (Float(a), Float(b)) => Ok(Bool(a >= b)),
                _ => err,
            },
            not_equal => match (lhs, rhs) {
                (Int(a), Int(b)) => Ok(Bool(a != b)),
                (Float(a), Int(b)) => Ok(Bool(a != b as f32)),
                (Int(a), Float(b)) => Ok(Bool((a as f32) != b)),
                (Float(a), Float(b)) => Ok(Bool(a != b)),
                (Bool(a), Bool(b)) => Ok(Bool(a != b)),
                (String(a), String(b)) => Ok(Bool(a != b)),
                _ => err,
            },
            equal => match (lhs, rhs) {
                (Int(a), Int(b)) => Ok(Bool(a == b)),
                (Float(a), Int(b)) => Ok(Bool(a == b as f32)),
                (Int(a), Float(b)) => Ok(Bool((a as f32) == b)),
                (Float(a), Float(b)) => Ok(Bool(a == b)),
                (Bool(a), Bool(b)) => Ok(Bool(a == b)),
                (String(a), String(b)) => Ok(Bool(a == b)),
                _ => err,
            },

            exponent => match (lhs, rhs) {
                (Int(a), Int(b)) => Ok(Int(a.pow(b as u32))),
                (Float(a), Int(b)) => Ok(Float(a.powf(b as f32))),
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
        let err = format!("{:?} {:?}", self, rhs);
        let err = Err(BWErr::OperationIncompatibleError(err));
        match self {
            Rule::minus => match rhs {
                Literal::Int(a) => Ok(Literal::Int(-a)),
                Literal::Float(a) => Ok(Literal::Float(-a)),
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
