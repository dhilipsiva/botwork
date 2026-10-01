use super::*;
use std::{fs, io::Write};

#[test]
fn manifests_name_packages_versions_and_one_source_per_dependency() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "acme-checks"
version = "1.2.0"
botwork = ">=0.2"

[dependencies]
helpers = { path = "../helpers" }
kit = { git = "https://example.com/kit.git", tag = "v1.4.0", version = "^1.4" }
pricing = { url = "https://example.com/pricing.tar.gz", sha256 = "0000000000000000000000000000000000000000000000000000000000000000" }
"#,
    )
    .unwrap();
    let package = manifest.package.unwrap();
    assert_eq!(
        (package.name.as_str(), package.version.to_string()),
        ("acme-checks", "1.2.0".into())
    );
    assert!(package.botwork.unwrap().matches(&Version::new(0, 2, 0)));
    assert_eq!(
        manifest.dependencies["helpers"].source,
        Source::Path("../helpers".into())
    );
    assert_eq!(
        manifest.dependencies["kit"].source.to_string(),
        "git https://example.com/kit.git tag v1.4.0"
    );
    assert!(manifest.dependencies["kit"]
        .version
        .as_ref()
        .unwrap()
        .matches(&Version::new(1, 4, 2)));
    // A project need not be a package.
    assert!(Manifest::parse("[dependencies]\n")
        .unwrap()
        .package
        .is_none());
}

#[test]
fn malformed_manifests_are_refused_with_the_reason() {
    for (text, reason) in [
        ("[package]\nname = \"Acme\"\nversion = \"1.0.0\"\n", "is not a package name"),
        ("[package]\nname = \"acme\"\nversion = \"one\"\n", "package version `one`"),
        ("[package]\nname = \"acme\"\nversion = \"1.0.0\"\nbotwork = \"many\"\n", "Botwork version requirement"),
        ("[packages]\n", "unknown field"),
        ("[dependencies]\nx = { path = \"a\", git = \"https://e.com/x.git\", tag = \"v1\" }\n", "exactly one source"),
        ("[dependencies]\nx = { git = \"https://e.com/x.git\" }\n", "exactly one of `tag`, `rev`, or `branch`"),
        ("[dependencies]\nx = { git = \"https://e.com/x.git\", rev = \"abc\" }\n", "40-digit"),
        ("[dependencies]\nx = { git = \"https://e.com/x.git\", tag = \"--upload-pack=x\" }\n", "is not a git reference name"),
        ("[dependencies]\nx = { git = \"ext::sh -c touch% /tmp/x\", tag = \"v1\" }\n", "`ext`"),
        ("[dependencies]\nx = { url = \"https://e.com/x.tar.gz\" }\n", "needs `sha256`"),
        ("[dependencies]\nx = { url = \"http://example.com/x.tar.gz\", sha256 = \"0000000000000000000000000000000000000000000000000000000000000000\" }\n", "`http` only on this machine"),
        ("[dependencies]\nBad = { path = \"x\" }\n", "is not a package name"),
    ] {
        let error = Manifest::parse(text).unwrap_err();
        assert!(error.contains(reason), "{text}: {error}");
    }
    // Plain HTTP to this machine is allowed: the archive hash pins the content.
    Manifest::parse("[dependencies]\nx = { url = \"http://127.0.0.1:8000/x.tar.gz\", sha256 = \"0000000000000000000000000000000000000000000000000000000000000000\" }\n").unwrap();
}

#[test]
fn package_names_are_lowercase_words_joined_by_single_hyphens() {
    for name in ["a", "http-kit", "kit2", "a-b-c"] {
        assert!(valid_name(name), "{name}");
    }
    for name in [
        "",
        "Kit",
        "2kit",
        "-kit",
        "kit-",
        "kit--x",
        "kit_x",
        "kit.x",
        &"a".repeat(65),
    ] {
        assert!(!valid_name(name), "{name}");
    }
}

#[test]
fn package_paths_name_a_file_inside_the_package() {
    let (name, file) = package_path("@kit/lib/auth.botwork").unwrap().unwrap();
    assert_eq!(
        (name, file),
        ("kit", PathBuf::from("lib").join("auth.botwork"))
    );
    assert!(package_path("lib/auth.botwork").is_none());
    for (path, reason) in [
        ("@kit", "no file in it"),
        ("@kit/", "no file in it"),
        ("@kit/../secret.botwork", "leaves the package"),
        ("@kit/lib/../../x.botwork", "leaves the package"),
        ("@Kit/x.botwork", "is not a package name"),
        ("@kit/c:\\x.botwork", "not a path inside the package"),
    ] {
        let error = package_path(path).unwrap().unwrap_err();
        assert!(error.contains(reason), "{path}: {error}");
    }
}

#[test]
fn lockfiles_render_the_same_bytes_for_the_same_resolution_and_read_back() {
    let package = |name: &str, dependencies: &[&str]| Locked {
        name: name.into(),
        version: "1.0.0".into(),
        source: format!("git https://example.com/{name}.git tag v1"),
        path: None,
        commit: Some("0".repeat(40)),
        tree: Some(format!("sha256-{}", "a".repeat(64))),
        dependencies: dependencies.iter().map(|name| name.to_string()).collect(),
    };
    let file = |url: &str| LockedFile {
        url: url.into(),
        sha256: "0".repeat(64),
    };
    let one = Lock {
        version: 1,
        packages: vec![package("b", &["z", "a"]), package("a", &[])],
        files: vec![
            file("https://b.example/x.botwork"),
            file("https://a.example/x.botwork"),
        ],
    };
    let two = Lock {
        version: 1,
        packages: vec![package("a", &[]), package("b", &["a", "z"])],
        files: vec![
            file("https://a.example/x.botwork"),
            file("https://b.example/x.botwork"),
        ],
    };
    assert_eq!(one.render(), two.render());
    let read = Lock::parse(&one.render()).unwrap();
    assert_eq!(read.packages[0].name, "a");
    assert_eq!(read.packages[1].dependencies, ["a", "z"]);
    assert!(Lock::parse("version = 2\n")
        .unwrap_err()
        .contains("lockfile version 2"));
    let twice = format!(
        "{}\n[[package]]\nname = \"a\"\nversion = \"1.0.0\"\nsource = \"x\"\n",
        one.render()
    );
    assert!(Lock::parse(&twice).unwrap_err().contains("listed twice"));
    let bad = one
        .render()
        .replace(&format!("sha256-{}", "a".repeat(64)), "sha256-xyz");
    assert!(Lock::parse(&bad).unwrap_err().contains("tree hash"));
}

#[test]
fn tree_hashes_follow_content_and_paths_not_order_or_time() {
    let first = tempfile::tempdir().unwrap();
    fs::create_dir(first.path().join("lib")).unwrap();
    fs::write(first.path().join("lib/a.botwork"), "Log |1|\n").unwrap();
    fs::write(first.path().join("botwork.toml"), "[package]\n").unwrap();
    let second = tempfile::tempdir().unwrap();
    fs::write(second.path().join("botwork.toml"), "[package]\n").unwrap();
    fs::create_dir(second.path().join("lib")).unwrap();
    fs::write(second.path().join("lib/a.botwork"), "Log |1|\n").unwrap();
    let hash = tree_hash(first.path()).unwrap();
    assert_eq!(hash, tree_hash(second.path()).unwrap());
    assert!(tree::digest(&hash).is_ok());
    fs::write(second.path().join("lib/a.botwork"), "Log |2|\n").unwrap();
    assert_ne!(hash, tree_hash(second.path()).unwrap());
    fs::write(second.path().join("lib/a.botwork"), "Log |1|\n").unwrap();
    fs::rename(
        second.path().join("lib/a.botwork"),
        second.path().join("lib/b.botwork"),
    )
    .unwrap();
    assert_ne!(hash, tree_hash(second.path()).unwrap());
    // An empty directory adds nothing.
    fs::rename(
        second.path().join("lib/b.botwork"),
        second.path().join("lib/a.botwork"),
    )
    .unwrap();
    fs::create_dir(second.path().join("empty")).unwrap();
    assert_eq!(hash, tree_hash(second.path()).unwrap());
}

/// A tar archive with raw entries, so tests can name paths that the tar
/// crate's builder would refuse.
fn archive(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (path, kind, content) in entries {
        let mut header = tar::Header::new_old();
        let name = &mut header.as_old_mut().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_entry_type(*kind);
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        if *kind == tar::EntryType::Symlink {
            header.set_link_name("/etc/passwd").unwrap();
        }
        header.set_cksum();
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(content);
        bytes.resize(bytes.len().div_ceil(512) * 512, 0);
    }
    bytes.resize(bytes.len() + 1024, 0);
    bytes
}

#[test]
fn extraction_writes_only_files_and_directories_inside_the_destination() {
    let directory = tempfile::tempdir().unwrap();
    let good = archive(&[
        ("pkg/", tar::EntryType::Directory, b""),
        ("pkg/botwork.toml", tar::EntryType::Regular, b"[package]\n"),
        ("pkg/lib/a.botwork", tar::EntryType::Regular, b"Log |1|\n"),
    ]);
    let target = directory.path().join("good");
    tree::extract(good.as_slice(), &target).unwrap();
    // A release tarball's single top-level directory is the package root.
    let root = tree::root(&target).unwrap();
    assert_eq!(root, target.join("pkg"));
    assert_eq!(
        fs::read_to_string(root.join("lib/a.botwork")).unwrap(),
        "Log |1|\n"
    );
    for (name, entries, reason) in [
        (
            "parent",
            vec![("../escape.botwork", tar::EntryType::Regular, &b"x"[..])],
            "outside the package",
        ),
        (
            "absolute",
            vec![("/tmp/escape.botwork", tar::EntryType::Regular, &b"x"[..])],
            "outside the package",
        ),
        (
            "nested",
            vec![("a/../../escape", tar::EntryType::Regular, &b"x"[..])],
            "outside the package",
        ),
        (
            "stream",
            vec![("a.botwork:hidden", tar::EntryType::Regular, &b"x"[..])],
            "outside the package",
        ),
        (
            "symlink",
            vec![("link", tar::EntryType::Symlink, &b""[..])],
            "only regular files and directories",
        ),
        (
            "hardlink",
            vec![("link", tar::EntryType::Link, &b""[..])],
            "only regular files and directories",
        ),
        (
            "twice",
            vec![
                ("a", tar::EntryType::Regular, &b"x"[..]),
                ("a", tar::EntryType::Regular, &b"y"[..]),
            ],
            "a",
        ),
    ] {
        let target = directory.path().join(name);
        let error = tree::extract(archive(&entries).as_slice(), &target).unwrap_err();
        assert!(error.contains(reason), "{name}: {error}");
        assert!(!directory.path().join("escape.botwork").exists());
    }
}

#[test]
fn cached_files_must_hash_to_the_pinned_tree() {
    let cache = tempfile::tempdir().unwrap();
    let scratch = cache::scratch(cache.path()).unwrap();
    let files = scratch.path().join("files");
    fs::create_dir(&files).unwrap();
    fs::File::create(files.join("botwork.toml"))
        .unwrap()
        .write_all(b"[package]\n")
        .unwrap();
    let pinned = format!("sha256-{}", "0".repeat(64));
    let error = cache::store(cache.path(), &files, Some(&pinned)).unwrap_err();
    assert!(error.contains("integrity check failed"), "{error}");
    let tree = cache::store(cache.path(), &files, None).unwrap();
    assert!(cache::package(cache.path(), &tree)
        .unwrap()
        .join("botwork.toml")
        .is_file());
}

#[test]
fn projects_pin_the_files_they_import_by_url() {
    let digest = "a".repeat(64);
    let manifest = Manifest::parse(&format!(
        "[files]\n\"https://example.com/lib/math.botwork\" = \"{digest}\"\n\"http://localhost:8000/tools.wasm\" = \"{digest}\"\n"
    ))
    .unwrap();
    assert_eq!(manifest.files.len(), 2);
    assert_eq!(
        manifest::file_name("https://example.com/lib/math.botwork").unwrap(),
        "math.botwork"
    );
    for (entry, reason) in [
        (
            "\"https://example.com/lib/math.botwork\" = \"abc\"",
            "is not a SHA-256",
        ),
        (
            &format!("\"https://example.com/lib/\" = \"{digest}\""),
            "names no file",
        ),
        (
            &format!("\"https://example.com/notes.txt\" = \"{digest}\""),
            "must name a .botwork",
        ),
        (
            &format!("\"https://example.com/a%2Fb.botwork\" = \"{digest}\""),
            "must name a .botwork",
        ),
        (
            &format!("\"https://example.com/.hidden.botwork\" = \"{digest}\""),
            "must name a .botwork",
        ),
        (
            &format!("\"http://example.com/x.botwork\" = \"{digest}\""),
            "`http` only on this machine",
        ),
        (
            &format!("\"ftp://example.com/x.botwork\" = \"{digest}\""),
            "uses `ftp`",
        ),
    ] {
        let error = Manifest::parse(&format!("[files]\n{entry}\n")).unwrap_err();
        assert!(error.contains(reason), "{entry}: {error}");
    }
    for (text, url) in [
        ("https://example.com/x.botwork", true),
        ("lib/x.botwork", false),
        ("@kit/x.botwork", false),
        ("c:\\x.botwork", false),
    ] {
        assert_eq!(is_url(text), url, "{text}");
    }
}
