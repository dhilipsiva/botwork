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
    #[serde(default, rename = "file", skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<LockedFile>,
}

/// One file fetched by URL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedFile {
    pub url: String,
    pub sha256: String,
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
        // The version comes first, so a newer lockfile's new keys are reported
        // as its version rather than as unknown fields.
        let table: toml::Table =
            toml::from_str(text).map_err(|error| error.message().to_owned())?;
        match table.get("version").map(toml::Value::as_integer) {
            Some(Some(version)) if version == i64::from(Self::VERSION) => {}
            Some(Some(version)) if version > i64::from(Self::VERSION) => {
                return Err(format!(
                    "lockfile version {version} is from a newer Botwork; this one reads version {}. \
                     Upgrade Botwork, or delete {LOCKFILE} and run `botwork --fetch` to lock the project again",
                    Self::VERSION
                ))
            }
            _ => {
                return Err(format!(
                    "no lockfile version Botwork reads (version {}); delete {LOCKFILE} and run `botwork --fetch` to lock the project again",
                    Self::VERSION
                ))
            }
        }
        let lock: Self = table
            .try_into()
            .map_err(|error: toml::de::Error| error.message().to_owned())?;
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
        lock.files.sort_by(|left, right| left.url.cmp(&right.url));
        format!(
            "{HEADER}{}",
            toml::to_string(&lock).expect("a lock serializes")
        )
    }

    pub fn get(&self, name: &str) -> Option<&Locked> {
        self.packages.iter().find(|package| package.name == name)
    }
}
