use super::*;
use crate::core::{run::ResultLimits, value_limits::ValueSize};

#[cfg(test)]
mod tests;

impl Context {
    fn check_result_export(
        &self,
        result: Option<&Literal>,
        limits: &ResultLimits,
    ) -> DiagnosticResult<()> {
        // RetainedName hashes immutable text; counters are lifetime metadata.
        #[allow(clippy::mutable_key_type)]
        let variables = &self.frames[0].variables;
        limits.count(variables.len(), result.is_some())?;
        limits.names(variables.keys().map(|name| name.as_str().len()))?;
        let mut used = ValueSize::default();
        if let Some(value) = result {
            limits.value(value, &mut used)?;
        }
        // Bounded reference scratch preserves deterministic rejection order.
        let mut bindings: Vec<_> = variables.iter().collect();
        bindings.sort_unstable_by_key(|(name, _)| name.as_str());
        for (_, stored) in bindings {
            limits.value(&stored.value, &mut used)?;
        }
        Ok(())
    }

    pub(crate) fn finish_result(
        mut self,
        result: EvaluationResult<Literal>,
        limits: &ResultLimits,
    ) -> (RuntimeResult, BTreeMap<String, Literal>, Option<Diagnostic>) {
        let result = result.map(Owned::new);
        if let Err(export_error) =
            self.check_result_export(result.as_ref().ok().map(|value| &**value), limits)
        {
            let primary = match result {
                Ok(_) => export_error.clone(),
                Err(original) => original.into_diagnostic(),
            };
            return (Err(primary), BTreeMap::new(), Some(export_error));
        }
        let variables = std::mem::take(&mut self.frames[0].variables)
            .into_iter()
            .map(|(name, value)| (name.into_string(), StoredValue::into_value(value)))
            .collect();
        (
            result
                .map(Owned::into_inner)
                .map_err(RuntimeDiagnostic::into_diagnostic),
            variables,
            None,
        )
    }
}
