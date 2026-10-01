//! Which language adapters released binaries carry, as docs/distribution.md
//! states it (decisions D6 to D9): the release build uses the default
//! features, which hold WebAssembly and not Python, and JavaScript needs no
//! build feature at all.

use std::fs;

fn read(path: &str) -> String {
    fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

/// The cells of the adapter table's row whose first cell links `adapter`.
fn row(table: &str, adapter: &str) -> Vec<String> {
    let line = table
        .lines()
        .find(|line| line.starts_with(&format!("| [{adapter}](")))
        .unwrap_or_else(|| panic!("no row for {adapter}"));
    line.trim_matches('|')
        .split(" | ")
        .map(|cell| cell.trim().to_owned())
        .collect()
}

#[test]
fn released_binaries_carry_the_default_features_which_hold_webassembly_alone() {
    let manifest = read("Cargo.toml");
    let (_, features) = manifest.split_once("\n[features]\n").unwrap();
    let features = features.split("\n[").next().unwrap();
    assert!(
        features.lines().any(|line| line == r#"default = ["wasm"]"#),
        "{features}"
    );
    assert!(features.lines().any(|line| line.starts_with("python = ")));
    // The release workflow builds the binary with exactly those features.
    let release = read(".github/workflows/release.yml");
    let builds: Vec<_> = release
        .lines()
        .filter(|line| line.contains("cargo +stable build") && line.contains("--bin botwork"))
        .collect();
    assert_eq!(builds.len(), 1, "{builds:?}");
    for flag in ["--features", "--no-default-features", "--all-features"] {
        assert!(!builds[0].contains(flag), "{}", builds[0]);
    }
}

#[test]
fn the_distribution_guide_says_which_adapters_releases_carry() {
    let guide = read("docs/distribution.md");
    let (_, section) = guide
        .split_once("## Language adapters and the single binary")
        .unwrap();
    let table = section.split("\n## ").next().unwrap();
    for (adapter, released, single) in [
        ("WebAssembly", "Yes", "Yes; Wasmtime adds about 16 MB"),
        ("JavaScript", "Yes", "Yes, with Node installed beside it"),
        (
            "Python",
            "No",
            "No: it loads libpython, a shared library, at start",
        ),
    ] {
        let cells = row(table, adapter);
        assert_eq!(cells.len(), 5, "{cells:?}");
        assert_eq!(cells[1], released, "{adapter}");
        assert_eq!(cells[4], single, "{adapter}");
    }
    // The extension guide's runtimes agree.
    let extending = read("docs/extending.md");
    for (adapter, runtime) in [
        ("WASM", "None: part of the binary"),
        ("Python", "libpython 3.10 or later"),
        ("JavaScript", "Node.js"),
    ] {
        assert!(
            extending
                .lines()
                .any(|line| line.starts_with(&format!("| {adapter} |"))
                    && line.contains(&format!("| {runtime} |"))),
            "{adapter}: {runtime}"
        );
    }
}
