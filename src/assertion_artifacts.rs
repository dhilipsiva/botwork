//! Optional assertion evidence files. A fresh directory owns one invocation's outputs.
use super::{BWErr, CliError, Diagnostic};
use serde::Serialize;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Copy)]
struct Limits {
    records: usize,
    bytes: usize,
    source_scan: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            records: 256,
            bytes: 64 * 1024 * 1024,
            source_scan: 64 * 1024 * 1024,
        }
    }
}
#[derive(Default)]
struct Usage {
    records: usize,
    bytes: usize,
    source_scan: usize,
}
pub(super) struct Store {
    directory: PathBuf,
    limits: Limits,
    usage: Mutex<Usage>,
}

#[derive(Serialize)]
pub(super) struct Identity<'a> {
    pub run: usize,
    pub file: &'a Path,
    pub case: Option<&'a str>,
    pub name: Option<&'a str>,
    pub dataset: Option<&'a str>,
    pub row: Option<&'a str>,
    pub suite_fixture: Option<&'a str>,
}
#[derive(Serialize)]
struct Location<'a> {
    file: &'a str,
    start_byte: usize,
    end_byte: usize,
    line: usize,
    column: usize,
    end_line: usize,
    end_column: usize,
}
#[derive(Serialize)]
struct OmittedLocation<'a> {
    file: &'a str,
    file_truncated: bool,
    start_byte: usize,
    end_byte: usize,
}
#[derive(Serialize)]
struct Record<'a> {
    format: &'static str,
    version: u32,
    identity: &'a Identity<'a>,
    cause_path: &'a [usize],
    code: &'static str,
    source: Option<Location<'a>>,
    omitted_source: Option<OmittedLocation<'a>>,
    reason: &'a str,
    /// Strings containing complete typed JSON, present only when operands are complete.
    actual_typed_json: Option<&'a str>,
    expected_typed_json: Option<&'a str>,
    /// Emergency previews or prefixes; never typed JSON values.
    actual_excerpt: Option<&'a str>,
    expected_excerpt: Option<&'a str>,
    operands_complete: bool,
    diagnostic_omissions: bool,
}

fn exceeded(resource: &str) -> io::Error {
    io::Error::other(format!("assertion artifact {resource} limit exceeded"))
}

struct Charged<'a> {
    file: &'a mut fs::File,
    used: &'a mut usize,
    maximum: usize,
}
impl Write for Charged<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() > self.maximum.saturating_sub(*self.used) {
            return Err(exceeded("total bytes"));
        }
        let written = self.file.write(data)?;
        *self.used += written;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Store {
    pub(super) fn new(parent: &Path) -> io::Result<Self> {
        fs::create_dir_all(parent)?;
        let mut builder = tempfile::Builder::new();
        builder.prefix("botwork-assertions-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(fs::Permissions::from_mode(0o700));
        }
        let directory = builder.tempdir_in(parent)?.keep();
        Ok(Self {
            directory,
            limits: Limits::default(),
            usage: Mutex::new(Usage::default()),
        })
    }

    pub(super) fn write_error(
        &self,
        error: &CliError,
        identity: &Identity<'_>,
    ) -> io::Result<Vec<PathBuf>> {
        let diagnostic = match error {
            CliError::Script(value) => value,
            CliError::SourceLimit { source, .. } => source,
            _ => return Ok(Vec::new()),
        };
        let mut paths = Vec::new();
        self.visit(diagnostic, identity, &mut Vec::new(), &mut paths)?;
        Ok(paths)
    }

    fn visit(
        &self,
        diagnostic: &Diagnostic,
        identity: &Identity<'_>,
        cause_path: &mut Vec<usize>,
        paths: &mut Vec<PathBuf>,
    ) -> io::Result<()> {
        if cause_path.len() > botwork::core::diagnostic::MAX_DIAGNOSTIC_DEPTH {
            return Err(exceeded("depth"));
        }
        let evidence = match &*diagnostic.error {
            BWErr::AssertionMismatch {
                reason,
                actual,
                expected,
            } => Some((
                reason.as_str(),
                Some(actual.as_str()),
                Some(expected.as_str()),
            )),
            BWErr::AssertionFailed(reason) => Some((reason.as_str(), None, None)),
            _ => None,
        };
        if let Some((reason, actual, expected)) = evidence {
            let mut used = self
                .usage
                .lock()
                .map_err(|_| io::Error::other("assertion artifact writer poisoned"))?;
            if used.records >= self.limits.records {
                return Err(exceeded("record count"));
            }
            used.records += 1;
            let source = if let Some(span) = &diagnostic.span {
                let work = span
                    .start()
                    .checked_add(span.end())
                    .and_then(|value| value.checked_mul(6))
                    .ok_or_else(|| exceeded("source scan"))?;
                used.source_scan = used
                    .source_scan
                    .checked_add(work)
                    .filter(|value| *value <= self.limits.source_scan)
                    .ok_or_else(|| exceeded("source scan"))?;
                let (line, column) = span.line_column();
                let (end_line, end_column) = span.end_line_column();
                Some(Location {
                    file: span.source().name(),
                    start_byte: span.start(),
                    end_byte: span.end(),
                    line,
                    column,
                    end_line,
                    end_column,
                })
            } else {
                None
            };
            let complete = actual.is_some() && expected.is_some() && diagnostic.omissions.is_none();
            let record = Record {
                format: "botwork-assertion",
                version: 1,
                identity,
                cause_path,
                code: diagnostic.code().as_str(),
                source,
                omitted_source: diagnostic
                    .omissions
                    .as_ref()
                    .and_then(|omitted| omitted.source.as_ref())
                    .map(|source| OmittedLocation {
                        file: &source.file,
                        file_truncated: source.file_truncated,
                        start_byte: source.start_byte,
                        end_byte: source.end_byte,
                    }),
                reason,
                actual_typed_json: actual.filter(|_| complete),
                expected_typed_json: expected.filter(|_| complete),
                actual_excerpt: actual.filter(|_| !complete),
                expected_excerpt: expected.filter(|_| !complete),
                operands_complete: complete,
                diagnostic_omissions: diagnostic.omissions.is_some(),
            };
            let destination = self
                .directory
                .join(format!("assertion-{:06}.json", used.records));
            let mut pending = tempfile::NamedTempFile::new_in(&self.directory)?;
            let mut writer = Charged {
                file: pending.as_file_mut(),
                used: &mut used.bytes,
                maximum: self.limits.bytes,
            };
            serde_json::to_writer_pretty(&mut writer, &record).map_err(io::Error::other)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
            pending
                .persist_noclobber(&destination)
                .map_err(|error| error.error)?;
            paths.push(destination);
        }
        for (index, cause) in diagnostic.causes.iter().enumerate() {
            cause_path.push(index);
            self.visit(cause, identity, cause_path, paths)?;
            cause_path.pop();
        }
        Ok(())
    }
}

pub(super) fn write_single(
    result: Result<(), CliError>,
    store: Option<&Store>,
    file: &Path,
) -> Result<(), CliError> {
    let Err(error) = result else {
        return result;
    };
    if let Some(store) = store {
        let identity = Identity {
            run: 1,
            file,
            case: None,
            name: None,
            dataset: None,
            row: None,
            suite_fixture: None,
        };
        let paths = store
            .write_error(&error, &identity)
            .map_err(|source| CliError::Artifact {
                source,
                original: error.to_string(),
            })?;
        let context = super::Context::default();
        for path in paths {
            context.write_output(
                &mut io::stderr().lock(),
                format_args!("[assertion artifact] {path:?}\n"),
            )?;
        }
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use botwork::core::run::{Engine, RunOptions};

    fn failure() -> CliError {
        Engine::default()
            .run_source("artifact.botwork", "Assert |false|", RunOptions::default())
            .result
            .unwrap_err()
            .into()
    }
    fn identity() -> Identity<'static> {
        Identity {
            run: 1,
            file: Path::new("artifact.botwork"),
            case: None,
            name: None,
            dataset: None,
            row: None,
            suite_fixture: None,
        }
    }

    #[test]
    fn quotas_remove_partial_files_and_preserve_the_original_failure() {
        for limits in [
            Limits {
                records: 0,
                ..Limits::default()
            },
            Limits {
                bytes: 1,
                ..Limits::default()
            },
            Limits {
                source_scan: 0,
                ..Limits::default()
            },
        ] {
            let parent = tempfile::tempdir().unwrap();
            let mut store = Store::new(parent.path()).unwrap();
            store.limits = limits;
            let error = write_single(Err(failure()), Some(&store), Path::new("artifact.botwork"))
                .unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains("Writing assertion artifacts failed")
                    && message.contains("BW9001"),
                "{message}"
            );
            assert_eq!(fs::read_dir(&store.directory).unwrap().count(), 0);
        }
    }

    #[test]
    fn exclusive_publication_preserves_existing_files_and_uses_a_new_number() {
        let parent = tempfile::tempdir().unwrap();
        let store = Store::new(parent.path()).unwrap();
        let original = store.directory.join("assertion-000001.json");
        fs::write(&original, "preserve").unwrap();
        assert!(store.write_error(&failure(), &identity()).is_err());
        assert_eq!(fs::read_to_string(&original).unwrap(), "preserve");
        let paths = store.write_error(&failure(), &identity()).unwrap();
        assert_eq!(paths[0].file_name().unwrap(), "assertion-000002.json");
        assert_eq!(fs::read_dir(&store.directory).unwrap().count(), 2);
    }

    #[test]
    fn rejected_evidence_is_explicitly_incomplete() {
        let parent = tempfile::tempdir().unwrap();
        let store = Store::new(parent.path()).unwrap();
        let mut options = RunOptions::default();
        options.limits.diagnostics.text_bytes = 0;
        let error = Engine::default()
            .run_source("tiny.botwork", "Assert |false|", options)
            .result
            .unwrap_err();
        let paths = store
            .write_error(&CliError::Script(error), &identity())
            .unwrap();
        assert_eq!(paths.len(), 1);
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&paths[0]).unwrap()).unwrap();
        assert_eq!(value["operands_complete"], false);
        assert_eq!(value["diagnostic_omissions"], true);
        assert_eq!(value["cause_path"], serde_json::json!([0]));
        // Emergency previews ("false") would parse as JSON, so they never occupy typed fields.
        assert!(value["actual_typed_json"].is_null() && value["expected_typed_json"].is_null());
        assert_eq!(value["actual_excerpt"], "false");
        assert_eq!(value["expected_excerpt"], "true");
    }

    #[test]
    fn complete_operands_use_only_typed_fields() {
        let parent = tempfile::tempdir().unwrap();
        let store = Store::new(parent.path()).unwrap();
        let paths = store.write_error(&failure(), &identity()).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&paths[0]).unwrap()).unwrap();
        assert_eq!(value["operands_complete"], true);
        assert_eq!(
            value["actual_typed_json"],
            r#"{"kind":"Bool","value":false}"#
        );
        assert!(value["actual_excerpt"].is_null() && value["expected_excerpt"].is_null());
    }
}
