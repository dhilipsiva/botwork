use pest::pratt_parser::PrattParser;
use pest_derive::Parser;
use std::collections::HashMap;
use thiserror::Error;

#[cfg(test)]
mod tests;

#[derive(Parser)]
#[grammar = "src/core/grammar.pest"]
pub struct BWParser;

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
    #[error("Variable not defined")]
    VariableNotDefined(String),
    #[error("Statement not defined")]
    StatementNotDefined(String),
    #[error("Parameter missing error")]
    ParameterMissingError(String),
    #[error("Parsing integer error")]
    ParsingError(String),
    #[error("Parsing integer error")]
    ParsingIntegerError(String),
    #[error("Operation performed on incompatible types")]
    OperationIncompatibleError(String),
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
