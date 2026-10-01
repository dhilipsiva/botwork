//! Package content: the SHA-256 tree hash that pins it, and extraction of a
//! tar archive that cannot write outside its destination.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
};

const PREFIX: &str = "sha256-";

/// The hex digest of a `sha256-…` tree hash.
pub fn digest(tree: &str) -> Result<&str, String> {
    tree.strip_prefix(PREFIX)
        .filter(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| format!("`{tree}` is not a `sha256-` tree hash"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// The tree hash of the files under `root`: SHA-256 over one line per regular
/// file, in byte order of its `/`-separated path, holding the path, a NUL, and
/// the file's own SHA-256. Directories count only through their files; any
/// other kind of entry, such as a symbolic link, is refused.
pub fn tree_hash(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort();
    let mut tree = Sha256::new();
    for (path, file) in files {
        let mut content = Sha256::new();
        let mut reader =
            fs::File::open(&file).map_err(|error| format!("{}: {error}", file.display()))?;
        io::copy(&mut reader, &mut content)
            .map_err(|error| format!("{}: {error}", file.display()))?;
        tree.update(path.as_bytes());
        tree.update([0]);
        tree.update(hex(&content.finalize()).as_bytes());
        tree.update(b"\n");
    }
    Ok(format!("{PREFIX}{}", hex(&tree.finalize())))
}

fn walk(root: &Path, directory: &Path, files: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries =
        fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", directory.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if kind.is_dir() {
            walk(root, &path, files)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("walked under the root")
                .components()
                .map(|component| component.as_os_str().to_str())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| format!("{}: the path is not UTF-8", path.display()))?
                .join("/");
            if files.len() >= MAX_TREE_ENTRIES {
                return Err(format!("more than {MAX_TREE_ENTRIES} files"));
            }
            files.push((relative, path));
        } else {
            return Err(format!(
                "{}: packages hold only regular files and directories",
                path.display()
            ));
        }
    }
    Ok(())
}

/// The relative path an archive entry names, if it is a safe one: only
/// ordinary components, none of which a platform reads specially.
fn entry_path(path: &Path) -> Option<PathBuf> {
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                let name = name.to_str()?;
                // Backslashes and colons separate paths, drives, and streams on
                // Windows; a trailing dot or space is dropped there.
                if name.contains(['\\', ':']) || name.ends_with(['.', ' ']) {
                    return None;
                }
                safe.push(name);
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => return None,
        }
    }
    (!safe.as_os_str().is_empty()).then_some(safe)
}

/// Extract the tar archive `reader` into the new directory `destination`.
/// Only regular files and directories are written, each inside the
/// destination; links, devices, absolute paths, and `..` are refused, as is
/// more than the package size and entry limits. File modes are not kept.
pub fn extract(reader: impl Read, destination: &Path) -> Result<(), String> {
    fs::create_dir(destination).map_err(|error| format!("{}: {error}", destination.display()))?;
    let mut archive = tar::Archive::new(reader);
    let mut written: u64 = 0;
    let mut count = 0;
    let entries = archive
        .entries()
        .map_err(|error| format!("reading the archive: {error}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("reading the archive: {error}"))?;
        let kind = entry.header().entry_type();
        // Global headers carry metadata such as git's commit id, not files.
        if kind == tar::EntryType::XGlobalHeader {
            continue;
        }
        let name = entry
            .path()
            .map_err(|error| format!("reading the archive: {error}"))?
            .into_owned();
        let relative = entry_path(&name).ok_or_else(|| {
            format!(
                "the archive names `{}`, outside the package",
                name.display()
            )
        })?;
        count += 1;
        if count > MAX_TREE_ENTRIES {
            return Err(format!(
                "the archive holds more than {MAX_TREE_ENTRIES} entries"
            ));
        }
        let target = destination.join(&relative);
        match kind {
            tar::EntryType::Directory => {
                fs::create_dir_all(&target).map_err(|error| format!("{}: {error}", target.display()))?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| format!("{}: {error}", parent.display()))?;
                }
                // A second entry for the same path is refused, not merged.
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .map_err(|error| format!("{}: {error}", target.display()))?;
                let remaining = MAX_TREE_BYTES - written;
                let copied = io::copy(&mut (&mut entry).take(remaining + 1), &mut file)
                    .map_err(|error| format!("{}: {error}", target.display()))?;
                if copied > remaining {
                    return Err(format!("the archive holds more than {MAX_TREE_BYTES} bytes"));
                }
                written += copied;
                file.flush().map_err(|error| format!("{}: {error}", target.display()))?;
            }
            other => {
                return Err(format!(
                    "the archive's `{}` is a {other:?} entry; packages hold only regular files and directories",
                    name.display()
                ))
            }
        }
    }
    Ok(())
}

/// The package root inside an extracted archive: the archive's single
/// top-level directory, as release tarballs have, or the archive itself.
pub fn root(extracted: &Path) -> Result<PathBuf, String> {
    let mut entries = fs::read_dir(extracted)
        .map_err(|error| format!("{}: {error}", extracted.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{}: {error}", extracted.display()))?;
    if entries.len() == 1 && entries[0].file_type().is_ok_and(|kind| kind.is_dir()) {
        return Ok(entries.remove(0).path());
    }
    Ok(extracted.to_owned())
}
