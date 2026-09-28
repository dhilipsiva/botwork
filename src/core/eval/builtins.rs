//! Fixed standard statements. Results are admitted before owned copies/effects.
use super::*;
use crate::core::{diagnostic::DiagnosticCode as Code, signature::ValueKind as Kind};
use std::time::Duration;
mod collections;
mod datetime;
mod operating_system;
mod processes;
mod strings;

#[derive(Clone, Copy)]
pub(super) enum Builtin {
    Log,
    Assert,
    AssertEqual,
    Fail,
    VariableExists,
    GetVariable,
    TypeOf,
    NoOperation,
    Collection(collections::Collection),
    String(strings::StringOp),
    DateTime(datetime::DateTimeOp),
    OperatingSystem(operating_system::OsOp),
    Process(processes::ProcessOp),
}

impl Builtin {
    fn signature(self) -> StatementSignature {
        let (header, description, parameters, returns, error) = match self {
            Self::Collection(kind) => return kind.signature(),
            Self::String(kind) => return kind.signature(),
            Self::DateTime(kind) => return kind.signature(),
            Self::OperatingSystem(kind) => return kind.signature(),
            Self::Process(kind) => return kind.signature(),
            Self::Log => ("Log |value|", "Write the value to stdout followed by a newline; return that value.", vec![], None, Some((Code::Output, "The destination rejected output; some bytes may already be written."))),
            Self::Assert => ("Assert |condition|", "Require a Bool condition to be true; otherwise fail immediately. Return None.", vec![("condition", Kind::Bool)], Some(Kind::None), Some((Code::Assertion, "The condition was false."))),
            Self::AssertEqual => ("Assert |actual| Equals |expected|", "Require deep equality using the language's exact numeric comparison rules. Return None.", vec![], Some(Kind::None), Some((Code::Assertion, "Actual and expected values differ."))),
            Self::Fail => ("Fail |message|", "Fail immediately with a String reason; this is not an expected assertion failure.", vec![("message", Kind::String)], Some(Kind::None), Some((Code::ExplicitFailure, "The script explicitly failed."))),
            Self::VariableExists => ("Variable Exists |name|", "Return whether the exact variable name is bound in the caller's lexical scope, including None values.", vec![("name", Kind::String)], Some(Kind::Bool), None),
            Self::GetVariable => ("Get Variable |name|", "Return an admitted copy of a variable from the caller's lexical scope. Names are case sensitive.", vec![("name", Kind::String)], None, Some((Code::UndefinedVariable, "No visible binding has that exact name."))),
            Self::TypeOf => ("Type Of |value|", "Return the value kind: None, Int, Float, Bool, String, Array, or Map.", vec![], Some(Kind::String), None),
            Self::NoOperation => ("No Operation", "Do nothing and return None; execution and result budgets still apply.", vec![], Some(Kind::None), None),
        };
        let origin = if matches!(self, Self::Log) {
            "<builtin Log>"
        } else {
            "<builtin>"
        };
        let mut signature = StatementSignature::native_at(origin, header)
            .expect("fixed built-in signature")
            .description(description);
        for (name, kind) in parameters {
            signature = signature.parameter(name, kind).expect("fixed parameter");
        }
        if let Some(kind) = returns {
            signature = signature.returns(kind);
        }
        if let Some((code, reason)) = error {
            signature = signature
                .documents_error(code, reason)
                .expect("fixed error documentation");
        }
        signature
    }

    pub(super) fn invoke(
        self,
        values: Vec<TemporaryValue>,
        context: &mut Context,
    ) -> TemporaryResult {
        context.checkpoint()?;
        match self {
            Self::Collection(kind) => kind.invoke(values, context),
            Self::String(kind) => kind.invoke(values, context),
            Self::DateTime(kind) => kind.invoke(values, context),
            Self::OperatingSystem(kind) => kind.invoke(values, context),
            Self::Process(kind) => kind.invoke(&values, context),
            Self::Log => {
                let result = context.copy_temporary(&values[0])?;
                write_log(&values[0], &mut io::stdout().lock(), context)?;
                Ok(result)
            }
            Self::Assert => {
                if matches!(&*values[0], Literal::Bool(true)) {
                    context.temporary(Literal::None)
                } else {
                    Err(context.detail_error(
                        BWErr::AssertionFailed,
                        "Expected true, got false",
                        None,
                        false,
                    ))
                }
            }
            Self::AssertEqual => {
                let equal = super::super::grammar::values_equal(&values[0], &values[1]).map_err(
                    |error| {
                        context.formatted_error(
                            BWErr::ArithmeticError,
                            format_args!("{error}"),
                            None,
                            false,
                        )
                    },
                )?;
                if equal {
                    context.temporary(Literal::None)
                } else {
                    Err(context.formatted_error(
                        BWErr::AssertionFailed,
                        format_args!(
                            "Expected {} ({}), got {} ({})",
                            *values[1],
                            values[1].kind().as_str(),
                            *values[0],
                            values[0].kind().as_str()
                        ),
                        None,
                        false,
                    ))
                }
            }
            Self::Fail => {
                let Literal::String(message) = &*values[0] else {
                    unreachable!("validated String")
                };
                Err(context.detail_error(BWErr::ExplicitFailure, message, None, false))
            }
            Self::VariableExists | Self::GetVariable => {
                let Literal::String(name) = &*values[0] else {
                    unreachable!("validated String")
                };
                crate::core::input::validate_name_with("variable inspection", name, |message| {
                    context.formatted_error(BWErr::OperationIncompatibleError, message, None, false)
                })?;
                if matches!(self, Self::VariableExists) {
                    context.temporary(Literal::Bool(context.find_variable_binding(name).is_some()))
                } else {
                    context.copy_temporary(&context.get_variable_binding(name, None)?.value)
                }
            }
            Self::TypeOf => context.temporary_string(values[0].kind().as_str()),
            Self::NoOperation => context.temporary(Literal::None),
        }
    }

    pub(super) fn needs_worker(self) -> bool {
        matches!(self, Self::Log | Self::Process(_))
            || matches!(self, Self::String(kind) if kind.is_regex())
            || matches!(self, Self::OperatingSystem(kind) if kind.needs_worker())
    }

    pub(super) fn needs_directory(self) -> bool {
        matches!(self, Self::OperatingSystem(kind) if kind.needs_worker())
    }
}

lazy_static::lazy_static! {
    static ref FIXED: Vec<(Builtin, Arc<StatementSignature>)> = [
        Builtin::Log, Builtin::Assert, Builtin::AssertEqual, Builtin::Fail,
        Builtin::VariableExists, Builtin::GetVariable, Builtin::TypeOf, Builtin::NoOperation,
    ].into_iter().map(|kind| (kind, Arc::new(kind.signature()))).collect();
    static ref SLEEP: StatementSignature = StatementSignature::native_at("<builtin>", "Sleep |milliseconds|")
        .expect("fixed sleep signature")
        .parameter("milliseconds", Kind::Int).expect("fixed parameter")
        .returns(Kind::None)
        .description("Cooperatively await a nonnegative Int number of milliseconds; requires asynchronous execution. Return None.")
        .documents_error(Code::IncompatibleType, "Milliseconds must be nonnegative.").expect("fixed error documentation");
}

pub(super) fn initialize(context: &mut Context) {
    // This finite fixed catalogue is outside user registry admission, preserving
    // infallible initialization under zero registry budgets. Never replace a host
    // registration already occupying a slot in this frame.
    for (kind, metadata) in FIXED
        .iter()
        .chain(collections::FIXED.iter())
        .chain(strings::FIXED.iter())
        .chain(datetime::FIXED.iter())
        .chain(operating_system::FIXED.iter())
        .chain(processes::FIXED.iter())
    {
        if !context.frames[context.current]
            .statements
            .contains_key(metadata.normalized())
        {
            context
                .insert_statement(
                    metadata.normalized(),
                    metadata.header(),
                    StmtType::Native {
                        body: NativeBody::Builtin(*kind),
                        metadata: Arc::clone(metadata),
                        _registry: None,
                    },
                )
                .expect("vacant fixed built-in slot");
        }
    }
    if !context.frames[context.current]
        .statements
        .contains_key(SLEEP.normalized())
    {
        let operation = NativeOperation::asynchronous(SLEEP.clone(), |values, _| async move {
            let Literal::Int(milliseconds) = values[0] else {
                unreachable!("validated Int")
            };
            let milliseconds = u64::try_from(milliseconds).map_err(|_| {
                Diagnostic::new(BWErr::OperationIncompatibleError(
                    "Sleep milliseconds must be nonnegative".into(),
                ))
            })?;
            tokio::time::sleep(Duration::from_millis(milliseconds)).await;
            Ok(Literal::None)
        })
        .expect("fixed asynchronous sleep");
        context
            .insert_statement(
                SLEEP.normalized(),
                SLEEP.header(),
                StmtType::Operation {
                    operation,
                    builtin: true,
                    _registry: None,
                },
            )
            .expect("vacant fixed sleep slot");
    }
}
