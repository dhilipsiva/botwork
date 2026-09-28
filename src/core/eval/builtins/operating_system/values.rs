use super::*;

#[cfg(test)]
mod tests;

pub(super) fn strings<'a>(
    context: &Context,
    parts: impl Iterator<Item = &'a OsStr> + Clone,
) -> TemporaryResult {
    let limits = context.limits().values;
    let mut size = admitted(context, limits.container_header(0))?;
    let mut count = 0;
    for part in parts.clone() {
        context.checkpoint()?;
        count += 1;
        admitted(context, limits.container_header(count))?;
        let child = admitted(context, limits.string_size(utf8(context, part)?.len()))?;
        admitted(context, limits.add_child(&mut size, child))?;
    }
    let reservation = context.temporary_reservation(size)?;
    let mut result = Vec::with_capacity(count);
    for part in parts {
        context.checkpoint()?;
        result.push(Literal::String(utf8(context, part)?.into()));
    }
    Ok(TemporaryValue::new(Literal::Array(result), reservation))
}

pub(super) struct StringArray<'a> {
    context: &'a Context,
    limits: crate::core::value_limits::ValueLimits,
    size: ValueSize,
    value: TemporaryValue,
}

impl<'a> StringArray<'a> {
    pub(super) fn new(context: &'a Context) -> EvaluationResult<Self> {
        let limits = context.limits().values;
        let size = admitted(context, limits.container_header(0))?;
        Ok(Self {
            context,
            limits,
            size,
            value: context.temporary(Literal::Array(vec![]))?,
        })
    }
    pub(super) fn push(&mut self, name: &OsStr) -> EvaluationResult<()> {
        self.context.checkpoint()?;
        let Literal::Array(values) = &mut self.value.value else {
            unreachable!()
        };
        admitted(self.context, self.limits.container_header(values.len() + 1))?;
        let text = utf8(self.context, name)?;
        let child = admitted(self.context, self.limits.string_size(text.len()))?;
        admitted(self.context, self.limits.add_child(&mut self.size, child))?;
        let value = self.context.temporary_string(text)?;
        let (value, reservation) = value.into_parts();
        values.push(value);
        self.value.absorb(reservation);
        Ok(())
    }
    pub(super) fn sorted(mut self) -> TemporaryValue {
        let Literal::Array(values) = &mut self.value.value else {
            unreachable!()
        };
        values.sort_unstable_by(|a, b| text(a).cmp(text(b)));
        self.value
    }
}

pub(super) fn binary_size(context: &Context, count: usize) -> EvaluationResult<ValueSize> {
    let limits = context.limits().values;
    let mut size = admitted(context, limits.container_header(count))?;
    size.nodes = count.checked_add(1).ok_or_else(|| {
        context.retain_limit(Diagnostic::new(BWErr::ResourceLimit {
            resource: "value nodes",
            limit: limits.nodes as u64,
        }))
    })?;
    size.depth = if count == 0 { 1 } else { 2 };
    size.payload_bytes = count
        .checked_mul(std::mem::size_of::<i32>())
        .ok_or_else(|| {
            context.retain_limit(Diagnostic::new(BWErr::ResourceLimit {
                resource: "value payload bytes",
                limit: limits.payload_bytes as u64,
            }))
        })?;
    admitted(context, limits.check_size(size))?;
    Ok(size)
}
