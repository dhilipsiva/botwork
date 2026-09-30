//! One bounded discovery owns suite sources and cached, immutable dataset files.
use super::*;
use botwork::core::{
    ast::Span,
    suite::{Dataset, DatasetFormat},
};
use std::{
    collections::{HashMap, HashSet},
    fs::OpenOptions,
    path::Path,
};

#[derive(Default)]
pub(super) struct Discovery {
    bytes: usize,
    // One file can be declared in different formats; each parse is cached separately.
    cache: HashMap<(PathBuf, DatasetFormat), Arc<Dataset>>,
    admitted: HashSet<*const Dataset>,
    rows: usize,
    nodes: usize,
}

impl Discovery {
    pub(super) fn read(&mut self, path: &Path, dataset: bool) -> Result<String, CliError> {
        let maximum =
            super::super::DEFAULT_SOURCE_BYTES.min(suite::MAX_SUITE_SOURCE_BYTES - self.bytes);
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(target_os = "linux")]
        if dataset {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let mut bytes = Vec::new();
        options
            .open(path)
            .and_then(|file| {
                if dataset && !file.metadata()?.is_file() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "dataset input must be an ordinary file",
                    ));
                }
                file.take(maximum as u64 + 1).read_to_end(&mut bytes)
            })
            .map_err(|source| CliError::Read {
                file: path.to_owned(),
                source,
            })?;
        if bytes.len() > maximum {
            return Err(if maximum < super::super::DEFAULT_SOURCE_BYTES {
                suite::resource(
                    "suite discovery source bytes",
                    suite::MAX_SUITE_SOURCE_BYTES,
                )
            } else {
                suite::resource(
                    if dataset {
                        "dataset source bytes"
                    } else {
                        "suite source bytes"
                    },
                    super::super::DEFAULT_SOURCE_BYTES,
                )
            }
            .into());
        }
        self.bytes += bytes.len();
        String::from_utf8(bytes).map_err(|error| CliError::Read {
            file: path.to_owned(),
            source: io::Error::new(io::ErrorKind::InvalidData, error),
        })
    }

    pub(super) fn admit(&mut self, data: &Arc<Dataset>) -> Result<(), Diagnostic> {
        if !self.admitted.insert(Arc::as_ptr(data)) {
            return Ok(());
        }
        self.rows += data.rows().len();
        self.nodes += data.value_nodes();
        if self.admitted.len() > suite::MAX_DATASETS {
            return Err(suite::resource("discovered datasets", suite::MAX_DATASETS));
        }
        if self.rows > suite::MAX_DATA_ROWS {
            return Err(suite::resource("defined data rows", suite::MAX_DATA_ROWS));
        }
        if self.nodes > suite::MAX_DATA_NODES {
            return Err(suite::resource(
                "dataset literal nodes",
                suite::MAX_DATA_NODES,
            ));
        }
        Ok(())
    }

    pub(super) fn load(
        &mut self,
        path: &Path,
        span: &Span,
        format: DatasetFormat,
    ) -> Result<Arc<Dataset>, Diagnostic> {
        let canonical = botwork::core::paths::canonicalize(path).map_err(|error| {
            Diagnostic::new(BWErr::ImportRead(format!(
                "Dataset {}: {error}",
                path.display()
            )))
            .at(span)
        })?;
        let key = (canonical, format);
        if let Some(data) = self.cache.get(&key) {
            return Ok(Arc::clone(data));
        }
        let canonical = &key.0;
        let source = self.read(canonical, true).map_err(|error| match error {
            CliError::Script(error) => error,
            error => Diagnostic::new(BWErr::ImportRead(error.to_string())).at(span),
        })?;
        let data = Arc::new(Dataset::parse_format(
            &canonical.display().to_string(),
            &source,
            format,
        )?);
        self.admit(&data)?;
        self.cache.insert(key, Arc::clone(&data));
        Ok(data)
    }
}
