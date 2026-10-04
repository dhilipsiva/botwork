//! Process creation runs on the same owned thread as I/O and cleanup.
use super::*;

pub(super) fn spawn(specification: WorkerCommand) -> io::Result<ChildOwner> {
    Command::new(&specification.executable)
        .args(&specification.arguments)
        .current_dir(&specification.directory)
        .env_clear()
        .envs(&specification.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map(|child| ChildOwner {
            group: register_group(child.id()),
            child: child.into(),
            owned: true,
            guardian: None,
            #[cfg(test)]
            observer: None,
        })
}

#[cfg(test)]
mod tests;
