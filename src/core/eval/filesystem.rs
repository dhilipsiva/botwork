use super::*;
use crate::core::run::blocking_io;
use std::path::Path;

impl Context {
    fn filesystem_control(&self) -> OperationControl {
        self.budget
            .as_ref()
            .map(|budget| budget.control().clone())
            .unwrap_or_default()
    }

    pub(crate) async fn read_source_bytes(
        &mut self,
        path: &Path,
        maximum: usize,
    ) -> Result<Vec<u8>, SourceFailure> {
        self.checkpoint().map_err(SourceFailure::Diagnostic)?;
        let result = if self.asynchronous {
            let path = path.to_owned();
            blocking_io::run(self.filesystem_control(), move |control| {
                blocking_io::read(&path, maximum, control)
            })
            .await
        } else {
            super::super::run::read_source(path, Some(maximum))
        };
        let bytes = result?;
        self.checkpoint().map_err(SourceFailure::Diagnostic)?;
        Ok(bytes)
    }

    pub(crate) async fn read_source_async(&mut self, path: &Path) -> Result<String, SourceFailure> {
        let bytes = self
            .read_source_bytes(path, self.limits().source_bytes)
            .await?;
        self.check_source_size(bytes.len())
            .map_err(SourceFailure::Diagnostic)?;
        String::from_utf8(bytes)
            .map_err(|error| SourceFailure::Io(io::Error::new(io::ErrorKind::InvalidData, error)))
    }

    pub(super) async fn canonicalize_source(
        &mut self,
        path: &Path,
    ) -> Result<PathBuf, SourceFailure> {
        self.checkpoint().map_err(SourceFailure::Diagnostic)?;
        let canonical = if self.asynchronous {
            let path = path.to_owned();
            blocking_io::run(self.filesystem_control(), move |_| {
                crate::core::paths::canonicalize(path).map_err(SourceFailure::Io)
            })
            .await?
        } else {
            crate::core::paths::canonicalize(path).map_err(SourceFailure::Io)?
        };
        self.checkpoint().map_err(SourceFailure::Diagnostic)?;
        Ok(canonical)
    }

    /// The package project around `directory`, read off the runtime's
    /// threads in asynchronous runs: the nearest `botwork.toml` above it and
    /// its lockfile.
    pub(super) async fn load_project(
        &mut self,
        directory: &Path,
    ) -> Result<crate::core::packages::Project, SourceFailure> {
        self.checkpoint().map_err(SourceFailure::Diagnostic)?;
        let load = |directory: &Path| {
            let root = crate::core::packages::Project::find(directory).ok_or_else(|| {
                format!(
                    "`@` imports need a {} at or above {}",
                    crate::core::packages::MANIFEST,
                    directory.display()
                )
            })?;
            crate::core::packages::Project::load(&root, None)
        };
        let project = if self.asynchronous {
            let directory = directory.to_owned();
            blocking_io::run(self.filesystem_control(), move |_| {
                load(&directory).map_err(|reason| SourceFailure::Io(std::io::Error::other(reason)))
            })
            .await?
        } else {
            load(directory).map_err(|reason| SourceFailure::Io(std::io::Error::other(reason)))?
        };
        self.checkpoint().map_err(SourceFailure::Diagnostic)?;
        Ok(project)
    }
}
