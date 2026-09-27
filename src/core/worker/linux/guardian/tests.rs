use super::*;

#[test]
fn completion_frames_preserve_worker_status_and_reject_inconsistent_evidence() {
    let done = decode(&frame(0, 7 << 8, 0)).unwrap();
    assert_eq!(done.status.unwrap().code(), Some(7));
    assert_eq!(done.cleanup, WorkerCleanup::TreeReaped);
    assert!(done.error.is_none());
    assert_eq!(
        decode(&frame(0, libc::SIGUSR1, 0))
            .unwrap()
            .status
            .unwrap()
            .signal(),
        Some(libc::SIGUSR1)
    );
    assert_eq!(
        decode(&frame(1, 0, libc::ENOENT)).unwrap().cleanup,
        WorkerCleanup::NotStarted
    );
    let error = decode(&frame(2, 0, libc::EPERM)).unwrap();
    assert_eq!(error.cleanup, WorkerCleanup::TreeReaped);
    assert!(error.error.is_some());
    for rejected in [
        frame(3, 0, 0),
        frame(0, 0, libc::EIO),
        frame(1, 1, libc::EIO),
        frame(1, 0, 0),
        frame(2, 0, 0),
        frame(0, 0x7f, 0),
        frame(0, 1 << 24, 0),
        frame(0, -2, 0),
        frame(0, libc::SIGRTMAX() + 1, 0),
        [0; FRAME_BYTES],
    ] {
        assert!(decode(&rejected).is_err());
    }
}

#[test]
fn child_scanning_handles_split_pids_and_continues_after_signal_errors() {
    struct Bytes<'a>(&'a [u8]);
    impl Read for Bytes<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = self.0.len().min(2).min(buffer.len());
            buffer[..count].copy_from_slice(&self.0[..count]);
            self.0 = &self.0[count..];
            Ok(count)
        }
    }
    let mut seen = Vec::new();
    let error = visit_children(Bytes(b"12345 67 89"), |pid| {
        seen.push(pid);
        if pid == 12345 {
            Err(io::Error::from_raw_os_error(libc::EPERM))
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    assert_eq!(seen, [12345, 67, 89]);
    for invalid in [b"2147483648".as_slice(), b"-5", b"text"] {
        assert!(visit_children(invalid, |_| panic!("invalid PID reached signalling")).is_err());
    }
    visit_children(b"0 ".as_slice(), |_| panic!("never signal a process group")).unwrap();
}

#[test]
fn stalled_control_close_retains_ownership_after_tree_acknowledgment() {
    for acknowledgment in [frame(0, 0, 0), frame(2, 7 << 8, libc::EPERM)] {
        use std::sync::mpsc;
        let pool = WorkerPool::new(WorkerLimits {
            max_in_flight: NonZeroUsize::new(1).unwrap(),
            timeout: Duration::from_secs(5),
            cleanup_timeout: Duration::from_millis(20),
            ..Default::default()
        })
        .unwrap();
        let (control, mut peer) = UnixStream::pair().unwrap();
        control.set_nonblocking(true).unwrap();
        peer.write_all(&acknowledgment).unwrap();
        drop(peer);
        // A real owned child plus a valid acknowledgment isolates the host's final
        // descriptor-close boundary without needing a second guardian test executable.
        *pool.0 .0.launcher.lock().unwrap() = Some(Box::new(move |command| {
            let mut child = super::super::launch::spawn(command)?;
            child.guardian = Some(control);
            Ok(child)
        }));
        let (ready, entered) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        pool.0 .0.io_hooks.lock().unwrap().push_back((
            Point::ControlClose,
            Box::new(move |pid| {
                let _ = ready.send(pid);
                let _ = wait.recv_timeout(Duration::from_secs(5));
            }),
        ));
        let retained = Arc::new(());
        let weak = Arc::downgrade(&retained);
        let command = WorkerCommand {
            executable: "/bin/cat".into(),
            arguments: vec![],
            directory: std::env::temp_dir(),
            environment: Default::default(),
        };
        let handle = pool
            .start_retained(
                command.clone(),
                b"input".to_vec(),
                OperationControl::default(),
                Some(retained),
            )
            .unwrap();
        let pid = entered.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let delivered = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(2), handle.wait_retained())
                .await
                .unwrap()
        });
        assert_eq!(delivered.report.cleanup, WorkerCleanup::Pending);
        assert_eq!(delivered.report.outcome, WorkerOutcome::Failed);
        assert!(!delivered.report.progress_complete);
        let diagnostic = delivered.report.diagnostic.as_ref().unwrap();
        if acknowledgment[4] == 2 {
            assert_eq!(
                diagnostic.code(),
                crate::core::diagnostic::DiagnosticCode::Native
            );
            assert_eq!(
                diagnostic.causes[0].code(),
                crate::core::diagnostic::DiagnosticCode::AsyncRuntime
            );
        }
        drop(delivered);
        assert!(weak.upgrade().is_some());
        assert!(pool
            .start(command, vec![], OperationControl::default())
            .is_err());
        release.send(()).unwrap();
        let end = Instant::now() + Duration::from_secs(2);
        while !pool.snapshot().active.is_empty() || weak.upgrade().is_some() {
            assert!(Instant::now() < end);
            std::thread::sleep(QUANTUM);
        }
        assert_eq!(
            pool.snapshot().completed[0].cleanup,
            WorkerCleanup::TreeReaped
        );
        assert_eq!(pool.snapshot().completed[0].outcome, WorkerOutcome::Failed);
    }
}
