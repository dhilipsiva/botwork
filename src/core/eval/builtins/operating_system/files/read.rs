use super::*;

pub(super) fn file(context: &Context, path: &Path, binary: bool) -> TemporaryResult {
    let mut file =
        fs::File::open(path).map_err(|error| io_error(context, "Open for reading", path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error(context, "Stat open file", path, error))?;
    regular(context, path, &metadata)?;
    let hint = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    from_reader(context, &mut file, path, hint, binary)
}

fn from_reader(
    context: &Context,
    input: &mut impl Read,
    path: &Path,
    hint: usize,
    binary: bool,
) -> TemporaryResult {
    let limits = context.limits().values;
    let output_size = |count| {
        if binary {
            values::binary_size(context, count)
        } else {
            admitted(context, limits.string_size(count))
        }
    };
    // Admit the opened file's length before payload allocation. A growing file
    // still needs incremental admission; a shrinking file releases unused credit.
    let mut planned = output_size(hint)?;
    let mut reservation = context.temporary_reservation(planned)?;
    let mut count = 0usize;
    let mut bytes = Vec::new();
    let mut integers = Vec::new();
    let mut buffer = [0u8; CHUNK];
    loop {
        let read = read_chunk(context, input, path, &mut buffer)?;
        if read == 0 {
            break;
        }
        count = count.checked_add(read).ok_or_else(|| {
            context.retain_limit(Diagnostic::new(BWErr::ResourceLimit {
                resource: "value payload bytes",
                limit: limits.payload_bytes as u64,
            }))
        })?;
        let actual = output_size(count)?;
        if actual.payload_bytes > planned.payload_bytes || actual.nodes > planned.nodes {
            if let Some(reservation) = &mut reservation {
                admitted(
                    context,
                    reservation.grow(
                        actual.nodes.saturating_sub(planned.nodes),
                        actual.payload_bytes.saturating_sub(planned.payload_bytes),
                    ),
                )?;
            }
            planned = actual;
        }
        if binary {
            integers.reserve_exact(read);
            integers.extend(
                buffer[..read]
                    .iter()
                    .map(|byte| Literal::Int(i32::from(*byte))),
            );
        } else {
            bytes.reserve_exact(read);
            bytes.extend_from_slice(&buffer[..read]);
        }
    }
    let actual = output_size(count)?;
    if let Some(reservation) = &mut reservation {
        reservation.release_child(ValueSize {
            nodes: planned.nodes - actual.nodes,
            depth: 0,
            payload_bytes: planned.payload_bytes - actual.payload_bytes,
        });
    }
    let value = if binary {
        Literal::Array(integers)
    } else {
        Literal::String(
            String::from_utf8(bytes)
                .map_err(|_| invalid(context, "File contents are not valid UTF-8"))?,
        )
    };
    Ok(TemporaryValue::new(value, reservation))
}

#[cfg(test)]
mod tests;
