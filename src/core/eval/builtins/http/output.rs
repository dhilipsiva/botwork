use super::*;

pub(super) fn planned(context: &Context, plan: &config::Plan) -> EvaluationResult<ValueSize> {
    let limits = context.limits().values;
    admitted(context, limits.container_header(7))?;
    admitted(context, limits.container_header(plan.max_headers))?;
    admitted(
        context,
        limits.key_size(plan.max_header_bytes.clamp(9, 256)),
    )?;
    admitted(context, limits.string_size(MAX_URL))?;
    if plan.binary {
        admitted(context, limits.container_header(plan.max_body))?;
        admitted(context, limits.container_header(plan.max_header_bytes))?;
    } else {
        admitted(context, limits.string_size(plan.max_body))?;
        admitted(context, limits.string_size(plan.max_header_bytes))?;
    }
    let size = ValueSize {
        nodes: 9
            + 2 * plan.max_headers
            + if plan.binary {
                plan.max_body + plan.max_header_bytes
            } else {
                0
            },
        depth: if plan.binary { 5 } else { 4 },
        payload_bytes: 57
            + MAX_URL
            + if plan.binary {
                4 * plan.max_body + 5 * plan.max_header_bytes
            } else {
                plan.max_body + plan.max_header_bytes
            },
    };
    admitted(context, limits.check_size(size))?;
    Ok(size)
}

pub(super) fn build(
    context: &Context,
    binary: bool,
    response: transport::Response,
    planned: ValueSize,
    mut reservation: Option<TemporaryReservation>,
) -> TemporaryResult {
    fn capture(context: &Context, binary: bool, bytes: Vec<u8>) -> EvaluationResult<Literal> {
        if binary {
            Ok(Literal::Array(
                bytes
                    .into_iter()
                    .map(|b| Literal::Int(i32::from(b)))
                    .collect(),
            ))
        } else {
            String::from_utf8(bytes)
                .map(Literal::String)
                .map_err(|_| invalid(context, "HTTP response is not valid UTF-8"))
        }
    }
    // The maximum shape was reserved before network work or owned response copies.
    let mut headers = HashMap::new();
    for (name, value) in &response.headers {
        context.checkpoint()?;
        let value = capture(context, binary, value.as_bytes().to_vec())?;
        let entry = headers
            .entry(name.as_str().to_owned())
            .or_insert_with(|| Literal::Array(Vec::new()));
        let Literal::Array(values) = entry else {
            unreachable!("header Array")
        };
        values.push(value);
    }
    let body = capture(context, binary, response.body)?;
    let value = Literal::Map(HashMap::from([
        ("status".into(), Literal::Int(i32::from(response.status))),
        ("headers".into(), Literal::Map(headers)),
        ("body".into(), body),
        ("url".into(), Literal::String(response.url)),
        (
            "success".into(),
            Literal::Bool((200..300).contains(&response.status)),
        ),
        ("redirects".into(), Literal::Int(response.redirects as i32)),
        ("attempts".into(), Literal::Int(response.attempts as i32)),
    ]));
    let actual = admitted(context, context.limits().values.check(&value))?;
    if let Some(reservation) = &mut reservation {
        reservation.release_child(ValueSize {
            nodes: planned.nodes - actual.nodes,
            depth: 0,
            payload_bytes: planned.payload_bytes - actual.payload_bytes,
        });
    }
    Ok(TemporaryValue::new(value, reservation))
}
