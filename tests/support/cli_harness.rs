use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub struct Harness {
    pub workspace: PathBuf,
    pub environment: String,
}

impl Harness {
    pub fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/cli-cases");
        fs::create_dir_all(&root).unwrap();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut workspace = None;
        for attempt in 0..100 {
            let path = root.join(format!("{}-{nonce}-{attempt}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    workspace = Some(path);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create CLI workspace: {error}"),
            }
        }
        let toolchain = Command::new("rustc").arg("--version").output().unwrap();
        assert!(toolchain.status.success());
        Self {
            workspace: workspace.expect("unique CLI workspace"),
            environment: format!(
                "{} / {}-{} / {} / locked dependencies / seed=none / adapters=none",
                String::from_utf8_lossy(&toolchain.stdout).trim(),
                std::env::consts::OS,
                std::env::consts::ARCH,
                if cfg!(debug_assertions) {
                    "debug"
                } else {
                    "release"
                }
            ),
        }
    }

    pub fn run(&self, id: &str, source: &str, timeout: Duration) -> Result<Output, String> {
        assert!(
            !id.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        );
        let path = self.workspace.join(format!("{id}.botwork"));
        fs::write(&path, source).unwrap();
        let stdout = self.workspace.join(format!("{id}.stdout"));
        let stderr = self.workspace.join(format!("{id}.stderr"));
        let mut child = Command::new(env!("CARGO_BIN_EXE_botwork"))
            .current_dir(&self.workspace)
            .arg("--file")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if start.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{id} timed out after {timeout:?}; {}\n{source}",
                    self.environment
                ));
            }
            thread::sleep(Duration::from_millis(5));
        };
        Ok(Output {
            status,
            stdout: fs::read(stdout).unwrap(),
            stderr: fs::read(stderr).unwrap(),
        })
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.workspace);
    }
}
