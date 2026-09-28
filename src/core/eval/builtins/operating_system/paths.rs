use super::*;

#[cfg(test)]
mod tests;

// Separate bounded workspace for OS-owned path construction, before value output.
pub(super) const MAX_PATH_BYTES: usize = 1024 * 1024;

fn bounded(context: &Context, bytes: Option<usize>) -> EvaluationResult<()> {
    if bytes.is_some_and(|bytes| bytes <= MAX_PATH_BYTES) {
        return Ok(());
    }
    admitted(
        context,
        Err(BWErr::ResourceLimit {
            resource: "filesystem path workspace bytes",
            limit: MAX_PATH_BYTES as u64,
        }),
    )
}

pub(super) fn path<'a>(
    context: &Context,
    value: &'a str,
    empty: bool,
) -> EvaluationResult<&'a Path> {
    if value.contains('\0') || !empty && value.is_empty() {
        return Err(invalid(
            context,
            "Paths must not contain NUL; filesystem paths must not be empty",
        ));
    }
    bounded(context, Some(value.len()))?;
    Ok(Path::new(value))
}

pub(super) fn directory(context: &Context) -> EvaluationResult<&Path> {
    if let Some(environment) = &context.environment {
        return Ok(environment.working_directory());
    }
    context.working_directory.as_deref().map_err(|error| {
        context.formatted_error(
            BWErr::NativeError,
            format_args!("Working directory snapshot: {error}"),
            None,
            false,
        )
    })
}

pub(super) fn push(context: &Context, base: &mut PathBuf, part: &Path) -> EvaluationResult<()> {
    let bound = if part.is_absolute() {
        Some(part.as_os_str().len())
    } else {
        base.as_os_str()
            .len()
            .checked_add(part.as_os_str().len())
            .and_then(|n| n.checked_add(1))
    };
    bounded(context, bound)?;
    let bound = bound.expect("admitted path size");
    if bound > base.as_os_str().len() {
        base.reserve_exact(bound - base.as_os_str().len());
    }
    base.push(part);
    Ok(())
}

pub(super) fn resolve(context: &Context, value: &str, empty: bool) -> EvaluationResult<PathBuf> {
    let input = path(context, value, empty)?;
    if input.is_absolute() {
        return Ok(input.to_owned());
    }
    let directory = directory(context)?;
    bounded(context, Some(directory.as_os_str().len()))?;
    let mut absolute = directory.to_owned();
    if input.as_os_str().is_empty() {
        return Ok(absolute);
    }
    push(context, &mut absolute, input)?;
    Ok(absolute)
}

pub(super) fn output(context: &Context, path: &Path) -> TemporaryResult {
    bounded(context, Some(path.as_os_str().len()))?;
    context.temporary_string(utf8(context, path.as_os_str())?)
}

// A trailing separator or dot asks the OS to traverse a symlink as a directory.
// Entry operations must address the final component itself instead. Components
// remove those suffixes without resolving symlinks or collapsing parent entries.
pub(super) fn entry(path: &Path) -> PathBuf {
    let mut entry = PathBuf::with_capacity(path.as_os_str().len());
    entry.extend(path.components());
    entry
}

pub(super) fn validate_removal(context: &Context, path: &Path) -> EvaluationResult<()> {
    if matches!(
        path.components().next_back(),
        Some(std::path::Component::Normal(_))
    ) {
        Ok(())
    } else {
        Err(invalid(
            context,
            "Directory removal requires a final name, not a root or parent component",
        ))
    }
}

pub(super) fn invoke(op: OsOp, arguments: &[TemporaryValue], context: &Context) -> TemporaryResult {
    if matches!(op, OsOp::WorkingDirectory) {
        return output(context, directory(context)?);
    }
    if matches!(op, OsOp::Join) {
        let Literal::Array(parts) = &*arguments[0] else {
            unreachable!("validated Array")
        };
        let mut joined = PathBuf::new();
        for part in parts {
            context.checkpoint()?;
            let Literal::String(part) = part else {
                return Err(invalid(context, "Join Path requires an Array of Strings"));
            };
            push(context, &mut joined, path(context, part, true)?)?;
        }
        return output(context, &joined);
    }
    let input = path(context, text(&arguments[0]), true)?;
    match op {
        OsOp::Absolute => output(context, &resolve(context, text(&arguments[0]), true)?),
        OsOp::IsAbsolute => context.temporary(Literal::Bool(input.is_absolute())),
        OsOp::Components => {
            values::strings(context, input.components().map(|part| part.as_os_str()))
        }
        _ => {
            let result = match op {
                OsOp::Parent => input.parent().map(Path::as_os_str),
                OsOp::Name => input.file_name(),
                OsOp::Extension => input.extension(),
                _ => unreachable!("path operation"),
            };
            match result {
                Some(value) => context.temporary_string(utf8(context, value)?),
                None => context.temporary(Literal::None),
            }
        }
    }
}
