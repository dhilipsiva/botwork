//! `--format` rewrites files in canonical layout; `--format-check` only reports
//! files whose layout would change.
use super::{CliError, Context};
use botwork::core::{
    format::{format, SourceKind},
    syntax_limits::DEFAULT_SOURCE_BYTES,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

fn read(path: &Path) -> Result<String, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| {
            file.take(DEFAULT_SOURCE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > DEFAULT_SOURCE_BYTES {
        return Err(format!(
            "{}: larger than {DEFAULT_SOURCE_BYTES} bytes",
            path.display()
        ));
    }
    String::from_utf8(bytes).map_err(|error| format!("{}: {error}", path.display()))
}

/// Replace `path` with `text` by writing a sibling file and renaming it over the
/// original, keeping the original's permissions. A failure leaves the original.
fn replace(path: &Path, text: &str) -> io::Result<()> {
    let target = fs::canonicalize(path)?;
    let directory = target.parent().unwrap_or(Path::new("/"));
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let permissions = fs::metadata(&target)?.permissions();
    for attempt in 0..64 {
        let temporary = directory.join(format!(".{name}.{}.{attempt}.format", std::process::id()));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let written = file
            .write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .and_then(|()| fs::set_permissions(&temporary, permissions.clone()))
            .and_then(|()| fs::rename(&temporary, &target));
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return written;
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free temporary name",
    ))
}

/// Format each file. With `check`, nothing is written and every file that would
/// change is listed. Unparseable and unreadable files are errors and are never
/// written.
pub(super) fn run(files: &[PathBuf], check: bool) -> Result<(), CliError> {
    let context = Context::default();
    let mut stderr = io::stderr().lock();
    let (mut changed, mut errors) = (0usize, 0usize);
    for path in files {
        let name = path.display().to_string();
        let formatted = read(path).and_then(|text| {
            format(&name, &text, SourceKind::of_path(path))
                .map(|formatted| (formatted != text).then_some(formatted))
                .map_err(|error| error.to_string())
        });
        match formatted {
            Ok(None) => {}
            Ok(Some(formatted)) => {
                changed += 1;
                if check {
                    context.write_output(&mut stderr, format_args!("would reformat {name}\n"))?;
                } else if let Err(error) = replace(path, &formatted) {
                    errors += 1;
                    context.write_output(
                        &mut stderr,
                        format_args!("{name}: writing failed: {error}\n"),
                    )?;
                } else {
                    context.write_output(&mut stderr, format_args!("reformatted {name}\n"))?;
                }
            }
            Err(error) => {
                errors += 1;
                context.write_output(&mut stderr, format_args!("{error}\n"))?;
            }
        }
    }
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    let verb = if check { "would change" } else { "reformatted" };
    context.write_output(
        &mut stderr,
        format_args!(
            "[format] {} file{}: {changed} {verb}, {errors} error{}\n",
            files.len(),
            plural(files.len()),
            plural(errors)
        ),
    )?;
    if errors != 0 || (check && changed != 0) {
        Err(CliError::Checked)
    } else {
        Ok(())
    }
}
