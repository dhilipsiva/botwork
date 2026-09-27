use super::*;
use crate::core::{
    run::{TemporaryReservation, TemporaryValue},
    value_limits::ValueSize,
};

impl Context {
    pub(super) fn temporary_reservation(
        &self,
        size: ValueSize,
    ) -> DiagnosticResult<Option<TemporaryReservation>> {
        self.budget
            .as_ref()
            .map(|budget| budget.reserve_temporary(size))
            .transpose()
    }

    pub(super) fn temporary(&self, value: Literal) -> DiagnosticResult<TemporaryValue> {
        let value = Owned::new(value);
        self.checkpoint()?;
        let size = self
            .limits()
            .values
            .check(&value)
            .map_err(|error| self.retain_limit(Diagnostic::new(error)))?;
        let reservation = self.temporary_reservation(size)?;
        Ok(TemporaryValue::new(value.into_inner(), reservation))
    }

    pub(super) fn copy_temporary(&self, value: &Literal) -> DiagnosticResult<TemporaryValue> {
        self.checkpoint()?;
        let size = self
            .limits()
            .values
            .check(value)
            .map_err(|error| self.retain_limit(Diagnostic::new(error)))?;
        let reservation = self.temporary_reservation(size)?;
        Ok(TemporaryValue::new(value.clone(), reservation))
    }

    pub(super) fn temporary_string(&self, value: &str) -> DiagnosticResult<TemporaryValue> {
        let size = self
            .limits()
            .values
            .string_size(value.len())
            .map_err(|error| self.retain_limit(Diagnostic::new(error)))?;
        let reservation = self.temporary_reservation(size)?;
        Ok(TemporaryValue::new(
            Literal::String(value.into()),
            reservation,
        ))
    }

    pub(super) fn temporary_binary(
        &self,
        operator: BinaryOp,
        left: TemporaryValue,
        right: TemporaryValue,
        span: &Span,
    ) -> DiagnosticResult<TemporaryValue> {
        let limits = self.limits().values;
        let left_size = limits
            .check(&left)
            .map_err(|error| self.retain_limit(Diagnostic::new(error)))?;
        let right_size = limits
            .check(&right)
            .map_err(|error| self.retain_limit(Diagnostic::new(error)))?;
        let output = if operator == BinaryOp::Add {
            limits
                .check_concatenation(&left, &right, left_size, right_size)
                .map_err(|error| self.retain_limit(Diagnostic::new(error)))?;
            match (&*left, &*right) {
                (Literal::String(_), Literal::String(_)) => Some(ValueSize {
                    nodes: 1,
                    depth: 1,
                    payload_bytes: left_size.payload_bytes + right_size.payload_bytes,
                }),
                (Literal::Array(_), Literal::Array(_)) => Some(ValueSize {
                    nodes: left_size.nodes + (right_size.nodes - 1),
                    depth: left_size.depth.max(right_size.depth),
                    payload_bytes: left_size.payload_bytes + right_size.payload_bytes,
                }),
                _ => None,
            }
        } else {
            None
        };
        let reserved = output
            .map(|size| self.temporary_reservation(size))
            .transpose()?;
        let (left, _left_reservation) = left.into_parts();
        let (right, _right_reservation) = right.into_parts();
        let result = operator
            .to_rule()
            .operate_binary_with_error(left, right, &limits, |category, message| {
                self.formatted_error(category, message, Some(span), true)
            })
            .map_err(|error| self.retain_limit(error))?;
        match reserved {
            Some(reservation) => Ok(TemporaryValue::new(result, reservation)),
            None => self.temporary(result),
        }
    }

    pub(super) fn temporary_unary(
        &self,
        operator: UnaryOp,
        operand: TemporaryValue,
        span: &Span,
    ) -> DiagnosticResult<TemporaryValue> {
        let (value, _reservation) = operand.into_parts();
        let result = operator
            .to_rule()
            .operate_unary_with_error(value, &self.limits().values, |category, message| {
                self.formatted_error(category, message, Some(span), true)
            })
            .map_err(|error| self.retain_limit(error))?;
        self.temporary(result)
    }
}

pub(super) struct TemporaryArguments {
    // Destroy argument values before releasing their reservations.
    values: Vec<Literal>,
    _reservations: Vec<Option<TemporaryReservation>>,
}

impl TemporaryArguments {
    pub(super) fn new(arguments: Vec<TemporaryValue>) -> Self {
        let (values, reservations) = arguments
            .into_iter()
            .map(TemporaryValue::into_parts)
            .unzip();
        Self {
            values,
            _reservations: reservations,
        }
    }
}

impl std::ops::Deref for TemporaryArguments {
    type Target = [Literal];
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}
