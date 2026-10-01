//! Packages (decision D10): `botwork --fetch` resolves a project's
//! `botwork.toml` into `botwork.lock` and a cache, and scripts import package
//! files as `@name/path`. Git sources need `git`; the tests are skipped
//! without it unless `BOTWORK_REQUIRE_GIT` is set, as it is in CI.

use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

fn has_git() -> bool {
    let found = Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    assert!(
        found || std::env::var_os("BOTWORK_REQUIRE_GIT").is_none(),
        "BOTWORK_REQUIRE_GIT is set but git was not found"
    );
    found
}

/// A directory of projects and packages, with a cache of its own.
struct World {
    directory: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        Self {
            directory: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        botwork::core::paths::canonicalize(self.directory.path())
            .unwrap()
            .join(relative)
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    /// A package directory with a manifest and files.
    fn package(&self, directory: &str, manifest: &str, files: &[(&str, &str)]) -> PathBuf {
        self.write(&format!("{directory}/botwork.toml"), manifest);
        for (name, text) in files {
            self.write(&format!("{directory}/{name}"), text);
        }
        self.path(directory)
    }

    /// Commit everything in `directory` as a git repository tagged `tag`.
    fn git(&self, directory: &Path, tag: &str) -> String {
        let git = |arguments: &[&str]| {
            let output = Command::new("git")
                .args([
                    "-c",
                    "user.name=Botwork",
                    "-c",
                    "user.email=botwork@example.com",
                ])
                .args(["-c", "init.defaultBranch=main", "-c", "core.autocrlf=false"])
                .args(arguments)
                .current_dir(directory)
                .output()
                .unwrap();
            assert!(output.status.success(), "{arguments:?}: {output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        if !directory.join(".git").exists() {
            git(&["init", "--quiet"]);
        }
        git(&["add", "--all"]);
        git(&["commit", "--quiet", "--allow-empty", "-m", tag]);
        git(&["tag", tag]);
        git(&["rev-parse", "HEAD"])
    }

    fn command(&self, directory: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_botwork"));
        command
            .current_dir(self.path(directory))
            .env("BOTWORK_CACHE_DIR", self.path("cache"));
        command
    }

    fn fetch(&self, directory: &str, flags: &[&str]) -> Output {
        self.command(directory)
            .arg("--fetch")
            .args(flags)
            .output()
            .unwrap()
    }

    fn run(&self, directory: &str, file: &str) -> Output {
        self.command(directory)
            .args(["--file", file])
            .output()
            .unwrap()
    }

    fn check(&self, directory: &str, file: &str) -> Output {
        self.command(directory)
            .args(["--check", "--file", file])
            .output()
            .unwrap()
    }
}

fn file_url(path: &Path) -> String {
    url::Url::from_directory_path(path).unwrap().to_string()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn succeeded(output: &Output) {
    assert!(output.status.success(), "{output:?}");
}

fn failed_with(output: &Output, reason: &str) {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        stderr(output).contains(reason),
        "{reason}: {}",
        stderr(output)
    );
}

/// A gzipped tarball of `files` under a top-level directory, as releases are.
fn tarball(top: &str, files: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, text) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("{top}/{name}"), text.as_bytes())
            .unwrap();
    }
    let archive = builder.into_inner().unwrap();
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&archive).unwrap();
    gzip.finish().unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Request paths, or `*` for every path, and the bodies that answer them.
type Routes = Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

/// An HTTP server on this machine that answers each request with the body
/// routed to its path, or `fallback` for any path, and counts them.
struct Server {
    port: u16,
    requests: Arc<AtomicUsize>,
    routes: Routes,
}

impl Server {
    fn start(fallback: Vec<u8>) -> Self {
        let server = Self::empty();
        server.serve("*", fallback);
        server
    }

    fn empty() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let routes: Routes = Arc::default();
        let (counted, served) = (Arc::clone(&requests), Arc::clone(&routes));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&buffer[..read]),
                    }
                }
                counted.fetch_add(1, Ordering::SeqCst);
                let request = String::from_utf8_lossy(&request);
                let path = request.split(' ').nth(1).unwrap_or("").to_owned();
                let body = served
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(route, _)| *route == path || route == "*")
                    .map(|(_, body)| body.clone());
                let _ = match body {
                    Some(body) => write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .and_then(|()| stream.write_all(&body)),
                    None => write!(
                        stream,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    ),
                };
            }
        });
        Self {
            port,
            requests,
            routes,
        }
    }

    /// Answer requests for `path` (or every path, for `*`) with `body`.
    fn serve(&self, path: &str, body: Vec<u8>) {
        self.routes.lock().unwrap().push((path.to_owned(), body));
    }

    fn url(&self, name: &str) -> String {
        format!("http://127.0.0.1:{}/{name}", self.port)
    }
}

/// A project with one package of each kind: a path, a git tag, and a url.
struct Fixture {
    world: World,
    server: Server,
    kit: PathBuf,
}

const MAIN: &str = r#"Import |"@helpers/math.botwork"| As |math|
Import |"@kit/greet.botwork"| As |kit|
Import |"@pricing/lib/tax.botwork"| As |tax|
Log |@{ math::Double |21| }|
Log |@{ kit::Greet |"Ada"| }|
Log |@{ tax::Taxed |1| }|
"#;

fn fixture() -> Fixture {
    let world = World::new();
    world.package(
        "helpers",
        "[package]\nname = \"helpers\"\nversion = \"0.1.0\"\n",
        &[("math.botwork", "Double |x| {\n    Return |x * 2|\n}\n")],
    );
    let kit = world.package(
        "kit",
        "[package]\nname = \"kit\"\nversion = \"1.4.0\"\n",
        &[(
            "greet.botwork",
            "Greet |name| {\n    Return |\"Hello, \" + name|\n}\n",
        )],
    );
    world.git(&kit, "v1.4.0");
    let archive = tarball(
        "pricing-2.0.0",
        &[
            (
                "botwork.toml",
                "[package]\nname = \"pricing\"\nversion = \"2.0.0\"\n",
            ),
            ("lib/tax.botwork", "Taxed |x| {\n    Return |x + 1|\n}\n"),
        ],
    );
    let digest = sha256(&archive);
    let server = Server::start(archive);
    world.write(
        "project/botwork.toml",
        &format!(
            "[dependencies]\nhelpers = {{ path = \"../helpers\" }}\nkit = {{ git = \"{}\", tag = \"v1.4.0\", version = \"^1.4\" }}\npricing = {{ url = \"{}\", sha256 = \"{digest}\" }}\n",
            file_url(&kit),
            server.url("pricing.tar.gz")
        ),
    );
    world.write("project/main.botwork", MAIN);
    Fixture { world, server, kit }
}

#[test]
fn fetching_locks_path_git_and_url_packages_and_scripts_import_them() {
    if !has_git() {
        return;
    }
    let Fixture { world, kit, .. } = fixture();
    let fetched = world.fetch("project", &[]);
    succeeded(&fetched);
    assert!(
        stdout(&fetched).contains("[fetch] kit 1.4.0 from git"),
        "{}",
        stdout(&fetched)
    );
    assert!(stdout(&fetched).ends_with("[fetch] botwork.lock written\n"));
    let lock = fs::read_to_string(world.path("project/botwork.lock")).unwrap();
    let commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&kit)
        .output()
        .unwrap();
    let commit = String::from_utf8(commit.stdout).unwrap();
    assert!(
        lock.contains(&format!("commit = \"{}\"", commit.trim())),
        "{lock}"
    );
    assert_eq!(lock.matches("tree = \"sha256-").count(), 2, "{lock}");
    assert!(lock.contains("path = \"../helpers\""), "{lock}");
    // The same resolution writes the same lockfile.
    let again = world.fetch("project", &[]);
    succeeded(&again);
    assert!(stdout(&again).ends_with("[fetch] botwork.lock up to date\n"));
    assert_eq!(
        fs::read_to_string(world.path("project/botwork.lock")).unwrap(),
        lock
    );
    let run = world.run("project", "main.botwork");
    succeeded(&run);
    assert_eq!(stdout(&run), "42\nHello, Ada\n2\n");
    // A manifest that names another source than the lockfile pins.
    let manifest = fs::read_to_string(world.path("project/botwork.toml")).unwrap();
    world.write(
        "project/botwork.toml",
        &manifest.replace("tag = \"v1.4.0\"", "tag = \"v1.5.0\""),
    );
    failed_with(
        &world.run("project", "main.botwork"),
        "botwork.lock pins `kit` from git",
    );
}

#[test]
fn once_fetched_runs_and_offline_fetches_need_no_network() {
    if !has_git() {
        return;
    }
    let Fixture { world, server, kit } = fixture();
    succeeded(&world.fetch("project", &[]));
    let downloads = server.requests.load(Ordering::SeqCst);
    assert_eq!(downloads, 1);
    // The git remote and the server are no longer needed.
    fs::remove_dir_all(&kit).unwrap();
    succeeded(&world.fetch("project", &["--offline", "--locked"]));
    succeeded(&world.run("project", "main.botwork"));
    assert_eq!(server.requests.load(Ordering::SeqCst), downloads);
    // Without the cache, offline fetching says what it lacks.
    fs::remove_dir_all(world.path("cache/packages")).unwrap();
    failed_with(
        &world.fetch("project", &["--offline"]),
        "is not in the cache, and --offline forbids fetching it",
    );
    assert_eq!(server.requests.load(Ordering::SeqCst), downloads);
}

#[test]
fn archives_and_fetched_files_must_match_their_pins() {
    if !has_git() {
        return;
    }
    let Fixture { world, server, .. } = fixture();
    // A url source whose archive is not the one its manifest pins.
    let manifest = fs::read_to_string(world.path("project/botwork.toml")).unwrap();
    let wrong = format!("sha256 = \"{}\"", "0".repeat(64));
    let tampered = regex_free_replace(&manifest, "sha256 = \"", &wrong);
    world.write("project/botwork.toml", &tampered);
    failed_with(&world.fetch("project", &[]), "integrity check failed");
    assert!(!world.path("project/botwork.lock").exists());
    world.write("project/botwork.toml", &manifest);
    succeeded(&world.fetch("project", &[]));
    // Files that do not hash to the lockfile's tree are refused on refetch.
    let lock = fs::read_to_string(world.path("project/botwork.lock")).unwrap();
    let start = lock.find("tree = \"sha256-").unwrap() + "tree = \"sha256-".len();
    let mut forged = lock.clone();
    forged.replace_range(start..start + 64, &"1".repeat(64));
    world.write("project/botwork.lock", &forged);
    let _ = server;
    failed_with(&world.fetch("project", &[]), "integrity check failed");
}

/// Replace the 64-digit hash after `prefix` with `replacement`.
fn regex_free_replace(text: &str, prefix: &str, replacement: &str) -> String {
    let start = text.find(prefix).unwrap();
    let end = start + prefix.len() + 64 + 1;
    format!("{}{replacement}{}", &text[..start], &text[end..])
}

#[test]
fn conflicting_sources_and_incompatible_versions_are_refused() {
    if !has_git() {
        return;
    }
    let world = World::new();
    let kit = world.package(
        "kit",
        "[package]\nname = \"kit\"\nversion = \"1.4.0\"\n",
        &[],
    );
    world.git(&kit, "v1.4.0");
    world.write(
        "kit/botwork.toml",
        "[package]\nname = \"kit\"\nversion = \"2.0.0\"\n",
    );
    world.git(&kit, "v2.0.0");
    let url = file_url(&kit);
    // Two packages need `kit` from different tags.
    world.package(
        "a",
        &format!("[package]\nname = \"a\"\nversion = \"1.0.0\"\n[dependencies]\nkit = {{ git = \"{url}\", tag = \"v1.4.0\" }}\n"),
        &[],
    );
    world.package(
        "b",
        &format!("[package]\nname = \"b\"\nversion = \"1.0.0\"\n[dependencies]\nkit = {{ git = \"{url}\", tag = \"v2.0.0\" }}\n"),
        &[],
    );
    world.write(
        "project/botwork.toml",
        "[dependencies]\na = { path = \"../a\" }\nb = { path = \"../b\" }\n",
    );
    failed_with(
        &world.fetch("project", &[]),
        "dependency conflict: `a` needs `kit` from git",
    );
    // A version requirement the resolved version fails.
    world.write(
        "project/botwork.toml",
        &format!(
            "[dependencies]\nkit = {{ git = \"{url}\", tag = \"v1.4.0\", version = \"^2\" }}\n"
        ),
    );
    failed_with(
        &world.fetch("project", &[]),
        "version incompatibility: botwork.toml needs `kit` ^2, but it is 1.4.0",
    );
    // A package that needs another Botwork.
    world.package(
        "future",
        "[package]\nname = \"future\"\nversion = \"1.0.0\"\nbotwork = \">=99\"\n",
        &[],
    );
    world.write(
        "project/botwork.toml",
        "[dependencies]\nfuture = { path = \"../future\" }\n",
    );
    failed_with(
        &world.fetch("project", &[]),
        "`future` 1.0.0 needs Botwork >=99",
    );
    // A dependency whose manifest names another package.
    world.write(
        "project/botwork.toml",
        "[dependencies]\nother = { path = \"../future\" }\n",
    );
    failed_with(&world.fetch("project", &[]), "names the package `future`");
    // A fetched package cannot name a local path.
    world.write(
        "kit/botwork.toml",
        "[package]\nname = \"kit\"\nversion = \"3.0.0\"\n[dependencies]\nlocal = { path = \"../a\" }\n",
    );
    world.git(&kit, "v3.0.0");
    world.write(
        "project/botwork.toml",
        &format!("[dependencies]\nkit = {{ git = \"{url}\", tag = \"v3.0.0\" }}\n"),
    );
    failed_with(
        &world.fetch("project", &[]),
        "a fetched package can only name git and url sources",
    );
}

#[test]
fn packages_import_only_what_their_own_manifests_name() {
    let world = World::new();
    world.package(
        "util",
        "[package]\nname = \"util\"\nversion = \"1.0.0\"\n",
        &[("text.botwork", "Shout |x| {\n    Return |x + \"!\"|\n}\n")],
    );
    world.package(
        "app",
        "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\nutil = { path = \"../util\" }\n",
        &[(
            "main.botwork",
            "Import |\"@util/text.botwork\"| As |util|\nHello {\n    Return |@{ util::Shout |\"hi\"| }|\n}\n",
        )],
    );
    world.package(
        "rogue",
        "[package]\nname = \"rogue\"\nversion = \"1.0.0\"\n",
        &[(
            "main.botwork",
            "Import |\"@util/text.botwork\"| As |util|\n",
        )],
    );
    world.write(
        "project/botwork.toml",
        "[dependencies]\napp = { path = \"../app\" }\nrogue = { path = \"../rogue\" }\nutil = { path = \"../util\" }\n",
    );
    world.write(
        "project/main.botwork",
        "Import |\"@app/main.botwork\"| As |app|\nLog |@{ app::Hello }|\n",
    );
    world.write(
        "project/rogue.botwork",
        "Import |\"@rogue/main.botwork\"| As |rogue|\n",
    );
    succeeded(&world.fetch("project", &[]));
    let run = world.run("project", "main.botwork");
    succeeded(&run);
    assert_eq!(stdout(&run), "hi!\n");
    // `rogue` uses `util` without naming it, though the project names it.
    failed_with(
        &world.run("project", "rogue.botwork"),
        "`@util` is not a dependency of the package `rogue`",
    );
    let check = world.check("project", "main.botwork");
    succeeded(&check);
    assert!(
        stderr(&check).contains("1 file, 2 modules: 0 errors"),
        "{}",
        stderr(&check)
    );
}

#[test]
fn imports_say_what_to_fetch_and_stay_inside_their_package() {
    let world = World::new();
    world.package(
        "helpers",
        "[package]\nname = \"helpers\"\nversion = \"0.1.0\"\n",
        &[("math.botwork", "Double |x| {\n    Return |x * 2|\n}\n")],
    );
    world.write(
        "loose/main.botwork",
        "Import |\"@helpers/math.botwork\"| As |math|\n",
    );
    failed_with(
        &world.run("loose", "main.botwork"),
        "package and URL imports need a botwork.toml",
    );
    world.write(
        "project/botwork.toml",
        "[dependencies]\nhelpers = { path = \"../helpers\" }\n",
    );
    world.write(
        "project/main.botwork",
        "Import |\"@helpers/math.botwork\"| As |math|\nLog |@{ math::Double |2| }|\n",
    );
    failed_with(
        &world.run("project", "main.botwork"),
        "botwork.lock does not list `helpers`; run `botwork --fetch`",
    );
    succeeded(&world.fetch("project", &[]));
    succeeded(&world.run("project", "main.botwork"));
    for (import, reason) in [
        ("@helpers/../project/main.botwork", "leaves the package"),
        ("@other/x.botwork", "`@other` is not a dependency"),
        ("@helpers", "no file in it"),
        ("@helpers/none.botwork", "none.botwork"),
    ] {
        world.write(
            "project/bad.botwork",
            &format!("Import |\"{import}\"| As |bad|\n"),
        );
        let run = world.run("project", "bad.botwork");
        failed_with(&run, reason);
        assert!(stderr(&run).contains("[BW6001]"), "{}", stderr(&run));
        let check = world.check("project", "bad.botwork");
        failed_with(&check, reason);
    }
    // Two packages under one namespace collide as modules do.
    world.write(
        "project/twice.botwork",
        "Import |\"@helpers/math.botwork\"| As |m|\nImport |\"@helpers/math.botwork\"| As |m|\n",
    );
    failed_with(&world.run("project", "twice.botwork"), "[BW6003]");
    // A link in a path package cannot lead out of it.
    #[cfg(unix)]
    {
        world.write("secret/secret.botwork", "Log |\"secret\"|\n");
        std::os::unix::fs::symlink(world.path("secret"), world.path("helpers/linked")).unwrap();
        world.write(
            "project/link.botwork",
            "Import |\"@helpers/linked/secret.botwork\"| As |s|\n",
        );
        failed_with(
            &world.run("project", "link.botwork"),
            "leads outside its package",
        );
    }
}

#[test]
fn locked_fetches_refuse_to_change_the_lockfile() {
    let world = World::new();
    world.package(
        "one",
        "[package]\nname = \"one\"\nversion = \"1.0.0\"\n",
        &[],
    );
    world.package(
        "two",
        "[package]\nname = \"two\"\nversion = \"1.0.0\"\n",
        &[],
    );
    world.write(
        "project/botwork.toml",
        "[dependencies]\none = { path = \"../one\" }\n",
    );
    failed_with(
        &world.fetch("project", &["--locked"]),
        "--locked needs a botwork.lock",
    );
    succeeded(&world.fetch("project", &[]));
    let lock = fs::read_to_string(world.path("project/botwork.lock")).unwrap();
    world.write(
        "project/botwork.toml",
        "[dependencies]\none = { path = \"../one\" }\ntwo = { path = \"../two\" }\n",
    );
    failed_with(&world.fetch("project", &["--locked"]), "does not pin `two`");
    assert_eq!(
        fs::read_to_string(world.path("project/botwork.lock")).unwrap(),
        lock
    );
    // A package's new version changes the lockfile too.
    world.write(
        "project/botwork.toml",
        "[dependencies]\none = { path = \"../one\" }\n",
    );
    world.write(
        "one/botwork.toml",
        "[package]\nname = \"one\"\nversion = \"1.1.0\"\n",
    );
    failed_with(
        &world.fetch("project", &["--locked"]),
        "botwork.lock is out of date",
    );
    succeeded(&world.fetch("project", &[]));
    assert!(fs::read_to_string(world.path("project/botwork.lock"))
        .unwrap()
        .contains("version = \"1.1.0\""));
    // Fetching needs a manifest.
    world.write("empty/readme.txt", "");
    failed_with(&world.fetch("empty", &[]), "has no botwork.toml");
}

#[test]
fn every_kind_of_module_imports_from_a_package() {
    let world = World::new();
    world.package(
        "tools",
        "[package]\nname = \"tools\"\nversion = \"1.0.0\"\n",
        &[],
    );
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wasm/statements.wasm"),
        world.path("tools/statements.wasm"),
    )
    .unwrap();
    world.write(
        "project/botwork.toml",
        "[dependencies]\ntools = { path = \"../tools\" }\n",
    );
    world.write(
        "project/main.botwork",
        "Import |\"@tools/statements.wasm\"| As |wasm|\nLog |@{ wasm::Echo |[1, 2]| }|\n",
    );
    succeeded(&world.fetch("project", &[]));
    let run = world.run("project", "main.botwork");
    succeeded(&run);
    assert_eq!(stdout(&run), "[1, 2]\n");
    // A WebAssembly module takes its namespace under the same rules.
    world.write(
        "project/twice.botwork",
        "Import |\"@tools/statements.wasm\"| As |wasm|\nImport |\"@tools/statements.wasm\"| As |wasm|\n",
    );
    failed_with(&world.run("project", "twice.botwork"), "[BW6003]");
}

#[test]
fn the_language_server_follows_package_imports() {
    let world = World::new();
    let module = world.package(
        "helpers",
        "[package]\nname = \"helpers\"\nversion = \"0.1.0\"\n",
        &[("math.botwork", "Double |x| {\n    Return |x * 2|\n}\n")],
    );
    world.write(
        "project/botwork.toml",
        "[dependencies]\nhelpers = { path = \"../helpers\" }\n",
    );
    succeeded(&world.fetch("project", &[]));
    let text = "Import |\"@helpers/math.botwork\"| As |m|\nLog |@{ m::Double |2| }|\n";
    let main = world.write("project/main.botwork", text);
    let language = botwork::core::language::Language::new(world.path("project"));
    let analysis = language.analyze(
        main.to_str().unwrap(),
        text,
        botwork::core::format::SourceKind::Script,
    );
    let definitions = analysis.definition(text.find("Double").unwrap());
    assert_eq!(definitions.len(), 1, "{definitions:?}");
    assert_eq!(Path::new(&definitions[0].file), module.join("math.botwork"));
}

#[test]
fn files_imported_by_url_are_pinned_fetched_once_and_named_by_their_url() {
    let world = World::new();
    let server = Server::empty();
    let helper_url = server.url("lib/helper.botwork");
    let library_url = server.url("lib/library.botwork");
    let relative_url = server.url("lib/relative.botwork");
    let wasm_url = server.url("tools/statements.wasm");
    let helper = "Shout |x| {\n    Return |x + \"!\"|\n}\n".to_owned();
    let library = format!(
        "Import |\"{helper_url}\"| As |helper|\nLoud |x| {{\n    Return |@{{ helper::Shout |x| }}|\n}}\nBroken {{\n    Fail |\"from the library\"|\n}}\n"
    );
    let relative = "Import |\"helper.botwork\"| As |helper|\n".to_owned();
    let wasm =
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wasm/statements.wasm")).unwrap();
    let files = [
        (&helper_url, helper.into_bytes()),
        (&library_url, library.into_bytes()),
        (&relative_url, relative.into_bytes()),
        (&wasm_url, wasm),
    ];
    let mut manifest = String::from("[files]\n");
    for (url, body) in &files {
        server.serve(
            url.trim_start_matches(&format!("http://127.0.0.1:{}", server.port)),
            body.clone(),
        );
        manifest.push_str(&format!("\"{url}\" = \"{}\"\n", sha256(body)));
    }
    world.write("project/botwork.toml", &manifest);
    world.write(
        "project/main.botwork",
        &format!("Import |\"{library_url}\"| As |library|\nImport |\"{wasm_url}\"| As |wasm|\nLog |@{{ library::Loud |\"hi\"| }}|\nLog |@{{ wasm::Echo |[3]| }}|\n"),
    );
    // Nothing runs before a fetch.
    failed_with(
        &world.run("project", "main.botwork"),
        "is not in the cache; run `botwork --fetch`",
    );
    succeeded(&world.fetch("project", &[]));
    let lock = fs::read_to_string(world.path("project/botwork.lock")).unwrap();
    assert_eq!(lock.matches("[[file]]").count(), 4, "{lock}");
    let downloads = server.requests.load(Ordering::SeqCst);
    assert_eq!(downloads, 4);
    let run = world.run("project", "main.botwork");
    succeeded(&run);
    assert_eq!(stdout(&run), "hi!\n[3]\n");
    // The cache serves later fetches and runs.
    succeeded(&world.fetch("project", &["--offline", "--locked"]));
    assert_eq!(server.requests.load(Ordering::SeqCst), downloads);
    // Offline, an empty cache is an error, and nothing is downloaded.
    fs::remove_dir_all(world.path("cache/files")).unwrap();
    failed_with(
        &world.fetch("project", &["--offline"]),
        "is not in the cache, and --offline forbids fetching it",
    );
    assert_eq!(server.requests.load(Ordering::SeqCst), downloads);
    succeeded(&world.fetch("project", &[]));
    // A module imported by URL is named by its URL in diagnostics and checks.
    world.write(
        "project/broken.botwork",
        &format!("Import |\"{library_url}\"| As |library|\nlibrary::Broken\n"),
    );
    let broken = world.run("project", "broken.botwork");
    failed_with(&broken, "from the library");
    assert!(
        stderr(&broken).contains(&format!("{library_url}:6:5")),
        "{}",
        stderr(&broken)
    );
    let check = world.check("project", "main.botwork");
    succeeded(&check);
    assert!(
        stderr(&check).contains("1 file, 2 modules: 0 errors"),
        "{}",
        stderr(&check)
    );
    // It may import only URLs and packages.
    world.write(
        "project/relative.botwork",
        &format!("Import |\"{relative_url}\"| As |relative|\n"),
    );
    failed_with(
        &world.run("project", "relative.botwork"),
        "a module imported by URL imports only URLs and packages",
    );
    failed_with(
        &world.check("project", "relative.botwork"),
        "a module imported by URL imports only URLs and packages",
    );
    // Undeclared URLs are refused, by runs and checks alike.
    let undeclared = server.url("lib/other.botwork");
    world.write(
        "project/other.botwork",
        &format!("Import |\"{undeclared}\"| As |other|\n"),
    );
    failed_with(
        &world.run("project", "other.botwork"),
        "is not in the [files] of",
    );
    failed_with(
        &world.check("project", "other.botwork"),
        "is not in the [files] of",
    );
    // A file that is not the pinned one fails the fetch.
    fs::remove_dir_all(world.path("cache/files")).unwrap();
    world.write(
        "project/botwork.toml",
        &manifest.replace(&sha256(&files[0].1), &"0".repeat(64)),
    );
    failed_with(&world.fetch("project", &[]), "integrity check failed");
    // Packages cannot name files.
    world.package(
        "kit",
        &format!(
            "[package]\nname = \"kit\"\nversion = \"1.0.0\"\n[files]\n\"{helper_url}\" = \"{}\"\n",
            sha256(&files[0].1)
        ),
        &[],
    );
    world.write(
        "project/botwork.toml",
        "[dependencies]\nkit = { path = \"../kit\" }\n",
    );
    failed_with(
        &world.fetch("project", &[]),
        "only a project's botwork.toml can",
    );
    // Without a project, a URL import says what it needs.
    world.write(
        "loose/main.botwork",
        &format!("Import |\"{library_url}\"| As |library|\n"),
    );
    failed_with(
        &world.run("loose", "main.botwork"),
        "package and URL imports need a botwork.toml",
    );
}
