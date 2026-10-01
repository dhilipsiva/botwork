//! Packages (decision D10): a `botwork.toml` manifest names a project's
//! dependencies, `botwork --fetch` resolves them into a `botwork.lock` with
//! content hashes and an offline cache, and scripts import their files as
//! `@name/path`. Sources are local paths, git repositories pinned to commits,
//! and URLs of gzipped tarballs pinned by SHA-256. See docs/packages.md.
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    path::{Component, Path, PathBuf},
};

mod cache;
mod download;
mod lock;
mod manifest;
mod project;
mod resolve;
mod source;
#[cfg(test)]
mod tests;
mod tree;

pub use lock::{Lock, Locked, LockedFile, LOCKFILE};
pub use manifest::{is_url, valid_name, Dependency, GitRef, Manifest, Package, Source, MANIFEST};
pub use project::{import_file, package_path, Project};
pub use resolve::{fetch, FetchOptions, FetchReport};
pub use tree::tree_hash;

/// The largest manifest or lockfile read.
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
/// The most dependencies one manifest names.
const MAX_DEPENDENCIES: usize = 256;
/// The most packages one project resolves, its dependencies' included.
const MAX_PACKAGES: usize = 1024;
/// The longest source URL.
const MAX_URL_BYTES: usize = 4096;
/// The most bytes one package's files, or one downloaded archive, may hold.
const MAX_TREE_BYTES: u64 = 256 * 1024 * 1024;
/// The most files and directories one package may hold.
const MAX_TREE_ENTRIES: usize = 100_000;

/// The Botwork version that packages' `botwork` requirements are checked
/// against.
pub fn botwork_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is semver")
}
