//! Canonical paths spelled the way users and editors write them.
//!
//! On Windows, `std::fs::canonicalize` returns verbatim paths such as
//! `\\?\C:\work\main.botwork`. Botwork names modules by their canonical path, and
//! shows those names in diagnostics, reports, and the language server, so it
//! drops the verbatim prefix whenever the path means the same without it.

use std::{
    io,
    path::{Path, PathBuf},
};

/// [`std::fs::canonicalize`], without a Windows verbatim prefix the path does
/// not need. Elsewhere it is `std::fs::canonicalize` itself.
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    if let Some(simple) = canonical.to_str().and_then(simplified) {
        return Ok(PathBuf::from(simple));
    }
    Ok(canonical)
}

/// Paths this long need the verbatim prefix unless long paths are enabled.
#[cfg(any(windows, test))]
const MAX_PATH: usize = 260;

/// The same path without its verbatim prefix, when Windows would read it the
/// same way: a drive or UNC path, shorter than `MAX_PATH`, whose components
/// are all ordinary names. Verbatim paths skip the normalization that would
/// otherwise drop trailing dots and spaces or map device names such as `NUL`.
#[cfg(any(windows, test))]
fn simplified(verbatim: &str) -> Option<String> {
    let (prefix, rest) = if let Some(rest) = verbatim.strip_prefix(r"\\?\UNC\") {
        (r"\\", rest)
    } else {
        let rest = verbatim.strip_prefix(r"\\?\")?;
        let drive = rest.as_bytes();
        if drive.len() < 3 || !drive[0].is_ascii_alphabetic() || &drive[1..3] != br":\" {
            return None;
        }
        ("", rest)
    };
    let simple = format!("{prefix}{rest}");
    let names = if prefix.is_empty() { &rest[3..] } else { rest };
    let ordinary = names
        .split('\\')
        .filter(|name| !name.is_empty())
        .all(ordinary_name);
    (ordinary && simple.len() < MAX_PATH).then_some(simple)
}

/// A file name Windows reads the same inside and outside a verbatim path.
#[cfg(any(windows, test))]
fn ordinary_name(name: &str) -> bool {
    const DEVICES: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !name
            .chars()
            .any(|character| character < ' ' || "<>:\"/|?*".contains(character))
        && !DEVICES
            .iter()
            .any(|device| stem.eq_ignore_ascii_case(device))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_and_unc_paths_lose_the_verbatim_prefix() {
        assert_eq!(
            simplified(r"\\?\C:\work\lib\main.botwork").as_deref(),
            Some(r"C:\work\lib\main.botwork")
        );
        assert_eq!(simplified(r"\\?\d:\").as_deref(), Some(r"d:\"));
        assert_eq!(
            simplified(r"\\?\UNC\server\share\main.botwork").as_deref(),
            Some(r"\\server\share\main.botwork")
        );
    }

    #[test]
    fn paths_that_need_the_prefix_keep_it() {
        for verbatim in [
            // Not verbatim, or not a drive or UNC path.
            r"C:\work\main.botwork",
            r"\\?\Volume{0b4e1c2a-0000-0000-0000-100000000000}\main.botwork",
            r"\\?\1:\main.botwork",
            r"\\?\C:",
            // Names Windows would change without the prefix.
            r"\\?\C:\work\trailing.",
            r"\\?\C:\work\trailing \main.botwork",
            r"\\?\C:\work\CON",
            r"\\?\C:\work\nul.botwork",
            r"\\?\C:\work\Com1 .txt",
            r"\\?\C:\work\a:b",
            r"\\?\C:\work\a?b",
            "\\\\?\\C:\\work\\tab\there",
            r"\\?\UNC\server\share\..\main.botwork",
        ] {
            assert_eq!(simplified(verbatim), None, "{verbatim}");
        }
    }

    #[test]
    fn long_paths_keep_the_prefix() {
        let within = format!(r"\\?\C:\{}", "a".repeat(MAX_PATH - 4));
        assert_eq!(
            simplified(&within).map(|path| path.len()),
            Some(MAX_PATH - 1)
        );
        let beyond = format!(r"\\?\C:\{}", "a".repeat(MAX_PATH - 3));
        assert_eq!(simplified(&beyond), None);
    }

    #[test]
    fn canonical_paths_resolve_links_and_dots() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("sub")).unwrap();
        std::fs::write(directory.path().join("main.botwork"), "").unwrap();
        let canonical = canonicalize(directory.path().join("sub/../main.botwork")).unwrap();
        assert_eq!(
            canonical,
            std::fs::canonicalize(directory.path())
                .map(|base| base.join("main.botwork"))
                .map(|path| {
                    path.to_str()
                        .and_then(simplified)
                        .map_or(path, PathBuf::from)
                })
                .unwrap()
        );
        assert!(!canonical.to_string_lossy().starts_with(r"\\?\"));
        assert!(canonicalize(directory.path().join("missing")).is_err());
    }
}
