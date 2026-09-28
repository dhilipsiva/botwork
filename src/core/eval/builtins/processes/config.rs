use super::*;
use crate::core::worker::WorkerCommand;
use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

enum Input<'a> {
    Empty,
    Text(&'a str),
    Bytes(&'a [Literal]),
}
pub(super) struct Plan<'a> {
    environment: &'a RunEnvironment,
    executable: &'a str,
    arguments: &'a [Literal],
    directory: Option<&'a str>,
    overlay: Option<&'a HashMap<String, Literal>>,
    inherit: bool,
    input: Input<'a>,
    directory_bytes: usize,
    executable_bytes: usize,
    command_bytes: usize,
    pub(super) limits: WorkerLimits,
}

impl<'a> Plan<'a> {
    pub(super) fn new(
        context: &Context,
        environment: &'a RunEnvironment,
        binary: bool,
        executable: &'a Literal,
        arguments: &'a Literal,
        options: Option<&'a Literal>,
    ) -> EvaluationResult<Self> {
        let Literal::String(executable) = executable else {
            unreachable!("validated executable")
        };
        let Literal::Array(arguments) = arguments else {
            unreachable!("validated arguments")
        };
        argument(context, executable, false)?;
        let mut plan = Self {
            environment,
            executable,
            arguments,
            directory: None,
            overlay: None,
            inherit: true,
            input: Input::Empty,
            directory_bytes: 0,
            executable_bytes: 0,
            command_bytes: 0,
            limits: WorkerLimits {
                request_bytes: MAX_INPUT_BYTES,
                stdout_bytes: if binary { 16_384 } else { 1024 * 1024 },
                stderr_bytes: if binary { 16_384 } else { 1024 * 1024 },
                ..WorkerLimits::default()
            },
        };
        if let Some(options) = options {
            let Literal::Map(options) = options else {
                unreachable!("validated options")
            };
            for (name, value) in options {
                context.checkpoint()?;
                match (name.as_str(), value) {
                    ("directory", Literal::String(value)) => {
                        argument(context, value, false)?;
                        plan.directory = Some(value);
                    }
                    ("environment", Literal::Map(value)) => plan.overlay = Some(value),
                    ("inherit_environment", Literal::Bool(value)) => plan.inherit = *value,
                    ("stdin", Literal::None) => (),
                    ("stdin", Literal::String(value)) if !binary => plan.input = Input::Text(value),
                    ("stdin", Literal::Array(value)) if binary => plan.input = Input::Bytes(value),
                    ("timeout_ms", Literal::Int(value)) if *value >= 0 => {
                        plan.limits.timeout = Duration::from_millis(*value as u64)
                    }
                    ("cleanup_timeout_ms", Literal::Int(value)) if *value >= 0 => {
                        plan.limits.cleanup_timeout = Duration::from_millis(*value as u64)
                    }
                    ("stdout_limit", Literal::Int(value)) if *value >= 0 => {
                        plan.limits.stdout_bytes = *value as usize
                    }
                    ("stderr_limit", Literal::Int(value)) if *value >= 0 => {
                        plan.limits.stderr_bytes = *value as usize
                    }
                    _ => {
                        return Err(context.formatted_error(
                            BWErr::OperationIncompatibleError,
                            format_args!("Unknown process option or invalid value for {name:?}"),
                            None,
                            false,
                        ))
                    }
                }
            }
        }
        for value in arguments {
            context.checkpoint()?;
            let Literal::String(value) = value else {
                return Err(invalid(context, "Process arguments must be Strings"));
            };
            argument(context, value, true)?;
        }
        if let Some(overlay) = plan.overlay {
            for (name, value) in overlay {
                environment_name(context, OsStr::new(name))?;
                match value {
                    Literal::None => (),
                    Literal::String(value) => argument(context, value, true)?,
                    _ => {
                        return Err(invalid(
                            context,
                            "Process environment values must be Strings or None",
                        ))
                    }
                }
            }
        }
        if let Input::Bytes(bytes) = &plan.input {
            for byte in *bytes {
                context.checkpoint()?;
                if !matches!(byte, Literal::Int(value) if (0..=255).contains(value)) {
                    return Err(invalid(
                        context,
                        "Process input bytes must be Int values from 0 through 255",
                    ));
                }
            }
        }
        if plan.input_bytes() > MAX_INPUT_BYTES {
            return Err(limit(context, "process input bytes", MAX_INPUT_BYTES));
        }
        let base = environment.working_directory().as_os_str().len();
        plan.directory_bytes = match plan.directory {
            Some(path) => joined_size(context, base, path)?,
            None => base,
        };
        plan.executable_bytes = if bare(executable) {
            executable.len()
        } else {
            joined_size(context, plan.directory_bytes, executable)?
        };
        let mut bytes = 0;
        let mut entries = arguments.len();
        add(context, &mut bytes, plan.directory_bytes)?;
        add(context, &mut bytes, plan.executable_bytes)?;
        for value in arguments {
            let Literal::String(value) = value else {
                unreachable!()
            };
            add(context, &mut bytes, value.len())?;
        }
        plan.visit_environment(context, |name, value| {
            entries = entries
                .checked_add(1)
                .ok_or_else(|| limit(context, "process command entries", MAX_COMMAND_ENTRIES))?;
            add(context, &mut bytes, name.len())?;
            add(context, &mut bytes, value.len())
        })?;
        if entries > MAX_COMMAND_ENTRIES {
            return Err(limit(
                context,
                "process command entries",
                MAX_COMMAND_ENTRIES,
            ));
        }
        plan.command_bytes = bytes;
        Ok(plan)
    }

    fn input_bytes(&self) -> usize {
        match self.input {
            Input::Empty => 0,
            Input::Text(text) => text.len(),
            Input::Bytes(bytes) => bytes.len(),
        }
    }
    pub(super) fn workspace_bytes(&self, context: &Context) -> EvaluationResult<usize> {
        self.command_bytes
            // Specification, native Command copies, and environment marshalling
            // can overlap during spawn. Reserve three logical payload copies.
            .checked_mul(3)
            .and_then(|n| n.checked_add(self.input_bytes()))
            .and_then(|n| n.checked_add(self.limits.stdout_bytes))
            .and_then(|n| n.checked_add(self.limits.stderr_bytes))
            .ok_or_else(|| limit(context, "process in-flight bytes", MAX_IN_FLIGHT_BYTES))
    }
    fn visit_environment(
        &self,
        context: &Context,
        mut visit: impl FnMut(&OsStr, &OsStr) -> EvaluationResult<()>,
    ) -> EvaluationResult<()> {
        if self.inherit {
            for (name, value) in self.environment.variables() {
                context.checkpoint()?;
                if name
                    .to_str()
                    .is_some_and(|name| self.overlay.is_some_and(|map| map.contains_key(name)))
                {
                    continue;
                }
                environment_name(context, name)?;
                if value.as_encoded_bytes().contains(&0) {
                    return Err(invalid(
                        context,
                        "Process environment values must not contain NUL",
                    ));
                }
                visit(name, value)?;
            }
        }
        if let Some(overlay) = self.overlay {
            for (name, value) in overlay {
                context.checkpoint()?;
                if let Literal::String(value) = value {
                    visit(OsStr::new(name), OsStr::new(value))?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn command(&self, context: &Context) -> EvaluationResult<WorkerCommand> {
        let base = self.environment.working_directory();
        let directory = self.directory.map_or_else(
            || base.to_owned(),
            |path| join(base, path, self.directory_bytes),
        );
        let executable = if bare(self.executable) {
            PathBuf::from(self.executable)
        } else {
            join(&directory, self.executable, self.executable_bytes)
        };
        let mut arguments = Vec::with_capacity(self.arguments.len());
        for value in self.arguments {
            context.checkpoint()?;
            let Literal::String(value) = value else {
                unreachable!()
            };
            arguments.push(OsString::from(value));
        }
        let mut environment = BTreeMap::new();
        self.visit_environment(context, |name, value| {
            environment.insert(name.to_owned(), value.to_owned());
            Ok(())
        })?;
        Ok(WorkerCommand {
            executable,
            arguments,
            directory,
            environment,
        })
    }
    pub(super) fn input(&self, context: &Context) -> EvaluationResult<Vec<u8>> {
        let mut output = Vec::with_capacity(self.input_bytes());
        match self.input {
            Input::Empty => (),
            Input::Text(text) => output.extend_from_slice(text.as_bytes()),
            Input::Bytes(bytes) => {
                for chunk in bytes.chunks(4096) {
                    context.checkpoint()?;
                    output.extend(chunk.iter().map(|value| {
                        let Literal::Int(byte) = value else {
                            unreachable!()
                        };
                        *byte as u8
                    }));
                }
            }
        }
        Ok(output)
    }
}

fn argument(context: &Context, value: &str, empty: bool) -> EvaluationResult<()> {
    if value.contains('\0') || (!empty && value.is_empty()) {
        Err(invalid(context, "Process paths must be nonempty; paths, arguments and environment values must not contain NUL"))
    } else {
        Ok(())
    }
}
fn environment_name(context: &Context, value: &OsStr) -> EvaluationResult<()> {
    if value.is_empty()
        || value
            .as_encoded_bytes()
            .iter()
            .any(|byte| matches!(byte, 0 | b'='))
    {
        Err(invalid(
            context,
            "Process environment names must be nonempty and contain neither NUL nor '='",
        ))
    } else {
        Ok(())
    }
}
fn bare(value: &str) -> bool {
    !value.contains(std::path::MAIN_SEPARATOR)
        && matches!(
            Path::new(value).components().next(),
            Some(std::path::Component::Normal(_))
        )
}
fn joined_size(context: &Context, base: usize, path: &str) -> EvaluationResult<usize> {
    let bytes = if Path::new(path).is_absolute() {
        Some(path.len())
    } else {
        base.checked_add(path.len()).and_then(|n| n.checked_add(1))
    };
    bytes
        .filter(|n| *n <= MAX_COMMAND_BYTES)
        .ok_or_else(|| limit(context, "process command bytes", MAX_COMMAND_BYTES))
}
fn join(base: &Path, path: &str, capacity: usize) -> PathBuf {
    let mut result = PathBuf::with_capacity(capacity);
    if !Path::new(path).is_absolute() {
        result.push(base);
    }
    result.push(path);
    result
}
fn add(context: &Context, total: &mut usize, bytes: usize) -> EvaluationResult<()> {
    *total = total
        .checked_add(bytes)
        .and_then(|n| n.checked_add(1))
        .filter(|n| *n <= MAX_COMMAND_BYTES)
        .ok_or_else(|| limit(context, "process command bytes", MAX_COMMAND_BYTES))?;
    Ok(())
}
