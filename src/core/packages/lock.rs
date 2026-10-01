//! `botwork.lock`: every package a project resolved, with the commit or
//! archive it came from and its content hash, so a run uses exactly those
//! files and needs no network.
use super::*;

/// The lockfile beside a project's manifest.
pub const LOCKFILE: &str = "botwork.lock";
const HEADER: &str = "# Written by `botwork --fetch`: the packages this project resolved.\n# Commit it; do not edit it.\n";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    pub version: u32,
    #[serde(default, rename = "package", skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<Locked>,
}

/// One resolved package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Locked {
    pub name: String,
    pub version: String,
    /// The source as its dependents name it; a change of source refetches.
    pub source: String,
    /// For a path source, its directory relative to the project root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// For a git source, the commit its reference resolved to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// For git and url sources, the SHA-256 tree hash of the package's files,
    /// which names its directory in the cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
    /// The names of the packages it depends on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

impl Lock {
    pub const VERSION: u32 = 1;

    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_MANIFEST_BYTES {
            return Err(format!("larger than {MAX_MANIFEST_BYTES} bytes"));
        }
        let lock: Self = toml::from_str(text).map_err(|error| error.message().to_owned())?;
        if lock.version != Self::VERSION {
            return Err(format!(
                "lockfile version {} is not {}; run `botwork --fetch` to rewrite it",
                lock.version,
                Self::VERSION
            ));
        }
        let mut names = BTreeSet::new();
        for package in &lock.packages {
            if !valid_name(&package.name) || !names.insert(package.name.as_str()) {
                return Err(format!(
                    "package `{}` is invalid or listed twice",
                    package.name
                ));
            }
            if let Some(tree) = &package.tree {
                tree::digest(tree)?;
            }
        }
        Ok(lock)
    }

    /// The text `botwork --fetch` writes: packages by name, each list sorted,
    /// so the same resolution always writes the same bytes.
    pub fn render(&self) -> String {
        let mut lock = self.clone();
        lock.packages
            .sort_by(|left, right| left.name.cmp(&right.name));
        for package in &mut lock.packages {
            package.dependencies.sort();
        }
        format!(
            "{HEADER}{}",
            toml::to_string(&lock).expect("a lock serializes")
        )
    }

    pub fn get(&self, name: &str) -> Option<&Locked> {
        self.packages.iter().find(|package| package.name == name)
    }
}
