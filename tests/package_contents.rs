//! The published crate carries what the library and CLI need to build, and nothing else:
//! `Cargo.toml`'s `include` list is `src/` plus exactly the files the sources read at compile time.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap()
}

fn rust_files(directory: &Path, into: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, into);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            into.push(path);
        }
    }
}

/// The quoted paths that follow each `marker` in `text`.
fn quoted<'a>(text: &'a str, marker: &str) -> Vec<&'a str> {
    text.match_indices(marker)
        .map(|(at, _)| {
            let rest = &text[at + marker.len()..];
            &rest[..rest.find('"').unwrap()]
        })
        .collect()
}

/// Package-relative paths of the files the sources embed: `include_str!` and `include_bytes!`
/// targets (relative to the source file), the Pest grammar, and the WIT world (relative to the
/// package root).
fn embedded() -> BTreeSet<String> {
    let root = root();
    let relative = |path: PathBuf| {
        fs::canonicalize(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
            .strip_prefix(&root)
            .unwrap()
            .to_str()
            .unwrap()
            .replace('\\', "/")
    };
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    let mut found = BTreeSet::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for marker in ["include_str!(\"", "include_bytes!(\""] {
            for path in quoted(&text, marker) {
                found.insert(relative(file.parent().unwrap().join(path)));
            }
        }
        for marker in ["#[grammar = \"", "bindgen!({ path: \""] {
            for path in quoted(&text, marker) {
                found.insert(relative(root.join(path)));
            }
        }
    }
    found
}

fn included() -> Vec<String> {
    let manifest: toml::Table = fs::read_to_string(root().join("Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    manifest["package"]["include"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pattern| pattern.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_crate_includes_every_file_the_sources_embed() {
    let included = included();
    assert!(included.contains(&"/src/**".to_owned()));
    let embedded = embedded();
    assert!(embedded.contains("src/core/grammar.pest"), "{embedded:?}");
    assert!(embedded.contains("wit/botwork.wit"), "{embedded:?}");
    for path in embedded.iter().filter(|path| !path.starts_with("src/")) {
        assert!(
            included.contains(&format!("/{path}")),
            "Cargo.toml's include list lacks /{path}, which the sources embed"
        );
    }
}

#[test]
fn the_crate_includes_nothing_else() {
    let embedded = embedded();
    let ordinary = [
        "/src/**",
        "/README.md",
        "/CHANGELOG.md",
        "/LICENSE-MIT",
        "/LICENSE-APACHE",
    ];
    for pattern in included() {
        if ordinary.contains(&pattern.as_str()) {
            continue;
        }
        let path = pattern.strip_prefix('/').unwrap();
        assert!(
            embedded.contains(path),
            "Cargo.toml includes {pattern}, which no source embeds"
        );
    }
    for license in ["LICENSE-MIT", "LICENSE-APACHE"] {
        assert!(root().join(license).is_file(), "{license}");
    }
}
