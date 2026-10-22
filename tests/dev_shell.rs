//! The Nix dev shells (`flake.nix`) and the Justfile pin the versions CI and the campaign scripts
//! use, so a run in the shell is the run CI makes.

use std::{collections::BTreeSet, fs, path::Path};

fn read(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

/// The text between the first `before` after `after` and the next `end`.
fn between<'a>(text: &'a str, after: &str, before: &str, end: &str) -> &'a str {
    let start = text
        .find(after)
        .unwrap_or_else(|| panic!("missing {after}"));
    let rest = &text[start..];
    let value = &rest[rest.find(before).unwrap() + before.len()..];
    &value[..value.find(end).unwrap()]
}

/// Every `nightly-YYYY-MM-DD` in `text`.
fn nightlies(text: &str) -> BTreeSet<&str> {
    text.match_indices("nightly-")
        .filter_map(|(at, _)| text.get(at..at + "nightly-YYYY-MM-DD".len()))
        .collect()
}

#[test]
fn the_nightly_is_the_one_ci_and_the_fuzz_campaign_use() {
    let flake = read("flake.nix");
    let nightly = format!("nightly-{}", between(&flake, "nightlyDate =", "\"", "\""));
    assert!(flake.contains("pkgs.rust-bin.nightly.${nightlyDate}"));
    assert_eq!(
        nightlies(&read(".github/workflows/ci.yml")),
        BTreeSet::from([nightly.as_str()])
    );
    assert_eq!(
        between(&read("scripts/fuzz_smoke.py"), "NIGHTLY =", "\"", "\""),
        nightly
    );
}

#[test]
fn the_stable_toolchain_is_an_exact_release_with_the_tools_the_checks_need() {
    let flake = read("flake.nix");
    let version = between(&flake, "rustVersion =", "\"", "\"");
    let parts: Vec<_> = version.split('.').collect();
    assert_eq!(parts.len(), 3, "{version}");
    assert!(
        parts.iter().all(|part| part.parse::<u32>().is_ok()),
        "{version}"
    );
    assert!(flake.contains("pkgs.rust-bin.stable.${rustVersion}.default"));
    // D20: the shell's stable is the rust-version this release declares.
    assert_eq!(
        between(&read("Cargo.toml"), "\nrust-version =", "\"", "\""),
        version
    );
    for component in ["clippy", "rustfmt", "llvm-tools-preview", "wasm32-wasip2"] {
        assert!(flake.contains(&format!("\"{component}\"")), "{component}");
    }
}

#[test]
fn the_campaign_tools_are_the_versions_the_scripts_require() {
    let flake = read("flake.nix");
    let llvm_cov = between(&flake, "pname = \"cargo-llvm-cov\"", "version = \"", "\"");
    assert_eq!(
        between(&read("scripts/coverage.py"), "TOOL_VERSION =", "\"", "\""),
        format!("cargo-llvm-cov {llvm_cov}")
    );
    let mutants = between(&flake, "cargo-mutants =", "version == \"", "\"");
    assert!(read("scripts/mutation_core.py").contains(&format!("\"cargo-mutants {mutants}\"")));
    let fuzz = between(&flake, "cargo-fuzz =", "version == \"", "\"");
    assert_eq!(
        between(&read("scripts/fuzz_smoke.py"), "FUZZ_VERSION =", "\"", "\""),
        format!("cargo-fuzz {fuzz}")
    );
}

#[test]
fn the_justfile_runs_every_helper_check_ci_runs() {
    let justfile = read("Justfile");
    let recipes: BTreeSet<_> = justfile
        .lines()
        .filter(|line| !line.starts_with([' ', '#']) && line.contains(':') && !line.contains(":="))
        .map(|line| line.split([' ', ':']).next().unwrap())
        .collect();
    for recipe in [
        "check",
        "test",
        "ci",
        "coverage",
        "mutants",
        "fuzz-smoke",
        "public-api",
        "release-dry-run",
    ] {
        assert!(recipes.contains(recipe), "{recipe}: {recipes:?}");
    }
    let check = between(&justfile, "\ncheck:", "\n", "\n\n");
    let ci = read(".github/workflows/ci.yml");
    let helpers: Vec<_> = fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with("_tools.py"))
        .collect();
    assert!(helpers.len() >= 7, "{helpers:?}");
    for helper in helpers {
        let command = format!("python3 tests/{helper}");
        assert!(check.contains(&command), "just check does not run {helper}");
        assert!(ci.contains(&command), "CI does not run {helper}");
    }
    for command in [
        "cargo fmt --all -- --check",
        "cargo clippy --locked --all-targets -- -D warnings",
        "python3 scripts/wasm_guest.py check",
    ] {
        assert!(check.contains(command), "{command}");
    }
}
