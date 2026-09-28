use super::*;

pub(super) fn size(
    context: &Context,
    binary: bool,
    stdout: usize,
    stderr: usize,
    exit_none: bool,
    signal_none: bool,
) -> EvaluationResult<ValueSize> {
    let limits = context.limits().values;
    let mut size = admitted(context, limits.container_header(5))?;
    for key in ["stdout", "stderr", "exit_code", "signal", "success"] {
        admitted(context, limits.key_size(key.len()))?;
        admitted(context, limits.add_bytes(&mut size, key.len()))?;
    }
    for count in [stdout, stderr] {
        let child = if binary {
            admitted(context, limits.container_header(count))?;
            let child = ValueSize {
                nodes: count
                    .checked_add(1)
                    .ok_or_else(|| limit(context, "value nodes", limits.nodes))?,
                depth: if count == 0 { 1 } else { 2 },
                payload_bytes: count
                    .checked_mul(4)
                    .ok_or_else(|| limit(context, "value payload bytes", limits.payload_bytes))?,
            };
            admitted(context, limits.check_size(child))?;
            child
        } else {
            admitted(context, limits.string_size(count))?
        };
        admitted(context, limits.add_child(&mut size, child))?;
    }
    for bytes in [
        if exit_none { 0 } else { 4 },
        if signal_none { 0 } else { 4 },
        1,
    ] {
        admitted(
            context,
            limits.add_child(
                &mut size,
                ValueSize {
                    nodes: 1,
                    depth: 1,
                    payload_bytes: bytes,
                },
            ),
        )?;
    }
    Ok(size)
}

pub(super) fn build(
    context: &Context,
    binary: bool,
    report: &mut WorkerReport,
    planned: ValueSize,
    retention: &Retention,
) -> TemporaryResult {
    let status = report.exit_status.expect("completed process status");
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    };
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    let actual = size(
        context,
        binary,
        report.stdout.len(),
        report.stderr.len(),
        status.code().is_none(),
        signal.is_none(),
    )?;
    let mut reservation = retention
        .output
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    if let Some(reservation) = &mut reservation {
        reservation.release_child(ValueSize {
            nodes: planned.nodes - actual.nodes,
            depth: 0,
            payload_bytes: planned.payload_bytes - actual.payload_bytes,
        });
    }
    let stdout = capture(
        context,
        binary,
        std::mem::take(&mut report.stdout),
        "Process stdout is not valid UTF-8",
    )?;
    let stderr = capture(
        context,
        binary,
        std::mem::take(&mut report.stderr),
        "Process stderr is not valid UTF-8",
    )?;
    let result = HashMap::from([
        ("stdout".into(), stdout),
        ("stderr".into(), stderr),
        (
            "exit_code".into(),
            status.code().map_or(Literal::None, Literal::Int),
        ),
        ("signal".into(), signal.map_or(Literal::None, Literal::Int)),
        ("success".into(), Literal::Bool(status.success())),
    ]);
    Ok(TemporaryValue::new(Literal::Map(result), reservation))
}

fn capture(
    context: &Context,
    binary: bool,
    bytes: Vec<u8>,
    reason: &str,
) -> EvaluationResult<Literal> {
    if !binary {
        return String::from_utf8(bytes)
            .map(Literal::String)
            .map_err(|_| invalid(context, reason));
    }
    let mut values = Vec::with_capacity(bytes.len());
    for chunk in bytes.chunks(4096) {
        context.checkpoint()?;
        values.extend(chunk.iter().map(|byte| Literal::Int(i32::from(*byte))));
    }
    Ok(Literal::Array(values))
}
