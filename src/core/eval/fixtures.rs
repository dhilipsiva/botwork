use super::*;
use crate::core::{run::SnapshotSize, suite::Suite};
use std::future::Future;

/// Immutable borrowed suite inputs. Clones share admitted names/value owners;
/// installing a case's independent copy admits its storage before copying.
#[derive(Clone)]
pub struct FixtureInputs {
    bindings: Arc<Vec<(RetainedName, Arc<StoredValue>)>>,
    control: OperationControl,
}

impl FixtureInputs {
    pub fn control(&self) -> &OperationControl {
        &self.control
    }

    /// Atomically inherit missing root bindings. Existing case inputs, including
    /// parameterized row bindings, take precedence and are never copied twice.
    pub fn inherit_into(&self, context: &mut Context) -> DiagnosticResult<()> {
        self.control.checkpoint()?;
        context.checkpoint()?;
        let mut size = SnapshotSize::default();
        size.entries(self.bindings.len());
        context.charge_snapshot(size)?;
        let mut bindings = Vec::with_capacity(self.bindings.len());
        for (name, value) in self.bindings.iter() {
            if context.frames[0].variables.contains_key(name.as_str()) {
                continue;
            }
            let reservation = context.reserve_value(&value.value)?;
            let name = context.variable_key(name.as_str(), 0)?;
            let value = Arc::new(StoredValue::new(value.value.clone(), reservation));
            bindings.push((name, value));
        }
        self.control.checkpoint()?;
        context.checkpoint()?;
        context.frames[0].variables.extend(bindings);
        Ok(())
    }
}

impl Context {
    fn fixture_inputs(&self) -> DiagnosticResult<FixtureInputs> {
        self.checkpoint()?;
        // Only immutable name text participates in hashing; quotas track lifetime.
        #[allow(clippy::mutable_key_type)]
        let variables = &self.frames[0].variables;
        let mut size = SnapshotSize::default();
        size.entries(variables.len());
        self.charge_snapshot(size)?;
        self.check_result_export(None, &self.limits().results)?;
        let mut bindings: Vec<_> = variables
            .iter()
            .map(|(name, value)| (name.clone(), Arc::clone(value)))
            .collect();
        bindings.sort_unstable_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
        self.checkpoint()?;
        Ok(FixtureInputs {
            bindings: Arc::new(bindings),
            control: self
                .budget
                .as_ref()
                .map(|budget| budget.control().clone())
                .unwrap_or_default(),
        })
    }
}

/// The fixture result covers setup, teardown and the owner's control. The body
/// contains independently reported case outcomes, if setup admitted the body.
pub struct FixtureResult<T> {
    pub result: DiagnosticResult<()>,
    pub body: Option<T>,
}

/// Retain one suite context through setup, all borrowers, and awaited teardown.
/// The body must join/drain every borrower before returning. Its case contexts
/// must inherit FixtureInputs::control. Request cancellation and keep polling
/// to finish cleanup; dropping this future cannot execute asynchronous teardown.
/// Context deadlines cover the whole suite lifetime, including body execution.
pub fn evaluate_suite_fixture_async<'a, T: 'a, F, Fut>(
    suite: &'a Suite,
    mut context: Context,
    body: F,
) -> impl Future<Output = FixtureResult<T>> + 'a
where
    F: FnOnce(FixtureInputs) -> Fut + 'a,
    Fut: Future<Output = T> + 'a,
{
    context.asynchronous = true;
    async move {
        let programs = suite.fixture_programs();
        let admitted = (|| {
            context.check_execution_mode()?;
            let limits = context.limits();
            for program in [&programs.setup, &programs.teardown] {
                program.validate_with_reporter(&limits.ast, limits.source_bytes, |failure| {
                    context.ast_error(failure)
                })?;
            }
            Ok::<_, RuntimeDiagnostic>(())
        })();
        if let Err(error) = admitted {
            return FixtureResult {
                result: Err(context
                    .runtime_diagnostic(error, None, false)
                    .into_diagnostic()),
                body: None,
            };
        }
        // Ownership is armed before Library/Setup can acquire resources.
        let setup = execution::evaluate_program_runtime(&programs.setup, &mut context).await;
        let setup = context
            .after_evaluation(setup)
            .and_then(|_| context.fixture_inputs().map_err(Into::into));
        let mut body_result = None;
        let primary = match setup {
            Err(error) => Err(error),
            Ok(inputs) => {
                body_result = Some(body(inputs).await);
                context.after_evaluation(Ok(()))
            }
        }
        .map_err(|error| context.runtime_diagnostic(error, None, false));
        let cleaned = match cleanup::CleanupContext::enter(&mut context) {
            Ok(guard) => {
                let result =
                    execution::evaluate_program_runtime(&programs.teardown, guard.context).await;
                guard
                    .context
                    .after_evaluation(result)
                    .map(|_| ())
                    .map_err(|error| guard.context.runtime_diagnostic(error, None, false))
            }
            Err(error) => Err(error),
        };
        let result = match (primary, cleaned) {
            (primary, Ok(())) => primary,
            (Ok(()), Err(cleanup)) => Err(cleanup),
            (Err(primary), Err(cleanup)) => {
                Err(primary.with_cleanup(cleanup, context.budget.as_ref()))
            }
        };
        FixtureResult {
            result: context.after_evaluation(result).map_err(|error| {
                context
                    .runtime_diagnostic(error, None, false)
                    .into_diagnostic()
            }),
            body: body_result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::diagnostic::DiagnosticCode;

    fn inputs() -> FixtureInputs {
        let mut owner = Context::with_limits(RunLimits::default()).unwrap();
        owner
            .set_input_variables(BTreeMap::from([
                ("a".into(), Literal::Int(1)),
                ("b".into(), Literal::Int(2)),
            ]))
            .unwrap();
        owner.fixture_inputs().unwrap()
    }

    #[test]
    fn rejected_inheritance_publishes_no_partial_bindings_and_releases_reservations() {
        let mut limits = RunLimits::default();
        limits.retained_values.values = 1;
        let mut case = Context::with_limits(limits).unwrap();
        assert_eq!(
            inputs().inherit_into(&mut case).unwrap_err().code(),
            DiagnosticCode::ResourceLimit
        );
        assert!(case.frames[0].variables.is_empty());
        let guard = cleanup::CleanupContext::enter(&mut case).unwrap();
        assert!(guard.context.reserve_value(&Literal::Int(3)).is_ok());
    }

    #[test]
    fn snapshot_admission_charges_exact_table_size_before_publication() {
        for (maximum, accepted) in [(1, false), (2, true)] {
            let mut limits = RunLimits::default();
            limits.snapshots.entries = maximum;
            let mut case = Context::with_limits(limits).unwrap();
            assert_eq!(inputs().inherit_into(&mut case).is_ok(), accepted);
            assert_eq!(case.frames[0].variables.len(), if accepted { 2 } else { 0 });
        }
    }

    #[test]
    fn stopped_parent_and_case_reject_snapshot_inheritance_before_publication() {
        let snapshot = inputs();
        let mut case = Context::with_limits(RunLimits::default()).unwrap();
        snapshot.control.cancel();
        assert_eq!(
            snapshot.inherit_into(&mut case).unwrap_err().code(),
            DiagnosticCode::Cancelled
        );
        assert!(case.frames[0].variables.is_empty());
        let snapshot = inputs();
        let control = OperationControl::default();
        let mut case = Context::with_control(RunLimits::default(), control.clone()).unwrap();
        control.cancel();
        assert_eq!(
            snapshot.inherit_into(&mut case).unwrap_err().code(),
            DiagnosticCode::Cancelled
        );
        assert!(case.frames[0].variables.is_empty());
    }
}
