//! The package cache: each fetched package's files in a directory named by
//! their tree hash, written once and then only read, so runs need no network.
use super::*;
use std::fs;

/// The cache directory: `BOTWORK_CACHE_DIR`, else the platform's user cache
/// directory's `botwork`.
pub fn root() -> Result<PathBuf, String> {
    if let Some(directory) = std::env::var_os("BOTWORK_CACHE_DIR").filter(|value| !value.is_empty())
    {
        return Ok(PathBuf::from(directory));
    }
    let variable = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        variable("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        variable("HOME").map(|home| home.join("Library/Caches"))
    } else {
        variable("XDG_CACHE_HOME").or_else(|| variable("HOME").map(|home| home.join(".cache")))
    };
    base.map(|base| base.join("botwork"))
        .ok_or_else(|| "no cache directory: set BOTWORK_CACHE_DIR".to_owned())
}

/// Where the package with tree hash `tree` lives in the cache under `root`.
pub fn package(root: &Path, tree: &str) -> Result<PathBuf, String> {
    tree::digest(tree)?;
    Ok(root.join("packages").join(tree))
}

/// A new, empty working directory in the cache, removed when dropped.
pub fn scratch(root: &Path) -> Result<tempfile::TempDir, String> {
    let packages = root.join("packages");
    fs::create_dir_all(&packages).map_err(|error| format!("{}: {error}", packages.display()))?;
    tempfile::Builder::new()
        .prefix(".fetch-")
        .tempdir_in(&packages)
        .map_err(|error| format!("{}: {error}", packages.display()))
}

/// Move the package files at `files` into the cache, named by their tree
/// hash, and return the hash. When `expected` is given, a different hash is
/// refused. An entry already present is kept: same hash, same files.
pub fn store(root: &Path, files: &Path, expected: Option<&str>) -> Result<String, String> {
    let tree = tree_hash(files)?;
    if let Some(expected) = expected {
        if tree != expected {
            return Err(format!(
                "integrity check failed: the files hash to {tree}, but botwork.lock pins {expected}"
            ));
        }
    }
    let target = package(root, &tree)?;
    if !target.is_dir() {
        match fs::rename(files, &target) {
            Ok(()) => {}
            // Another fetch stored the same files first.
            Err(_) if target.is_dir() => {}
            Err(error) => return Err(format!("{}: {error}", target.display())),
        }
    }
    Ok(tree)
}

/// Where the file `name`, with SHA-256 `sha256`, lives in the cache under
/// `root`.
pub fn file(root: &Path, sha256: &str, name: &str) -> PathBuf {
    root.join("files").join(sha256).join(name)
}

/// Store `bytes` as the file `name`, after checking that they hash to
/// `sha256`, which names their directory. The file appears whole or not at all.
pub fn store_file(root: &Path, bytes: &[u8], sha256: &str, name: &str) -> Result<PathBuf, String> {
    let target = file(root, sha256, name);
    if target.is_file() {
        return Ok(target);
    }
    let directory = target.parent().expect("a file in a directory");
    fs::create_dir_all(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let mut scratch = tempfile::Builder::new()
        .prefix(".fetch-")
        .tempfile_in(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    std::io::Write::write_all(&mut scratch, bytes)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    scratch
        .persist(&target)
        .map_err(|error| format!("{}: {}", target.display(), error.error))?;
    Ok(target)
}
