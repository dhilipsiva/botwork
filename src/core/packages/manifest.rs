//! `botwork.toml`: a project's or package's name, version, the Botwork versions
//! it works with, and its dependencies.
use super::*;

/// The file that marks a project or package root.
pub const MANIFEST: &str = "botwork.toml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    package: Option<RawPackage>,
    #[serde(default)]
    dependencies: BTreeMap<String, RawDependency>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackage {
    name: String,
    version: String,
    botwork: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDependency {
    path: Option<String>,
    git: Option<String>,
    tag: Option<String>,
    rev: Option<String>,
    branch: Option<String>,
    url: Option<String>,
    sha256: Option<String>,
    version: Option<String>,
}

/// A package's identity and the Botwork versions it works with.
#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub version: Version,
    pub botwork: Option<VersionReq>,
}

/// Where a dependency comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A directory, relative to the manifest that names it.
    Path(String),
    Git {
        url: String,
        reference: GitRef,
    },
    Url {
        url: String,
        sha256: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitRef {
    Tag(String),
    Rev(String),
    Branch(String),
}

impl fmt::Display for Source {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => write!(output, "path {path}"),
            Self::Git { url, reference } => match reference {
                GitRef::Tag(tag) => write!(output, "git {url} tag {tag}"),
                GitRef::Rev(rev) => write!(output, "git {url} rev {rev}"),
                GitRef::Branch(branch) => write!(output, "git {url} branch {branch}"),
            },
            Self::Url { url, sha256 } => write!(output, "url {url} sha256 {sha256}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Dependency {
    pub source: Source,
    /// The versions of the package the dependent accepts.
    pub version: Option<VersionReq>,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    /// None for a project that is not itself a package.
    pub package: Option<Package>,
    pub dependencies: BTreeMap<String, Dependency>,
}

/// Whether `name` is a valid package name: lowercase ASCII letters, digits,
/// and single hyphens between them, starting with a letter, at most 64 bytes.
pub fn valid_name(name: &str) -> bool {
    name.len() <= 64
        && name.starts_with(|character: char| character.is_ascii_lowercase())
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Manifest {
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_MANIFEST_BYTES {
            return Err(format!("larger than {MAX_MANIFEST_BYTES} bytes"));
        }
        let raw: Raw = toml::from_str(text).map_err(|error| error.message().to_owned())?;
        let package = raw
            .package
            .map(|package| {
                if !valid_name(&package.name) {
                    return Err(format!(
                        "`{}` is not a package name: use lowercase letters, digits, and hyphens, starting with a letter",
                        package.name
                    ));
                }
                let version = Version::parse(&package.version).map_err(|error| {
                    format!("package version `{}`: {error}", package.version)
                })?;
                let botwork = package
                    .botwork
                    .map(|requirement| {
                        VersionReq::parse(&requirement).map_err(|error| {
                            format!("Botwork version requirement `{requirement}`: {error}")
                        })
                    })
                    .transpose()?;
                Ok(Package {
                    name: package.name,
                    version,
                    botwork,
                })
            })
            .transpose()?;
        if raw.dependencies.len() > MAX_DEPENDENCIES {
            return Err(format!("more than {MAX_DEPENDENCIES} dependencies"));
        }
        let mut dependencies = BTreeMap::new();
        for (name, raw) in raw.dependencies {
            if !valid_name(&name) {
                return Err(format!("dependency `{name}` is not a package name"));
            }
            let dependency =
                dependency(raw).map_err(|error| format!("dependency `{name}`: {error}"))?;
            dependencies.insert(name, dependency);
        }
        Ok(Self {
            package,
            dependencies,
        })
    }
}

fn dependency(raw: RawDependency) -> Result<Dependency, String> {
    let version = raw
        .version
        .as_deref()
        .map(|requirement| {
            VersionReq::parse(requirement)
                .map_err(|error| format!("version requirement `{requirement}`: {error}"))
        })
        .transpose()?;
    let git_keys = raw.tag.is_some() || raw.rev.is_some() || raw.branch.is_some();
    let source = match (raw.path, raw.git, raw.url) {
        (Some(path), None, None) if !git_keys && raw.sha256.is_none() => {
            if path.is_empty() {
                return Err("an empty path".into());
            }
            Source::Path(path)
        }
        (None, Some(url), None) if raw.sha256.is_none() => {
            remote(&url, &["https", "ssh", "file"])?;
            let reference = match (raw.tag, raw.rev, raw.branch) {
                (Some(tag), None, None) => GitRef::Tag(reference(tag)?),
                (None, Some(rev), None) if hex(&rev, 40) => GitRef::Rev(rev),
                (None, Some(rev), None) => {
                    return Err(format!("rev `{rev}` is not a full 40-digit lowercase commit hash"))
                }
                (None, None, Some(branch)) => GitRef::Branch(reference(branch)?),
                _ => return Err("a git source needs exactly one of `tag`, `rev`, or `branch`".into()),
            };
            Source::Git { url, reference }
        }
        (None, None, Some(url)) if !git_keys => {
            remote(&url, &["https", "http"])?;
            let sha256 = raw
                .sha256
                .ok_or("a url source needs `sha256`, the archive's SHA-256 in lowercase hex")?;
            if !hex(&sha256, 64) {
                return Err(format!("sha256 `{sha256}` is not 64 lowercase hex digits"));
            }
            Source::Url { url, sha256 }
        }
        _ => {
            return Err(
                "name exactly one source: `path`, `git` (with `tag`, `rev`, or `branch`), or `url` (with `sha256`)"
                    .into(),
            )
        }
    };
    Ok(Dependency { source, version })
}

/// A git reference name, which must not look like an option.
fn reference(name: String) -> Result<String, String> {
    if name.is_empty()
        || name.starts_with('-')
        || name.len() > 255
        || name
            .bytes()
            .any(|byte| byte <= b' ' || byte == 0x7f || b"~^:?*[\\".contains(&byte))
    {
        return Err(format!("`{name}` is not a git reference name"));
    }
    Ok(name)
}

/// A remote URL with one of `schemes`, which must not look like an option.
fn remote(url: &str, schemes: &[&str]) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|error| format!("`{url}`: {error}"))?;
    if !schemes.contains(&parsed.scheme()) {
        return Err(format!(
            "`{url}` uses `{}`; use {}",
            parsed.scheme(),
            schemes
                .iter()
                .map(|scheme| format!("`{scheme}`"))
                .collect::<Vec<_>>()
                .join(" or ")
        ));
    }
    if url.len() > MAX_URL_BYTES || url.bytes().any(|byte| byte <= b' ' || byte == 0x7f) {
        return Err(format!("`{url}` is not a valid URL"));
    }
    // The archive's hash pins its content either way; plain HTTP is for local
    // mirrors and tests alone.
    let loopback = match parsed.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if parsed.scheme() == "http" && !loopback {
        return Err(format!(
            "`{url}` uses `http`; use `https`, or `http` only on this machine"
        ));
    }
    Ok(())
}
