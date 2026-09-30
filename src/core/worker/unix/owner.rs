//! Single ownership of every OS handle; observation locks never span OS calls.
use super::observation::{Observation, Stream};
use super::*;

fn close_pipe<T>(pipe: &mut Option<T>, _observation: &Observation, _pid: u32) {
    if pipe.is_some() {
        #[cfg(test)]
        _observation.hook(Point::Close, _pid);
        drop(pipe.take());
    }
}

fn read_pipe(
    pipe: &mut Option<impl Read>,
    observation: &Observation,
    stream: Stream,
    _pid: u32,
) -> DiagnosticResult<()> {
    let Some(reader) = pipe.as_mut() else {
        return Ok(());
    };
    if observation.status().published {
        close_pipe(pipe, observation, _pid);
        return Ok(());
    }
    let mut buffer = [0; CHUNK];
    let size = CHUNK.min(observation.remaining(stream).saturating_add(1));
    #[cfg(test)]
    observation.hook(Point::Read, _pid);
    match reader.read(&mut buffer[..size]) {
        Ok(0) => {
            close_pipe(pipe, observation, _pid);
            Ok(())
        }
        Ok(count) => {
            #[cfg(test)]
            observation.hook(Point::ReadComplete, _pid);
            observation.capture(stream, &buffer[..count])
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(io_failure(error)),
    }
}

pub(super) fn run(
    specification: WorkerCommand,
    input: RetainedInput,
    observation: &Arc<Observation>,
) {
    #[cfg(target_os = "linux")]
    if let Some(ticket) = &observation.request.journal {
        if let Err(error) = ticket.file() {
            observation.error(runtime(format_args!(
                "Persisting worker intent failed: {error}"
            )));
            drop(input);
            observation.finish(WorkerCleanup::NotStarted, false);
            return;
        }
    }
    if observation.status().stopped {
        drop(input);
        observation.finish(WorkerCleanup::NotStarted, false);
        return;
    }
    #[cfg(test)]
    let launcher = observation
        .shared
        .launcher
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    #[cfg(test)]
    let launched = match launcher {
        Some(launcher) => launcher(specification),
        None => launch_worker(specification, observation),
    };
    #[cfg(not(test))]
    let launched = launch_worker(specification, observation);
    let mut child = match launched {
        Ok(child) => child,
        Err(error) => {
            observation.error(runtime(format_args!(
                "Starting isolated worker failed: {error}"
            )));
            drop(input);
            observation.finish(WorkerCleanup::NotStarted, false);
            return;
        }
    };
    #[cfg(test)]
    {
        child.observer = Some(observation.clone());
    }
    let pid = child.child.id();
    observation.started(pid);
    let mut stdin = child.child.stdin.take();
    let mut stdout = child.child.stdout.take();
    let mut stderr = child.child.stderr.take();
    #[cfg(test)]
    observation.hook(Point::Setup, pid);
    let setup = nonblocking(stdin.as_ref().expect("piped stdin"))
        .and_then(|()| nonblocking(stdout.as_ref().expect("piped stdout")))
        .and_then(|()| nonblocking(stderr.as_ref().expect("piped stderr")));
    let mut output_failed = false;
    let mut written: usize = 0;
    let mut signalled = false;
    let mut unverified = false;
    let mut cleanup = WorkerCleanup::Reaped;
    if let Err(error) = setup {
        observation.error(io_failure(error));
        output_failed = true;
        close_pipe(&mut stdin, observation, pid);
        close_pipe(&mut stdout, observation, pid);
        close_pipe(&mut stderr, observation, pid);
    }
    loop {
        let state = observation.status();
        if state.stopped {
            close_pipe(&mut stdin, observation, pid);
            observation.cleanup();
        }
        if state.expired || state.published {
            close_pipe(&mut stdin, observation, pid);
            close_pipe(&mut stdout, observation, pid);
            close_pipe(&mut stderr, observation, pid);
            output_failed = true;
        }
        if state.stopped && !signalled {
            #[cfg(test)]
            observation.hook(Point::Terminate, pid);
            if let Err(error) = child.terminate() {
                observation.error(io_failure(error));
            }
            signalled = true;
        }
        if let Some(writer) = stdin.as_mut() {
            if written == input.len() {
                close_pipe(&mut stdin, observation, pid);
            } else {
                let end = input.len().min(written.saturating_add(CHUNK));
                #[cfg(test)]
                observation.hook(Point::Write, pid);
                match writer.write(&input[written..end]) {
                    Ok(0) => observation.error(io_failure(io::ErrorKind::WriteZero.into())),
                    Ok(count) => {
                        #[cfg(test)]
                        observation.hook(Point::WriteComplete, pid);
                        written += count;
                        observation.wrote(written);
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) => observation.error(io_failure(error)),
                }
            }
        }
        for stream in [Stream::Stdout, Stream::Stderr] {
            let result = match stream {
                Stream::Stdout => read_pipe(&mut stdout, observation, stream, pid),
                Stream::Stderr => read_pipe(&mut stderr, observation, stream, pid),
            };
            if let Err(error) = result {
                observation.error(error);
                output_failed = true;
                close_pipe(&mut stdout, observation, pid);
                close_pipe(&mut stderr, observation, pid);
            }
        }
        if child.owned {
            #[cfg(test)]
            observation.hook(Point::Observe, pid);
            match child.exited() {
                Ok(true) => {
                    // Start cleanup observation BEFORE signalling/reaping can stall.
                    observation.cleanup();
                    if !signalled {
                        #[cfg(test)]
                        observation.hook(Point::Terminate, pid);
                        if let Err(error) = child.terminate() {
                            observation.error(io_failure(error));
                        }
                        signalled = true;
                    }
                    #[cfg(test)]
                    observation.hook(Point::Reap, pid);
                    match child.reap() {
                        Ok(Some(completion)) => {
                            cleanup = completion.cleanup;
                            if let Some(status) = completion.status {
                                observation.reaped(status);
                                if !status.success() && !observation.status().stopped {
                                    observation.error(Diagnostic::formatted(
                                        BWErr::NativeError,
                                        format_args!("Isolated worker exited with {status}"),
                                    ));
                                }
                            }
                            if let Some(error) = completion.error {
                                observation.error(error);
                            }
                            if written != input.len() && !observation.status().stopped {
                                observation.error(runtime(format_args!(
                                    "Worker exited before accepting its complete request"
                                )));
                            }
                            close_pipe(&mut stdin, observation, pid);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            child.owned = false;
                            observation.lost_ownership(io_failure(error));
                            unverified = true;
                            break;
                        }
                    }
                }
                Ok(false) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    child.owned = false;
                    observation.lost_ownership(io_failure(error));
                    unverified = true;
                    break;
                }
            }
        }
        if !child.owned && stdout.is_none() && stderr.is_none() {
            break;
        }
        std::thread::sleep(QUANTUM);
    }
    close_pipe(&mut stdin, observation, pid);
    close_pipe(&mut stdout, observation, pid);
    close_pipe(&mut stderr, observation, pid);
    #[cfg(test)]
    if child.guardian.is_some() {
        observation.hook(Point::ControlClose, pid);
    }
    close_pipe(&mut child.guardian, observation, pid);
    drop(child);
    let io_complete =
        cleanup != WorkerCleanup::NotStarted && !output_failed && written == input.len();
    // Final publication must not race the request payload's reservation refund.
    drop(input);
    if unverified {
        observation.finish(WorkerCleanup::Unverified, false);
    } else {
        observation.finish(cleanup, io_complete);
    }
}

fn launch_worker(
    specification: WorkerCommand,
    observation: &Observation,
) -> io::Result<ChildOwner> {
    #[cfg(target_os = "linux")]
    if let Some(executable) = &observation.shared.guardian {
        let record = observation
            .request
            .journal
            .as_ref()
            .map(|ticket| ticket.file())
            .transpose()?;
        return guardian::spawn(
            executable,
            specification,
            record.as_deref(),
            observation.shared.namespaced,
        );
    }
    #[cfg(not(target_os = "linux"))]
    let _ = observation;
    launch::spawn(specification)
}
