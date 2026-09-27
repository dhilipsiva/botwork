//! Reservations accompany interpreter-created details before operation handoff.
use super::*;
use crate::core::diagnostic::{DiagnosticConstruction, FormattedDetail};

/// Dispose payloads before refunding ownership, including failed construction.
pub(super) struct PendingDiagnostic {
    pub(super) value: OwnedDiagnostic,
    pub(super) reservation: Option<Reservation>,
}

impl std::ops::Deref for PendingDiagnostic {
    type Target = Diagnostic;
    fn deref(&self) -> &Diagnostic {
        self.value.as_ref()
    }
}
impl From<Diagnostic> for PendingDiagnostic {
    fn from(value: Diagnostic) -> Self {
        Self {
            value: OwnedDiagnostic::new(value),
            reservation: None,
        }
    }
}

impl NativeOperation {
    pub(super) fn track_error(
        &self,
        error: impl Into<PendingDiagnostic>,
        preserve_stop: bool,
        admitted: bool,
        scope: &Option<Arc<Reservation>>,
    ) -> Box<Tracked<OwnedDiagnostic>> {
        let PendingDiagnostic {
            value,
            reservation: previous,
        } = error.into();
        if admitted && value.as_ref().is_emergency() {
            drop(previous);
            return tracked(value, None, scope);
        }
        // A host-shaped summary still passes measurement. Match public at()'s
        // no-attachment semantics without treating its shape as trusted admission.
        let location = (!value.as_ref().is_emergency()).then_some((self.signature.header(), false));
        let admission = self.diagnostic_limits.retained_runtime_size(
            value.as_ref(),
            std::iter::empty(),
            location,
            None,
            None,
        );
        let mut previous = previous;
        let admitted = admission.and_then(|(size, _)| {
            self.ownership
                .diagnostic(size, previous.take())
                .map_err(|rejection| {
                    let (violation, retained) = *rejection;
                    previous = retained;
                    violation
                })
        });
        match admitted {
            Ok(reservation) => tracked(
                OwnedDiagnostic::new(
                    value
                        .into_inner()
                        .capture_context(location, std::iter::empty()),
                ),
                Some(reservation),
                scope,
            ),
            Err(violation) => {
                let error = value.into_inner().rejected_context(violation, location, 0);
                drop(previous); // Rejected full payloads are already disposed.
                tracked(
                    OwnedDiagnostic::new(if preserve_stop {
                        preserve_primary(error)
                    } else {
                        error
                    }),
                    None,
                    scope,
                )
            }
        }
    }

    pub(super) fn attach_stop(
        &self,
        stopped: Diagnostic,
        error: Box<Tracked<OwnedDiagnostic>>,
        preserve_stop: bool,
        scope: &Option<Arc<Reservation>>,
    ) -> Box<Tracked<OwnedDiagnostic>> {
        let Tracked {
            value, reservation, ..
        } = *error;
        self.combine_pending(
            stopped.into(),
            PendingDiagnostic { value, reservation },
            preserve_stop,
            scope,
        )
    }

    pub(super) fn combine_errors(
        &self,
        primary: Box<Tracked<OwnedDiagnostic>>,
        cause: Box<Tracked<OwnedDiagnostic>>,
        preserve_stop: bool,
        scope: &Option<Arc<Reservation>>,
    ) -> Box<Tracked<OwnedDiagnostic>> {
        let Tracked {
            value, reservation, ..
        } = *primary;
        let primary = PendingDiagnostic { value, reservation };
        let Tracked {
            value, reservation, ..
        } = *cause;
        self.combine_pending(
            primary,
            PendingDiagnostic { value, reservation },
            preserve_stop,
            scope,
        )
    }

    fn combine_pending(
        &self,
        primary: PendingDiagnostic,
        cause: PendingDiagnostic,
        preserve_stop: bool,
        scope: &Option<Arc<Reservation>>,
    ) -> Box<Tracked<OwnedDiagnostic>> {
        if Arc::ptr_eq(&primary.error, &cause.error) {
            drop(cause);
            return self.track_error(primary, preserve_stop, true, scope);
        }
        if primary.is_emergency() {
            drop(cause);
            let PendingDiagnostic { value, reservation } = primary;
            let value = OwnedDiagnostic::new(value.into_inner().omit_handled_cause());
            drop(reservation);
            return tracked(value, None, scope);
        }
        let location = Some((self.signature.header(), false));
        let admission = self.diagnostic_limits.retained_runtime_size(
            &primary,
            std::iter::empty(),
            location,
            None,
            Some(&cause),
        );
        let PendingDiagnostic {
            value: primary,
            reservation: primary_lease,
        } = primary;
        let PendingDiagnostic {
            value: cause,
            reservation: cause_lease,
        } = cause;
        let mut previous = Reservation::merge(primary_lease, cause_lease);
        let admitted = admission.and_then(|(size, _)| {
            self.ownership
                .diagnostic(size, previous.take())
                .map_err(|rejection| {
                    let (violation, retained) = *rejection;
                    previous = retained;
                    violation
                })
        });
        match admitted {
            Ok(reservation) => tracked(
                OwnedDiagnostic::new(
                    primary
                        .into_inner()
                        .capture_context(location, std::iter::empty())
                        .while_handling(cause.into_inner()),
                ),
                Some(reservation),
                scope,
            ),
            Err(violation) => {
                drop(cause);
                let error = primary
                    .into_inner()
                    .rejected_context(violation, location, 0)
                    .omit_handled_cause();
                drop(previous);
                tracked(
                    OwnedDiagnostic::new(if preserve_stop {
                        preserve_primary(error)
                    } else {
                        error
                    }),
                    None,
                    scope,
                )
            }
        }
    }

    fn constructed_error(
        &self,
        category: fn(String) -> BWErr,
        message: std::fmt::Arguments<'_>,
        primary: Option<PendingDiagnostic>,
    ) -> PendingDiagnostic {
        let (stopped, mut previous) = match primary {
            Some(PendingDiagnostic { value, reservation }) => {
                if value.as_ref().is_emergency() {
                    return PendingDiagnostic {
                        value: OwnedDiagnostic::new(value.into_inner().omit_handled_cause()),
                        reservation,
                    };
                }
                (Some(value.into_inner()), reservation)
            }
            None => (None, None),
        };
        let mut failed_reservation = None;
        let (value, reservation) = self.diagnostic_limits.formatted_admitted(
            |[detail]| category(detail),
            [FormattedDetail::exact(message)],
            DiagnosticConstruction {
                location: Some((self.signature.header(), false)),
                stopped,
                related: None,
            },
            std::iter::empty(),
            |size, _, initial| {
                self.ownership
                    .diagnostic(size, initial.or_else(|| previous.take()))
                    .map_err(|rejection| {
                        let (violation, retained) = *rejection;
                        failed_reservation = retained;
                        violation
                    })
            },
        );
        // Rejection has consumed the old primary before any guard can refund it.
        drop((previous, failed_reservation));
        PendingDiagnostic {
            value: OwnedDiagnostic::new(value),
            reservation,
        }
    }

    pub(super) fn detail_error(
        &self,
        category: fn(String) -> BWErr,
        message: &str,
    ) -> PendingDiagnostic {
        self.constructed_error(category, format_args!("{message}"), None)
    }
    pub(super) fn numeric_error(&self, error: ArithmeticFailure) -> PendingDiagnostic {
        self.constructed_error(BWErr::ArithmeticError, format_args!("{error}"), None)
    }
    pub(super) fn worker_error(&self, error: &tokio::task::JoinError) -> PendingDiagnostic {
        self.constructed_error(
            BWErr::AsyncRuntime,
            format_args!("Blocking worker ended unexpectedly: {error}"),
            None,
        )
    }
    pub(super) fn worker_cleanup_error(
        &self,
        primary: impl Into<PendingDiagnostic>,
        cause: &tokio::task::JoinError,
    ) -> PendingDiagnostic {
        self.constructed_error(
            BWErr::AsyncRuntime,
            format_args!("Blocking worker ended during cleanup: {cause}"),
            Some(primary.into()),
        )
    }
    pub(super) fn signature_error(&self, message: std::fmt::Arguments<'_>) -> PendingDiagnostic {
        self.constructed_error(BWErr::OperationIncompatibleError, message, None)
    }
    pub(super) fn panic_error(&self) -> PendingDiagnostic {
        self.detail_error(BWErr::NativePanic, self.signature.normalized())
    }
}

fn tracked(
    value: OwnedDiagnostic,
    reservation: Option<Reservation>,
    scope: &Option<Arc<Reservation>>,
) -> Box<Tracked<OwnedDiagnostic>> {
    Box::new(Tracked {
        value,
        reservation,
        _scope: scope.clone(),
    })
}

fn preserve_primary(mut rejection: Diagnostic) -> Diagnostic {
    let mut original = rejection.causes.pop().expect("bounded original");
    original.causes.push(rejection);
    original
}

#[cfg(test)]
mod tests;
