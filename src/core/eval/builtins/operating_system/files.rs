use super::*;
use std::io::{Read, Write};

mod read;

const CHUNK: usize = 16 * 1024;

pub(super) fn invoke(op: OsOp, arguments: &[TemporaryValue], context: &Context) -> TemporaryResult {
    let path = paths::resolve(context, text(&arguments[0]), false)?;
    let path = if matches!(op, OsOp::Exists | OsOp::Kind | OsOp::RemoveDirectory) {
        paths::entry(&path)
    } else {
        path
    };
    if matches!(op, OsOp::RemoveDirectory) {
        paths::validate_removal(context, &path)?;
    }
    match op {
        OsOp::Canonical => {
            return paths::output(
                context,
                &fs::canonicalize(&path)
                    .map_err(|error| io_error(context, "Canonicalize", &path, error))?,
            )
        }
        OsOp::Exists | OsOp::Kind | OsOp::FileExists | OsOp::DirectoryExists => {
            return inspect(context, op, &path)
        }
        OsOp::Size => {
            let metadata =
                fs::metadata(&path).map_err(|error| io_error(context, "Stat", &path, error))?;
            regular(context, &path, &metadata)?;
            return strings::formatted(context, |out| write!(out, "{}", metadata.len()));
        }
        OsOp::Read | OsOp::ReadBinary => {
            return read::file(context, &path, matches!(op, OsOp::ReadBinary))
        }
        OsOp::List => return list(context, &path),
        OsOp::TemporaryDirectory => return temporary(context, &path, text(&arguments[1])),
        _ => (),
    }
    if matches!(op, OsOp::WriteBinary) {
        validate_bytes(context, &arguments[1])?;
    }
    let destination = if matches!(op, OsOp::Move | OsOp::Copy) {
        Some(paths::resolve(context, text(&arguments[1]), false)?)
    } else {
        None
    };
    // Even a None result consumes a node and a live value. Admit before effects.
    let result = context.temporary(Literal::None)?;
    context.checkpoint()?;
    match op {
        OsOp::Write | OsOp::Append | OsOp::Create | OsOp::WriteBinary => {
            let mut options = fs::OpenOptions::new();
            options.write(true);
            match op {
                OsOp::Append => {
                    options.append(true).create(true);
                }
                OsOp::Create => {
                    options.create_new(true);
                }
                _ => {
                    options.create(true);
                }
            }
            let mut file = options
                .open(&path)
                .map_err(|error| io_error(context, "Open for writing", &path, error))?;
            regular(
                context,
                &path,
                &file
                    .metadata()
                    .map_err(|error| io_error(context, "Stat open file", &path, error))?,
            )?;
            context.checkpoint()?;
            if matches!(op, OsOp::Write | OsOp::WriteBinary) {
                file.set_len(0)
                    .map_err(|error| io_error(context, "Truncate", &path, error))?;
            }
            if matches!(op, OsOp::WriteBinary) {
                let Literal::Array(bytes) = &*arguments[1] else {
                    unreachable!("validated Array")
                };
                let mut buffer = [0u8; CHUNK];
                for chunk in bytes.chunks(CHUNK) {
                    for (out, value) in buffer.iter_mut().zip(chunk) {
                        let Literal::Int(value) = value else {
                            unreachable!("validated byte")
                        };
                        *out = *value as u8;
                    }
                    write_all(context, &mut file, &path, &buffer[..chunk.len()])?;
                }
            } else {
                write_all(context, &mut file, &path, text(&arguments[1]).as_bytes())?;
            }
        }
        OsOp::Copy => {
            let destination = destination.as_ref().unwrap();
            let mut source = fs::File::open(&path)
                .map_err(|error| io_error(context, "Open for copying", &path, error))?;
            regular(
                context,
                &path,
                &source
                    .metadata()
                    .map_err(|error| io_error(context, "Stat open file", &path, error))?,
            )?;
            context.checkpoint()?;
            let mut target = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)
                .map_err(|error| {
                    io_error(context, "Create copy destination", destination, error)
                })?;
            let mut buffer = [0u8; CHUNK];
            loop {
                let count = read_chunk(context, &mut source, &path, &mut buffer)?;
                if count == 0 {
                    break;
                }
                write_all(context, &mut target, destination, &buffer[..count])?;
            }
        }
        OsOp::Move => fs::rename(&path, destination.as_ref().unwrap())
            .map_err(|error| io_error(context, "Rename source", &path, error))?,
        OsOp::RemoveFile => ignore_missing(context, "Remove file", &path, fs::remove_file(&path))?,
        OsOp::CreateDirectory => fs::create_dir_all(&path)
            .map_err(|error| io_error(context, "Create directory", &path, error))?,
        OsOp::RemoveDirectory => {
            let recursive = matches!(&*arguments[1], Literal::Bool(true));
            let removal = if recursive {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_dir(&path)
            };
            ignore_missing(context, "Remove directory", &path, removal)?;
        }
        _ => unreachable!("filesystem operation"),
    }
    context.checkpoint()?;
    Ok(result)
}

fn validate_bytes(context: &Context, value: &Literal) -> EvaluationResult<()> {
    let Literal::Array(bytes) = value else {
        unreachable!("validated Array")
    };
    for byte in bytes {
        context.checkpoint()?;
        if !matches!(byte, Literal::Int(value) if (0..=255).contains(value)) {
            return Err(invalid(
                context,
                "Binary file bytes must be Int values from 0 through 255",
            ));
        }
    }
    Ok(())
}

fn regular(context: &Context, path: &Path, metadata: &fs::Metadata) -> EvaluationResult<()> {
    if metadata.is_file() {
        return Ok(());
    }
    Err(context.formatted_error(
        BWErr::NativeError,
        format_args!("Expected a regular file: {}", path.display()),
        None,
        false,
    ))
}

fn inspect(context: &Context, op: OsOp, path: &Path) -> TemporaryResult {
    let result = if matches!(op, OsOp::FileExists | OsOp::DirectoryExists) {
        fs::metadata(path)
    } else {
        fs::symlink_metadata(path)
    };
    let metadata = match result {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_error(context, "Inspect", path, error)),
    };
    match op {
        OsOp::Kind => context.temporary_string(match metadata {
            None => "missing",
            Some(meta) if meta.file_type().is_symlink() => "symlink",
            Some(meta) if meta.is_file() => "file",
            Some(meta) if meta.is_dir() => "directory",
            _ => "other",
        }),
        _ => context.temporary(Literal::Bool(match op {
            OsOp::Exists => metadata.is_some(),
            OsOp::FileExists => metadata.is_some_and(|meta| meta.is_file()),
            OsOp::DirectoryExists => metadata.is_some_and(|meta| meta.is_dir()),
            _ => unreachable!(),
        })),
    }
}

fn list(context: &Context, path: &Path) -> TemporaryResult {
    let mut result = values::StringArray::new(context)?;
    let entries =
        fs::read_dir(path).map_err(|error| io_error(context, "List directory", path, error))?;
    for entry in entries {
        context.checkpoint()?;
        let entry =
            entry.map_err(|error| io_error(context, "Read directory entry", path, error))?;
        // read_dir owns one OS entry/name; retain no result copy before admission.
        result.push(&entry.file_name())?;
    }
    context.checkpoint()?;
    Ok(result.sorted())
}

fn temporary(context: &Context, directory: &Path, prefix: &str) -> TemporaryResult {
    if prefix.len() > 64
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid(context, "Temporary directory prefix must have at most 64 ASCII letters, digits, dashes, or underscores"));
    }
    let mut expected = directory.to_owned();
    paths::push(
        context,
        &mut expected,
        Path::new(&format!("{prefix}xxxxxxxxxxxx")),
    )?;
    let size = admitted(
        context,
        context
            .limits()
            .values
            .string_size(utf8(context, expected.as_os_str())?.len()),
    )?;
    let reservation = context.temporary_reservation(size)?;
    context.checkpoint()?;
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix).rand_bytes(12);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let directory = builder
        .tempdir_in(directory)
        .map_err(|error| io_error(context, "Create temporary directory in", directory, error))?;
    context.checkpoint()?;
    let path = utf8(context, directory.path().as_os_str())?;
    if path.len() != size.payload_bytes {
        return Err(invalid(
            context,
            "Temporary path size differs from its admitted plan",
        ));
    }
    let value = Literal::String(path.into());
    let _ = directory.keep();
    Ok(TemporaryValue::new(value, reservation))
}

fn ignore_missing(
    context: &Context,
    action: &str,
    path: &Path,
    result: io::Result<()>,
) -> EvaluationResult<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(context, action, path, error)),
    }
}

fn read_chunk(
    context: &Context,
    input: &mut impl Read,
    path: &Path,
    buffer: &mut [u8],
) -> EvaluationResult<usize> {
    loop {
        context.checkpoint()?;
        let count = match input.read(buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result.map_err(|error| io_error(context, "Read", path, error))?,
        };
        context.checkpoint()?;
        return Ok(count);
    }
}

fn write_all(
    context: &Context,
    output: &mut impl Write,
    path: &Path,
    mut bytes: &[u8],
) -> EvaluationResult<()> {
    while !bytes.is_empty() {
        context.checkpoint()?;
        let count = match output.write(&bytes[..bytes.len().min(CHUNK)]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result.map_err(|error| io_error(context, "Write", path, error))?,
        };
        if count == 0 {
            return Err(io_error(
                context,
                "Write",
                path,
                io::Error::from(io::ErrorKind::WriteZero),
            ));
        }
        bytes = &bytes[count..];
    }
    context.checkpoint()?;
    Ok(())
}

#[cfg(test)]
mod tests;
