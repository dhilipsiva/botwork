#[path = "support/markdown.rs"]
mod markdown;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Workspace(PathBuf);

impl Workspace {
    fn create() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/doc-examples");
        fs::create_dir_all(&root).unwrap();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        for attempt in 0..100 {
            let path = root.join(format!("{}-{nonce}-{attempt}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create documentation workspace: {error}"),
            }
        }
        panic!("unique documentation workspace");
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn markdown_files(root: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            markdown_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

fn documents() -> Vec<(String, Vec<markdown::Block>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("README.md")];
    markdown_files(&root.join("docs"), &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            let source = fs::read_to_string(&path).unwrap();
            let blocks =
                markdown::blocks(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
            (name, blocks)
        })
        .collect()
}

#[test]
fn every_documented_botwork_example_matches_its_cli_output() {
    let expected = BTreeMap::from([
        (
            "readme-sample",
            (
                "README.md",
                include_bytes!("doc-examples/readme-sample.stdout").as_slice(),
            ),
        ),
        (
            "multiline-call",
            (
                "docs/language.md",
                include_bytes!("doc-examples/multiline-call.stdout").as_slice(),
            ),
        ),
        (
            "multilingual-call",
            (
                "docs/language.md",
                include_bytes!("doc-examples/multilingual-call.stdout").as_slice(),
            ),
        ),
        (
            "catch-recovery",
            (
                "docs/language.md",
                include_bytes!("doc-examples/catch-recovery.stdout").as_slice(),
            ),
        ),
    ]);
    let mut seen = BTreeSet::new();
    let workspace = Workspace::create();
    let toolchain = Command::new("rustc").arg("--version").output().unwrap();
    assert!(toolchain.status.success());
    let environment = format!(
        "{} / {}-{} / {} / locked dependencies / timeout=5s / seed=none / adapters=none",
        String::from_utf8_lossy(&toolchain.stdout).trim(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    for (document, blocks) in documents() {
        for block in blocks
            .into_iter()
            .filter(|block| block.language == "botwork")
        {
            let id = block.id.as_deref().unwrap();
            assert!(
                seen.insert(id.to_owned()),
                "duplicate documentation id: {id}"
            );
            let (expected_document, stdout) = expected
                .get(id)
                .unwrap_or_else(|| panic!("{document}:{}: register output for {id}", block.line));
            assert_eq!(&document, expected_document, "{id}: unexpected document");
            let source = workspace.0.join(format!("{id}.botwork"));
            fs::write(&source, &block.source).unwrap();
            let stdout_path = workspace.0.join(format!("{id}.stdout"));
            let stderr_path = workspace.0.join(format!("{id}.stderr"));
            let mut child = Command::new(env!("CARGO_BIN_EXE_botwork"))
                .arg("--file")
                .arg(&source)
                .stdin(Stdio::null())
                .stdout(fs::File::create(&stdout_path).unwrap())
                .stderr(fs::File::create(&stderr_path).unwrap())
                .spawn()
                .unwrap();
            let start = Instant::now();
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if start.elapsed() >= Duration::from_secs(5) {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "{document}:{} ({id}) timed out; {environment}\n{}",
                        block.line, block.source
                    );
                }
                thread::sleep(Duration::from_millis(5));
            };
            let diagnostic = fs::read(&stderr_path).unwrap();
            let label = format!(
                "{document}:{} ({id}); {environment}\n{}",
                block.line, block.source
            );
            assert_eq!(
                status.code(),
                Some(0),
                "{label}\n{}",
                String::from_utf8_lossy(&diagnostic)
            );
            assert!(
                diagnostic.is_empty(),
                "{label}: {}",
                String::from_utf8_lossy(&diagnostic)
            );
            assert_eq!(&fs::read(stdout_path).unwrap(), stdout, "{label}");
        }
    }
    assert_eq!(
        seen,
        expected.keys().map(|key| (*key).to_owned()).collect(),
        "stale or missing documentation expectations"
    );
}

#[test]
fn every_rust_documentation_example_is_included_in_crate_doctests() {
    let inclusion = "#![doc = include_str!(\"../docs/interpreter-architecture.md\")]";
    assert!(include_str!("../src/lib.rs").contains(inclusion));
    let rust_documents: BTreeSet<_> = documents()
        .into_iter()
        .filter(|(_, blocks)| blocks.iter().any(|block| block.language == "rust"))
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        rust_documents,
        BTreeSet::from(["docs/interpreter-architecture.md".to_owned()]),
        "include new Rust documentation examples in rustdoc before registering their files"
    );
}
