use super::*;

pub(super) fn invoke(op: OsOp, arguments: &[TemporaryValue], context: &Context) -> TemporaryResult {
    let environment = context.environment.as_ref().ok_or_else(|| {
        context.detail_error(
            BWErr::RunConfiguration,
            "Environment statements require an Engine run environment",
            None,
            false,
        )
    })?;
    if matches!(op, OsOp::Environment) {
        let limits = context.limits().values;
        let mut size = admitted(
            context,
            limits.container_header(environment.variables().len()),
        )?;
        for (name, value) in environment.variables() {
            context.checkpoint()?;
            let name = utf8(context, name)?;
            let value = utf8(context, value)?;
            admitted(context, limits.key_size(name.len()))?;
            let child = admitted(context, limits.string_size(value.len()))?;
            size.payload_bytes = size.payload_bytes.checked_add(name.len()).ok_or_else(|| {
                context.retain_limit(Diagnostic::new(BWErr::ResourceLimit {
                    resource: "value payload bytes",
                    limit: limits.payload_bytes as u64,
                }))
            })?;
            admitted(context, limits.add_child(&mut size, child))?;
        }
        let reservation = context.temporary_reservation(size)?;
        let mut values = HashMap::with_capacity(environment.variables().len());
        for (name, value) in environment.variables() {
            context.checkpoint()?;
            values.insert(
                utf8(context, name)?.into(),
                Literal::String(utf8(context, value)?.into()),
            );
        }
        return Ok(TemporaryValue::new(Literal::Map(values), reservation));
    }
    let name = text(&arguments[0]);
    if name.is_empty() || name.contains(['=', '\0']) {
        return Err(invalid(
            context,
            "Environment names must be nonempty and contain neither '=' nor NUL",
        ));
    }
    let value = environment.get(name);
    if matches!(op, OsOp::EnvironmentExists) {
        return context.temporary(Literal::Bool(value.is_some()));
    }
    match value {
        Some(value) => context.temporary_string(utf8(context, value)?),
        None => context.temporary(Literal::None),
    }
}
