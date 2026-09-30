//! One evaluator for synchronous entry points and owned asynchronous runs.
use super::*;
use std::{future::Future, pin::Pin};

type EvalFuture<'a, T> = Pin<Box<dyn Future<Output = EvaluationResult<T>> + Send + 'a>>;

pub(super) async fn tick(context: &mut Context) -> EvaluationResult<()> {
    context.tick()?;
    if context.asynchronous
        && context
            .budget
            .as_ref()
            .is_some_and(|budget| budget.used() % 256 == 0)
    {
        tokio::task::yield_now().await;
        context.checkpoint()?;
    }
    Ok(())
}

pub(super) async fn invoke(call: &Call, context: &mut Context) -> TemporaryResult {
    invoke_inner(call, context)
        .await
        .map_err(|error| context.runtime_diagnostic(error, Some(&call.span), false))
}

pub(super) async fn invoke_inner(call: &Call, context: &mut Context) -> TemporaryResult {
    let (definition, owner) = context.get_statement(&call.signature).ok_or_else(|| {
        context.detail_error(
            BWErr::StatementNotDefined,
            call.span.text(),
            Some(&call.span),
            false,
        )
    })?;
    if matches!(
        definition,
        StmtType::Operation { .. }
            | StmtType::Native {
                body: NativeBody::Builtin(builtins::Builtin::Http(_)),
                ..
            }
    ) && !context.asynchronous
    {
        return Err(context.detail_error(
            BWErr::AsyncRuntime,
            "Use asynchronous execution for this statement",
            Some(&call.span),
            false,
        ));
    }
    let metadata = definition.metadata();
    let parameter_count = metadata.parameters().len();
    if parameter_count != call.arguments.len() {
        return Err(context.detail_error(
            BWErr::ParameterMissingError,
            "The call does not match the definition's parameter count",
            Some(&call.span),
            false,
        ));
    }
    context.check_call_depth()?;
    let mut argument_slots = context
        .budget
        .as_ref()
        .map(|budget| budget.reserve_argument_slots(parameter_count))
        .transpose()?;
    let mut arguments = Vec::with_capacity(parameter_count);
    for (index, argument) in call.arguments.iter().enumerate() {
        if let Some(reservation) = &mut argument_slots {
            reservation.release_argument_slot();
        }
        let value = evaluate_expression(argument, context).await?;
        validate_numeric_values(&value).map_err(|error| {
            context.formatted_error(
                BWErr::ArithmeticError,
                format_args!("{error}"),
                Some(&argument.span),
                true,
            )
        })?;
        metadata.validate_argument(index, &value, |message| {
            context.formatted_error(
                BWErr::OperationIncompatibleError,
                message,
                Some(&argument.span),
                true,
            )
        })?;
        arguments.push(value);
    }
    invoke_resolved(call, definition, owner, arguments, context).await
}

pub(super) fn invoke_resolved<'a>(
    call: &'a Call,
    definition: StmtType,
    owner: usize,
    arguments: Vec<TemporaryValue>,
    context: &'a mut Context,
) -> EvalFuture<'a, TemporaryValue> {
    Box::pin(async move {
        let _depth = context
            .enter_evaluation()
            .map_err(|error| context.runtime_diagnostic(error.into(), Some(&call.span), false))?;
        match definition {
            StmtType::Operation { operation, .. } => {
                let control = context
                    .environment
                    .as_ref()
                    .map(|environment| environment.control().clone())
                    .or_else(|| {
                        context
                            .budget
                            .as_ref()
                            .map(|budget| budget.control().clone())
                    })
                    .unwrap_or_default();
                context.check_call_depth()?;
                let statement = Some(operation.signature().header());
                let frame = context.retain_call(&call.signature, statement, &call.span, None)?;
                context.calls.push(frame);
                let (values, reservations): (Vec<_>, Vec<_>) = arguments
                    .into_iter()
                    .map(TemporaryValue::into_parts)
                    .unzip();
                // invoke admits ownership synchronously. Release the evaluator's
                // argument leases only after the operation has taken responsibility.
                let pending = operation.invoke(values, control);
                drop(reservations);
                let result = pending.await.map(Owned::new).map_err(|error| {
                    context.latch_limit(&error);
                    context.runtime_diagnostic(error.into(), Some(&call.span), false)
                });
                let result = context
                    .after_evaluation(result)
                    .and_then(|value| context.temporary(value.into_inner()))
                    .map_err(|error| context.runtime_diagnostic(error, Some(&call.span), false));
                context.calls.pop();
                result
            }
            StmtType::Imported {
                module,
                exported,
                import_site,
                ..
            } => {
                imports::invoke_imported(call, &module, &exported, arguments, &import_site, context)
                    .await
            }
            StmtType::Native {
                body,
                metadata,
                _registry,
            } => {
                if let NativeBody::Builtin(builtins::Builtin::Http(kind)) = body {
                    context.check_call_depth()?;
                    let statement = Some(metadata.header());
                    let frame =
                        context.retain_call(&call.signature, statement, &call.span, None)?;
                    context.calls.push(frame);
                    let result = kind.invoke(&arguments, context).await;
                    let result = context
                        .after_evaluation(result)
                        .and_then(|value| {
                            blocking::validate_result(context, &metadata, &value, &call.span)?;
                            Ok(value)
                        })
                        .map_err(|error| {
                            context.runtime_diagnostic(error, Some(&call.span), false)
                        });
                    context.calls.pop();
                    return result;
                }
                // Inspection needs caller bindings. Log, regex, filesystem work,
                // and arbitrary host callbacks run on bounded blocking workers.
                let inline = matches!(&body, NativeBody::Builtin(kind) if !kind.needs_worker());
                let directory =
                    matches!(&body, NativeBody::Builtin(kind) if kind.needs_directory());
                if context.asynchronous && !inline {
                    context.check_call_depth()?;
                    let statement = Some(metadata.header());
                    let frame =
                        context.retain_call(&call.signature, statement, &call.span, None)?;
                    context.calls.push(frame);
                    let span = call.span.clone();
                    let result = context
                        .blocking_with_directory(directory, move |worker| {
                            let _registry = _registry;
                            blocking::native_body(worker, body, &metadata, arguments, &span)
                        })
                        .await
                        .map_err(|error| {
                            context.runtime_diagnostic(error, Some(&call.span), false)
                        });
                    context.calls.pop();
                    result
                } else {
                    let statement = Some(metadata.header());
                    context.with_call(&call.signature, statement, &call.span, None, |context| {
                        blocking::native_body(context, body, &metadata, arguments, &call.span)
                    })
                }
            }
            StmtType::UserDefined {
                definition,
                metadata,
                ..
            } => {
                let frame = Frame {
                    variables: definition
                        .parameters
                        .iter()
                        .zip(arguments)
                        .map(|(parameter, value)| {
                            let (value, _reservation) = value.into_parts();
                            let value = context.store_value(value)?;
                            context
                                .retain_name(&parameter.text)
                                .map(|name| (name, value))
                        })
                        .collect::<DiagnosticResult<HashMap<_, _>>>()?,
                    parent: Some(owner),
                    ..Frame::default()
                };
                context.check_call_depth().map_err(|error| {
                    context.runtime_diagnostic(error.into(), Some(&call.span), false)
                })?;
                let statement = Some(metadata.header());
                let call_frame = context.retain_call(
                    &call.signature,
                    statement,
                    &call.span,
                    Some(&definition.span),
                )?;
                context.calls.push(call_frame);
                let caller = context.current;
                context.current = context.frames.len();
                context.frames.push(frame);
                let completion = evaluate_block(&definition.body, context).await;
                context.frames.pop();
                context.current = caller;
                let result = completion
                    .and_then(|completion| {
                        let value = match completion {
                            Completion::Return(value) => value,
                            completion => finish_script(completion, context, &call.span)?,
                        };
                        metadata.validate_return(&value, |message| {
                            context.formatted_error(
                                BWErr::OperationIncompatibleError,
                                message,
                                Some(&call.span),
                                false,
                            )
                        })?;
                        Ok(value)
                    })
                    .map_err(|error| context.runtime_diagnostic(error, None, false));
                context.calls.pop();
                result
            }
        }
    })
}

pub(super) fn evaluate_expression<'a>(
    expression: &'a Expr,
    context: &'a mut Context,
) -> EvalFuture<'a, TemporaryValue> {
    Box::pin(async move {
        let _depth = context.enter_evaluation().map_err(|error| {
            context.runtime_diagnostic(error.into(), Some(&expression.span), true)
        })?;
        evaluate_expression_inner(expression, context)
            .await
            .and_then(|value| {
                context.check_value(&value)?;
                Ok(value)
            })
            .map_err(|error| context.runtime_diagnostic(error, Some(&expression.span), true))
    })
}

pub(super) async fn evaluate_expression_inner(
    expression: &Expr,
    context: &mut Context,
) -> TemporaryResult {
    tick(context).await?;
    #[cfg(test)]
    context
        .expression_visits
        .borrow_mut()
        .push(expression.span.text().to_owned());

    match &expression.kind {
        ExprKind::Integer(text) => text
            .parse::<i32>()
            .map(Literal::Int)
            .map_err(|error| {
                context.formatted_error(
                    BWErr::ParsingIntegerError,
                    format_args!("{error}"),
                    Some(&expression.span),
                    true,
                )
            })
            .and_then(|value| context.temporary(value)),
        ExprKind::Float(text) => {
            let value = text.parse::<f32>().map_err(|error| {
                context.formatted_error(
                    BWErr::ParsingIntegerError,
                    format_args!("{error}"),
                    Some(&expression.span),
                    true,
                )
            })?;
            let value = finite_float(value, "Float literal").map_err(|error| {
                context.formatted_error(
                    BWErr::ArithmeticError,
                    format_args!("{error}"),
                    Some(&expression.span),
                    true,
                )
            })?;
            context.temporary(value)
        }
        ExprKind::Bool(value) => context.temporary(Literal::Bool(*value)),
        ExprKind::String(value) => context.temporary_string(value),
        ExprKind::Variable(name) => context.copy_temporary(
            &context
                .get_variable_binding(name, Some(&expression.span))?
                .value,
        ),
        ExprKind::Call(call) => invoke(call, context).await,
        ExprKind::Access { base, segments } => evaluate_access(base, segments, context).await,
        ExprKind::Array(elements) => {
            let limits = context.limits().values;
            let admit = |error| context.retain_limit(Diagnostic::new(error));
            let mut size = limits.container_header(elements.len()).map_err(admit)?;
            let mut planned = size;
            planned.nodes += elements.len();
            let reservation = context.temporary_reservation(planned)?;
            let mut result = TemporaryValue::new(
                Literal::Array(Vec::with_capacity(elements.len())),
                reservation,
            );
            for element in elements {
                result.release_child(crate::core::value_limits::ValueSize {
                    nodes: 1,
                    ..Default::default()
                });
                let value = evaluate_expression(element, context).await?;
                let child = limits
                    .check(&value)
                    .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                limits
                    .add_child(&mut size, child)
                    .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                let (value, reservation) = value.into_parts();
                let Literal::Array(values) = &mut result.value else {
                    unreachable!()
                };
                values.push(value);
                result.absorb(reservation);
            }
            Ok(result)
        }
        ExprKind::Map(entries) => {
            let limits = context.limits().values;
            let mut size = limits
                .container_header(0)
                .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
            let mut keys = HashSet::new();
            for (key, _) in entries {
                limits
                    .key_size(key.text.len())
                    .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                if !keys.contains(key.text.as_str()) {
                    limits
                        .container_header(keys.len() + 1)
                        .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                    limits
                        .add_bytes(&mut size, key.text.len())
                        .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                    keys.insert(key.text.as_str());
                }
            }
            let mut planned = size;
            planned.nodes += keys.len();
            let reservation = context.temporary_reservation(planned)?;
            let mut result = TemporaryValue::new(Literal::Map(HashMap::new()), reservation);
            for (key, expression) in entries {
                let Literal::Map(values) = &result.value else {
                    unreachable!()
                };
                if !values.contains_key(&key.text) {
                    result.release_child(crate::core::value_limits::ValueSize {
                        nodes: 1,
                        ..Default::default()
                    });
                }
                let value = evaluate_expression(expression, context).await?;
                let child = limits
                    .check(&value)
                    .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                let Literal::Map(values) = &result.value else {
                    unreachable!()
                };
                let old_size = if let Some(previous) = values.get(&key.text) {
                    let old = limits
                        .check(previous)
                        .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                    size.nodes -= old.nodes;
                    size.payload_bytes -= old.payload_bytes;
                    // Keeping a previous maximum depth is safe within this map;
                    // the completed value is measured afresh by its caller.
                    Some(old)
                } else {
                    None
                };
                limits
                    .add_child(&mut size, child)
                    .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                let (value, reservation) = value.into_parts();
                let Literal::Map(values) = &mut result.value else {
                    unreachable!()
                };
                if let Some(previous) = values.get_mut(&key.text) {
                    let old = std::mem::replace(previous, value);
                    drop(old);
                    result.release_child(old_size.expect("existing map child"));
                } else {
                    values.insert(key.text.clone(), value);
                }
                result.absorb(reservation);
            }
            Ok(result)
        }
        ExprKind::Unary {
            operator, operand, ..
        } => match (operator, &operand.kind) {
            (UnaryOp::Negate, ExprKind::Integer(text)) => {
                tick(context).await?;
                // Convert the signed atom together: MIN's positive magnitude is not i32.
                // Compound operands still evaluate normally before checked negation.
                #[cfg(test)]
                context
                    .expression_visits
                    .borrow_mut()
                    .push(operand.span.text().to_owned());
                format!("-{text}")
                    .parse::<i32>()
                    .map(Literal::Int)
                    .map_err(|error| {
                        context.formatted_error(
                            BWErr::ParsingIntegerError,
                            format_args!("{error}"),
                            Some(&expression.span),
                            true,
                        )
                    })
                    .and_then(|value| context.temporary(value))
            }
            _ => {
                let operand = evaluate_expression(operand, context).await?;
                context.temporary_unary(*operator, operand, &expression.span)
            }
        },
        ExprKind::Binary {
            operator,
            left,
            right,
            ..
        } => {
            let left = evaluate_expression(left, context).await?;
            if matches!(operator, BinaryOp::And | BinaryOp::Or) {
                let Literal::Bool(value) = &*left else {
                    let name = if *operator == BinaryOp::And {
                        "and"
                    } else {
                        "or"
                    };
                    return Err(context.formatted_error(
                        BWErr::OperationIncompatibleError,
                        format_args!("The left operand of `{name}` must be a boolean"),
                        Some(&expression.span),
                        true,
                    ));
                };
                if (*operator == BinaryOp::And && !value) || (*operator == BinaryOp::Or && *value) {
                    return Ok(left);
                }
            }
            let right = evaluate_expression(right, context).await?;
            context.temporary_binary(*operator, left, right, &expression.span)
        }
    }
}

pub(super) async fn evaluate_access(
    base: &Expr,
    segments: &[AccessSegment],
    context: &mut Context,
) -> TemporaryResult {
    // Retain an immutable snapshot across effectful index calls without copying
    // the whole variable container. Only the selected result is copied.
    let binding;
    let temporary;
    let mut value = if let ExprKind::Variable(name) = &base.kind {
        #[cfg(test)]
        context
            .expression_visits
            .borrow_mut()
            .push(base.span.text().to_owned());
        binding = Arc::clone(context.get_variable_binding(name, Some(&base.span))?);
        &binding.value
    } else {
        temporary = evaluate_expression(base, context).await?;
        &temporary
    };
    let error = |context: &Context, segment: &AccessSegment, reason: std::fmt::Arguments<'_>| {
        context.access_error(base, segments, segment, reason)
    };
    for segment in segments {
        // Evaluate this key before checking its receiver/type; do not evaluate
        // any later key until this lookup succeeds.
        let key = match segment {
            AccessSegment::Literal(_) => None,
            AccessSegment::Computed { index, .. } => {
                Some(evaluate_expression(index, context).await?)
            }
        };
        value = match value {
            Literal::Map(values) => {
                let name = match (segment, key.as_deref()) {
                    (AccessSegment::Literal(name), _) => &name.text,
                    (_, Some(Literal::String(name))) => name,
                    _ => {
                        return Err(error(
                            context,
                            segment,
                            format_args!("map key must be a string"),
                        ))
                    }
                };
                values.get(name).ok_or_else(|| {
                    error(context, segment, format_args!("map key does not exist"))
                })?
            }
            Literal::Array(values) => {
                let index = match (segment, key.as_deref()) {
                    (AccessSegment::Literal(name), _) => {
                        if name.text.is_empty()
                            || !name.text.bytes().all(|byte| byte.is_ascii_digit())
                        {
                            return Err(error(
                                context,
                                segment,
                                format_args!("array index must contain ASCII decimal digits"),
                            ));
                        }
                        name.text.parse::<usize>().ok()
                    }
                    (_, Some(Literal::Int(index))) if *index >= 0 => usize::try_from(*index).ok(),
                    _ => {
                        return Err(error(
                            context,
                            segment,
                            format_args!("array index must be a nonnegative integer"),
                        ))
                    }
                };
                index.and_then(|index| values.get(index)).ok_or_else(|| {
                    error(
                        context,
                        segment,
                        format_args!("array index is out of bounds for length {}", values.len()),
                    )
                })?
            }
            _ => {
                return Err(error(
                    context,
                    segment,
                    format_args!("value is neither a map nor an array"),
                ))
            }
        };
    }
    context.copy_temporary(value)
}

pub(super) async fn evaluate_block(block: &Block, context: &mut Context) -> CompletionResult {
    for statement in &block.statements {
        match evaluate_statement(statement, context).await? {
            Completion::Normal(_) => (),
            control => return Ok(control),
        }
    }
    Ok(Completion::Normal(context.temporary(Literal::None)?))
}

pub(super) async fn evaluate_for(
    binding: &str,
    iterable: &Expr,
    body: &Block,
    context: &mut Context,
) -> CompletionResult {
    let iterable_value = evaluate_expression(iterable, context).await?;
    if !matches!(&*iterable_value, Literal::Array(_)) {
        return Err(context.detail_error(
            BWErr::OperationIncompatibleError,
            "For requires an array to iterate over",
            Some(&iterable.span),
            true,
        ));
    }
    let (iterable_value, _iterable_reservation) = iterable_value.into_parts();
    let Literal::Array(values) = iterable_value else {
        unreachable!()
    };
    let previous = context.frames[context.current]
        .variables
        .remove_entry(binding);
    let mut iterator_name = previous.as_ref().map(|(name, _)| name.clone());
    let result = (async {
        for value in values {
            tick(context).await?;
            let stored = context.store_value(value)?;
            if iterator_name.is_none() {
                iterator_name = Some(context.retain_name(binding)?);
            }
            context.frames[context.current]
                .variables
                .insert(iterator_name.as_ref().unwrap().clone(), stored);
            match evaluate_block(body, context).await? {
                Completion::Normal(_) | Completion::Continue => (),
                Completion::Break => break,
                returned @ Completion::Return(_) => return Ok(returned),
            }
        }
        Ok(Completion::Normal(context.temporary(Literal::None)?))
    })
    .await;
    if let Some((name, value)) = previous {
        context.frames[context.current]
            .variables
            .insert(name, value);
    } else {
        context.frames[context.current].variables.remove(binding);
    }
    result
}

pub(super) async fn evaluate_while(
    condition: &Expr,
    body: &Block,
    context: &mut Context,
) -> CompletionResult {
    loop {
        let value = evaluate_expression(condition, context).await?;
        let Literal::Bool(should_loop) = &*value else {
            return Err(context.detail_error(
                BWErr::OperationIncompatibleError,
                "While requires a boolean condition",
                Some(&condition.span),
                true,
            ));
        };
        let should_loop = *should_loop;
        drop(value);
        if !should_loop {
            break;
        }
        match evaluate_block(body, context).await? {
            Completion::Normal(_) | Completion::Continue => (),
            Completion::Break => break,
            returned @ Completion::Return(_) => return Ok(returned),
        }
    }
    Ok(Completion::Normal(context.temporary(Literal::None)?))
}

pub(super) async fn evaluate_handler(
    binding: Option<&Name>,
    handler: &Block,
    original: RuntimeDiagnostic,
    context: &mut Context,
) -> CompletionResult {
    let owner = context.current;
    let original = context.retain_handler(original)?;
    let previous = if let Some(name) = binding {
        let installed = (|| {
            context.checkpoint()?;
            let limits = context.limits();
            let limits = limits.diagnostic_values.intersect(&limits.values);
            let size = original
                .value
                .value_size_with_limits(&limits)
                .map_err(|error| context.retain_limit(error))?;
            let reservation = context.temporary_reservation(size)?;
            let value = TemporaryValue::new(original.value.to_value(), reservation);
            let (value, _reservation) = value.into_parts();
            context.set_variable(&name.text, value)
        })();
        match installed {
            Ok(previous) => previous,
            Err(error) => {
                return Err(context.finish_handler_error(
                    context.runtime_diagnostic(error.into(), Some(&name.span), false),
                    original,
                ))
            }
        }
    } else {
        None
    };
    let handler_name = binding.map(|name| {
        context.frames[owner]
            .variables
            .get_key_value(name.text.as_str())
            .expect("installed handler binding")
            .0
            .clone()
    });
    context.handlers.push(HandledError {
        invocation: owner,
        diagnostic: original.clone(),
    });
    let result = evaluate_block(handler, context).await;
    context.handlers.pop();
    let result = result.map_err(|error| context.finish_handler_error(error, original));
    if let Some(name) = binding {
        // Reservation counters never participate in key equality or hashing.
        #[allow(clippy::mutable_key_type)]
        let variables = &mut context.frames[owner].variables;
        if let Some(value) = previous {
            variables.insert(handler_name.expect("handler name"), value);
        } else {
            variables.remove(name.text.as_str());
        }
    }
    result
}

pub(super) fn evaluate_statement<'a>(
    statement: &'a Statement,
    context: &'a mut Context,
) -> EvalFuture<'a, Completion> {
    Box::pin(async move {
        let _depth = context.enter_evaluation().map_err(|error| {
            context.runtime_diagnostic(error.into(), Some(&statement.span), false)
        })?;
        evaluate_statement_inner(statement, context)
            .await
            .map_err(|error| context.runtime_diagnostic(error, Some(&statement.span), false))
    })
}

// Declaration admission runs only for definitions; its scratch state must not
// increase every active import/control frame's stack requirements.
#[inline(never)]
fn evaluate_definition(definition: &Arc<Definition>, context: &mut Context) -> CompletionResult {
    context.check_statement_collision(&definition.signature, &definition.span)?;
    let reservation = context
        .budget
        .as_ref()
        .map(|budget| budget.reserve_definition(definition, |failure| context.ast_error(failure)))
        .transpose()
        .inspect_err(|error| context.latch_limit(error))?;
    let registry = context.reserve_registry(RegistryPlan::definition(definition))?;
    context.insert_statement(
        &definition.signature,
        &definition.span,
        StmtType::UserDefined {
            definition: Arc::clone(definition),
            metadata: Arc::new(definition.signature_metadata()),
            _reservation: reservation,
            _registry: registry,
        },
    )?;
    Ok(Completion::Normal(context.temporary(Literal::None)?))
}

/// Owned cleanup and polling share one boxed await site: every await arm in the
/// recursive evaluator enlarges its unoptimized frame at each nesting level.
fn boxed_statement<'a>(
    statement: &'a Statement,
    context: &'a mut Context,
) -> EvalFuture<'a, Completion> {
    match &statement.kind {
        StatementKind::Finally { body, cleanup } => {
            super::cleanup::evaluate(body, cleanup, context)
        }
        StatementKind::Poll { .. } => super::polling::evaluate(statement, context),
        _ => unreachable!("only owned cleanup and polling statements are boxed here"),
    }
}

pub(super) async fn evaluate_statement_inner(
    statement: &Statement,
    context: &mut Context,
) -> CompletionResult {
    tick(context).await?;
    match &statement.kind {
        StatementKind::Assign { name, value } => {
            let value = match value {
                AssignmentValue::Expression(expression) => {
                    evaluate_expression(expression, context).await?
                }
                AssignmentValue::Call(call) => invoke(call, context).await?,
            };
            // Reserve the stored copy before cloning its potentially large payload.
            let reservation = context.reserve_value(&value)?;
            let name = context.variable_key(&name.text, context.current)?;
            let stored = Arc::new(StoredValue::new(value.clone(), reservation));
            context.frames[context.current]
                .variables
                .insert(name, stored);
            Ok(Completion::Normal(value))
        }
        StatementKind::Define(definition) => evaluate_definition(definition, context),
        StatementKind::Invoke(call) => invoke(call, context).await.map(Completion::Normal),
        StatementKind::Import {
            path,
            path_span,
            namespace,
        } => imports::evaluate_import(path, path_span, namespace, &statement.span, context)
            .await
            .and_then(|value| context.temporary(value))
            .map(Completion::Normal),
        StatementKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let value = evaluate_expression(condition, context).await?;
            let Literal::Bool(condition) = &*value else {
                return Err(context.detail_error(
                    BWErr::OperationIncompatibleError,
                    "If requires a boolean condition",
                    Some(&condition.span),
                    true,
                ));
            };
            let condition = *condition;
            drop(value);
            if condition {
                evaluate_block(then_branch, context).await
            } else {
                match else_branch {
                    Some(ElseBranch::Block(block)) => evaluate_block(block, context).await,
                    Some(ElseBranch::If(statement)) => evaluate_statement(statement, context).await,
                    None => Ok(Completion::Normal(context.temporary(Literal::None)?)),
                }
            }
        }
        StatementKind::For {
            binding,
            iterable,
            body,
        } => evaluate_for(&binding.text, iterable, body, context).await,
        StatementKind::While { condition, body } => evaluate_while(condition, body, context).await,
        StatementKind::Finally { .. } | StatementKind::Poll { .. } => {
            boxed_statement(statement, context).await
        }
        StatementKind::Try {
            body,
            binding,
            handler,
        } => match evaluate_block(body, context).await {
            Ok(value) => Ok(value),
            Err(original) => {
                let original = context
                    .after_evaluation::<Literal>(Err(original))
                    .unwrap_err();
                if context.checkpoint().is_err() {
                    Err(original)
                } else {
                    evaluate_handler(binding.as_ref(), handler, original, context).await
                }
            }
        },
        StatementKind::Return(expression) => {
            let value = match expression {
                Some(expression) => evaluate_expression(expression, context).await?,
                None => context.temporary(Literal::None)?,
            };
            Ok(Completion::Return(value))
        }
        StatementKind::Break => Ok(Completion::Break),
        StatementKind::Continue => Ok(Completion::Continue),
        StatementKind::Rethrow => {
            match context
                .handlers
                .last()
                .filter(|handler| handler.invocation == context.current)
            {
                Some(handler) => Err(context.rethrow_handler(&handler.diagnostic, &statement.span)),
                None => Err(context.detail_error(
                    BWErr::ControlFlowError,
                    "Rethrow requires an enclosing Catch in the same invocation",
                    Some(&statement.span),
                    false,
                )),
            }
        }
    }
}

/// Record a root program's top-level statement when recording is enabled. The
/// ordinary path returns the statement's own boxed future, so programs, including
/// imported modules, keep one await site and no recorder state in their frames.
fn top_level_statement<'a>(
    statement: &'a Statement,
    context: &'a mut Context,
) -> EvalFuture<'a, Completion> {
    // Imported modules run with a nonempty loading stack; only the run's own
    // program records statements, while module logs still reach the recorder.
    let Some(recorder) = context
        .environment
        .as_ref()
        .and_then(|environment| environment.recorder.clone())
        .filter(|_| context.loading.is_empty())
    else {
        return evaluate_statement(statement, context);
    };
    Box::pin(async move {
        recorder.statement_started(statement.kind_name(), &statement.span);
        let completion = evaluate_statement(statement, context).await;
        recorder.statement_finished(completion.as_ref().err().map(|error| &**error));
        completion
    })
}

pub(super) async fn execute_statement_runtime(
    statement: &Statement,
    context: &mut Context,
) -> EvaluationResult<Literal> {
    let result = (async {
        context.check_execution_mode()?;
        let statements = std::slice::from_ref(statement);
        let limits = context.limits();
        crate::core::ast_limits::check_statements(statements, &limits.ast, limits.source_bytes)
            .map_err(|failure| context.ast_error(failure))?;
        ast::validate_control_script(statements).map_err(|failure| context.ast_error(failure))?;
        finish_script(
            evaluate_statement(statement, context).await?,
            context,
            &statement.span,
        )
        .map(TemporaryValue::into_inner)
    })
    .await;
    result.map_err(|error| context.runtime_diagnostic(error, None, false))
}

pub(crate) async fn evaluate_program_runtime(
    program: &Program,
    context: &mut Context,
) -> EvaluationResult<Literal> {
    let result = (async {
        context.check_execution_mode()?;
        let limits = context.limits();
        program
            .validate_with_reporter(&limits.ast, limits.source_bytes, |failure| {
                context.ast_error(failure)
            })
            .inspect_err(|error| context.latch_limit(error))?;
        let mut result = None;
        for statement in &program.statements {
            // A replaced script result is unobservable once the next statement starts.
            drop(result.take());
            context.trace_statement(statement, &program.source).await?;
            result = Some(finish_script(
                top_level_statement(statement, context).await?,
                context,
                &statement.span,
            )?);
        }
        result
            .map_or_else(|| context.temporary(Literal::None), Ok)
            .map(TemporaryValue::into_inner)
    })
    .await;
    result.map_err(|error| context.runtime_diagnostic(error, None, false))
}
