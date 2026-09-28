use super::*;

pub(super) fn array_size(
    limits: &ValueLimits,
    count: usize,
    children: impl IntoIterator<Item = Result<ValueSize, BWErr>>,
) -> Result<ValueSize, BWErr> {
    let mut size = limits.container_header(count)?;
    for child in children {
        limits.add_child(&mut size, child?)?;
    }
    Ok(size)
}

pub(super) fn map_size<'a>(
    limits: &ValueLimits,
    count: usize,
    entries: impl Iterator<Item = (&'a str, &'a Literal)>,
) -> Result<ValueSize, BWErr> {
    let mut size = limits.container_header(count)?;
    for (key, value) in entries {
        limits.key_size(key.len())?;
        limits.add_bytes(&mut size, key.len())?;
        limits.add_child(&mut size, limits.check(value)?)?;
    }
    Ok(size)
}

pub(super) fn repeat_size(
    limits: &ValueLimits,
    value: &Literal,
    count: usize,
) -> Result<ValueSize, BWErr> {
    let empty = limits.container_header(count)?;
    if count == 0 {
        return Ok(empty);
    }
    let child = limits.check(value)?;
    let overflow = |resource, limit: usize| BWErr::ResourceLimit {
        resource,
        limit: limit as u64,
    };
    let size = ValueSize {
        nodes: child
            .nodes
            .checked_mul(count)
            .and_then(|nodes| nodes.checked_add(1))
            .ok_or_else(|| overflow("value nodes", limits.nodes))?,
        payload_bytes: child
            .payload_bytes
            .checked_mul(count)
            .ok_or_else(|| overflow("value payload bytes", limits.payload_bytes))?,
        depth: child.depth + 1,
    };
    limits.check_size(size)?;
    Ok(size)
}

pub(super) fn produce(
    context: &Context,
    size: ValueSize,
    build: impl FnOnce() -> EvaluationResult<Literal>,
) -> TemporaryResult {
    let reservation = context.temporary_reservation(size)?;
    context.checkpoint()?;
    let value = build()?;
    Ok(TemporaryValue::new(value, reservation))
}

pub(super) fn array<'a>(
    context: &Context,
    size: ValueSize,
    count: usize,
    values: impl Iterator<Item = &'a Literal>,
) -> TemporaryResult {
    produce(context, size, || {
        let mut result = Vec::with_capacity(count);
        for value in values {
            context.checkpoint()?;
            result.push(value.clone());
        }
        Ok(Literal::Array(result))
    })
}

pub(super) fn map<'a>(
    context: &Context,
    size: ValueSize,
    count: usize,
    values: impl Iterator<Item = (&'a str, &'a Literal)>,
) -> TemporaryResult {
    produce(context, size, || {
        let mut result = HashMap::with_capacity(count);
        for (key, value) in values {
            context.checkpoint()?;
            if result.contains_key(key) {
                return Err(context.formatted_error(
                    BWErr::OperationIncompatibleError,
                    format_args!("Duplicate map key {key:?}"),
                    None,
                    false,
                ));
            }
            result.insert(key.to_owned(), value.clone());
        }
        Ok(Literal::Map(result))
    })
}
