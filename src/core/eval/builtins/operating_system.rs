//! Filesystem effects use the run directory and the existing blocking boundary.
use super::*;
use crate::core::{signature::ValueKinds, value_limits::ValueSize};
use std::{ffi::OsStr, fs, path::Path};

mod environment;
mod files;
mod paths;
mod values;

#[derive(Clone, Copy)]
pub(in crate::core::eval) enum OsOp {
    WorkingDirectory,
    Join,
    Absolute,
    Canonical,
    Parent,
    Name,
    Extension,
    IsAbsolute,
    Exists,
    Kind,
    FileExists,
    DirectoryExists,
    Size,
    Read,
    ReadBinary,
    Write,
    Append,
    WriteBinary,
    Create,
    Copy,
    Move,
    RemoveFile,
    CreateDirectory,
    List,
    RemoveDirectory,
    TemporaryDirectory,
    EnvironmentExists,
    GetEnvironment,
    Environment,
    Platform,
    Separator,
    Components,
}

impl OsOp {
    pub(super) fn needs_worker(self) -> bool {
        matches!(
            self,
            Self::Canonical
                | Self::Exists
                | Self::Kind
                | Self::FileExists
                | Self::DirectoryExists
                | Self::Size
                | Self::Read
                | Self::ReadBinary
                | Self::Write
                | Self::Append
                | Self::WriteBinary
                | Self::Create
                | Self::Copy
                | Self::Move
                | Self::RemoveFile
                | Self::CreateDirectory
                | Self::List
                | Self::RemoveDirectory
                | Self::TemporaryDirectory
        )
    }

    pub(super) fn signature(self) -> StatementSignature {
        let s = Kind::String;
        let optional = ValueKinds::from(s).union(Kind::None.into());
        let (header, description, parameters, result) = match self {
            Self::WorkingDirectory => ("Working Directory", "Return the run's directory snapshot without changing process state.", vec![], s.into()),
            Self::Join => ("Join Path |parts|", "Join an Array of Strings using native path rules; absolute later parts replace the prefix. No shell expansion.", vec![("parts", Kind::Array)], s.into()),
            Self::Absolute => ("Absolute Path |path|", "Resolve against the run directory without accessing the filesystem or collapsing parent components.", vec![("path", s)], s.into()),
            Self::Canonical => ("Canonical Path |path|", "Resolve an existing path and symlinks to the operating system's canonical absolute spelling.", vec![("path", s)], s.into()),
            Self::Parent => ("Parent Path |path|", "Return the lexical parent, including an empty String for a single relative component; None if no parent exists.", vec![("path", s)], optional),
            Self::Name => ("File Name |path|", "Return the final lexical normal component, or None for roots/parent-only paths.", vec![("path", s)], optional),
            Self::Extension => ("File Extension |path|", "Return the final filename extension without its dot; preserve an empty extension; None when absent.", vec![("path", s)], optional),
            Self::IsAbsolute => ("Path Is Absolute |path|", "Test native absolute-path syntax without filesystem access.", vec![("path", s)], Kind::Bool.into()),
            Self::Exists => ("Path Exists |path|", "Test existence without following the final symlink; dangling symlinks exist. Other I/O errors fail.", vec![("path", s)], Kind::Bool.into()),
            Self::Kind => ("Path Kind |path|", "Return file, directory, symlink, other, or missing, without following the final symlink.", vec![("path", s)], s.into()),
            Self::FileExists => ("File Exists |path|", "Test for a regular file, following symlinks. Missing targets return false; other I/O errors fail.", vec![("path", s)], Kind::Bool.into()),
            Self::DirectoryExists => ("Directory Exists |path|", "Test for a directory, following symlinks. Missing targets return false; other I/O errors fail.", vec![("path", s)], Kind::Bool.into()),
            Self::Size => ("File Size |path|", "Return a regular file's byte length as an exact decimal String.", vec![("path", s)], s.into()),
            Self::Read => ("Read File |path|", "Read a regular file as strict UTF-8, preserving bytes and line endings under value/temporary limits.", vec![("path", s)], s.into()),
            Self::ReadBinary => ("Read Binary File |path|", "Read a regular file as an Array of Int bytes, bounded by array/value/temporary limits.", vec![("path", s)], Kind::Array.into()),
            Self::Write => ("Write File |path| Text |text|", "Create or truncate a regular file and write exact UTF-8 text without adding a newline; return None.", vec![("path", s), ("text", s)], Kind::None.into()),
            Self::Append => ("Append To File |path| Text |text|", "Create or append exact UTF-8 text to a regular file; concurrent writes can interleave. Return None.", vec![("path", s), ("text", s)], Kind::None.into()),
            Self::WriteBinary => ("Write Binary File |path| Bytes |bytes|", "Create or truncate a regular file after validating every Array element as an Int from 0 through 255. Return None.", vec![("path", s), ("bytes", Kind::Array)], Kind::None.into()),
            Self::Create => ("Create File |path| Text |text|", "Atomically create a new regular file; fail if any destination entry exists, including a dangling symlink. Return None.", vec![("path", s), ("text", s)], Kind::None.into()),
            Self::Copy => ("Copy File |source| To |destination|", "Stream a regular file into a new destination without overwriting existing entries; no permission/metadata copy. Return None.", vec![("source", s), ("destination", s)], Kind::None.into()),
            Self::Move => ("Move Path |source| To |destination|", "Rename a path using native replacement rules; cross-filesystem moves fail without a copy fallback. Return None.", vec![("source", s), ("destination", s)], Kind::None.into()),
            Self::RemoveFile => ("Remove File |path|", "Remove a file or supported symlink entry; missing paths succeed. Return None; directories are not recursively removed.", vec![("path", s)], Kind::None.into()),
            Self::CreateDirectory => ("Create Directory |path|", "Create missing parents and the directory; an existing directory succeeds. Return None.", vec![("path", s)], Kind::None.into()),
            Self::List => ("List Directory |path|", "Return sorted UTF-8 immediate entry names, including hidden entries; no recursion or implicit filtering.", vec![("path", s)], Kind::Array.into()),
            Self::RemoveDirectory => ("Remove Directory |path| Recursively |recursive|", "Remove an empty directory or its entire tree when Bool recursive is true; missing paths succeed. Return None.", vec![("path", s), ("recursive", Kind::Bool)], Kind::None.into()),
            Self::TemporaryDirectory => ("Create Temporary Directory In |directory| Prefix |prefix|", "Create a unique directory with an ASCII alphanumeric/dash/underscore prefix and return its path. Explicit cleanup is required.", vec![("directory", s), ("prefix", s)], s.into()),
            Self::EnvironmentExists => ("Environment Variable Exists |name|", "Test an exact key in the run's immutable environment snapshot, including empty/non-UTF-8 values.", vec![("name", s)], Kind::Bool.into()),
            Self::GetEnvironment => ("Get Environment Variable |name|", "Return a UTF-8 value from the run environment, or None when absent; never read live process globals.", vec![("name", s)], optional),
            Self::Environment => ("Environment Variables", "Return the run's immutable environment as a UTF-8 Map; non-UTF-8 names or values fail without lossy conversion.", vec![], Kind::Map.into()),
            Self::Platform => ("Operating System", "Return the Rust target OS name, such as linux, windows, or macos.", vec![], s.into()),
            Self::Separator => ("Path Separator", "Return the platform's primary path separator.", vec![], s.into()),
            Self::Components => ("Path Components |path|", "Return native lexical path components, including roots, prefixes, and parent components; do not access the filesystem.", vec![("path", s)], Kind::Array.into()),
        };
        let mut signature = StatementSignature::native_at("<operating-system>", header)
            .expect("fixed OS signature")
            .description(description)
            .returns(result)
            .documents_error(
                Code::IncompatibleType,
                "Invalid path, byte, environment name, prefix, or non-UTF-8 value.",
            )
            .expect("fixed error")
            .documents_error(
                Code::Native,
                "The filesystem operation failed; effects already completed are not rolled back.",
            )
            .expect("fixed error");
        if matches!(
            self,
            Self::Environment | Self::GetEnvironment | Self::EnvironmentExists
        ) {
            signature = signature
                .documents_error(
                    Code::RunConfiguration,
                    "Environment access requires an Engine run environment.",
                )
                .expect("fixed error");
        }
        for (name, kind) in parameters {
            signature = signature.parameter(name, kind).expect("fixed parameter");
        }
        signature
    }

    pub(super) fn invoke(
        self,
        arguments: Vec<TemporaryValue>,
        context: &Context,
    ) -> TemporaryResult {
        context.checkpoint()?;
        match self {
            Self::EnvironmentExists | Self::GetEnvironment | Self::Environment => {
                environment::invoke(self, &arguments, context)
            }
            Self::Platform => context.temporary_string(std::env::consts::OS),
            Self::Separator => context.temporary_string(std::path::MAIN_SEPARATOR_STR),
            Self::WorkingDirectory
            | Self::Join
            | Self::Absolute
            | Self::Parent
            | Self::Name
            | Self::Extension
            | Self::IsAbsolute
            | Self::Components => paths::invoke(self, &arguments, context),
            _ => files::invoke(self, &arguments, context),
        }
    }
}

fn text(value: &Literal) -> &str {
    let Literal::String(value) = value else {
        unreachable!("validated String")
    };
    value
}
fn invalid(context: &Context, reason: &str) -> RuntimeDiagnostic {
    context.detail_error(BWErr::OperationIncompatibleError, reason, None, false)
}
fn io_error(context: &Context, action: &str, path: &Path, error: io::Error) -> RuntimeDiagnostic {
    context.formatted_error(
        BWErr::NativeError,
        format_args!("{action} {}: {error}", path.display()),
        None,
        false,
    )
}
fn utf8<'a>(context: &Context, value: &'a OsStr) -> EvaluationResult<&'a str> {
    value
        .to_str()
        .ok_or_else(|| invalid(context, "The operating system value is not valid UTF-8"))
}
fn admitted<T>(context: &Context, value: Result<T, BWErr>) -> EvaluationResult<T> {
    value.map_err(|error| context.retain_limit(Diagnostic::new(error)).into())
}

lazy_static::lazy_static! {
    pub(super) static ref FIXED: Vec<(Builtin, Arc<StatementSignature>)> = [
        OsOp::WorkingDirectory, OsOp::Join, OsOp::Absolute, OsOp::Canonical, OsOp::Parent,
        OsOp::Name, OsOp::Extension, OsOp::IsAbsolute, OsOp::Exists, OsOp::Kind,
        OsOp::FileExists, OsOp::DirectoryExists, OsOp::Size, OsOp::Read, OsOp::ReadBinary,
        OsOp::Write, OsOp::Append, OsOp::WriteBinary, OsOp::Create, OsOp::Copy, OsOp::Move,
        OsOp::RemoveFile, OsOp::CreateDirectory, OsOp::List, OsOp::RemoveDirectory,
        OsOp::TemporaryDirectory, OsOp::EnvironmentExists, OsOp::GetEnvironment,
        OsOp::Environment, OsOp::Platform, OsOp::Separator, OsOp::Components,
    ].into_iter().map(|kind| (Builtin::OperatingSystem(kind), Arc::new(kind.signature()))).collect();
}
