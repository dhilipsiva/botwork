//! Resolving a project's dependencies: every package once, from one source,
//! in a version its dependents and its Botwork requirement accept, fetched
//! into the cache, and recorded in `botwork.lock`.
use super::*;

#[derive(Clone, Debug, Default)]
pub struct FetchOptions {
    /// Use only the lockfile and the cache; fetch nothing.
    pub offline: bool,
    /// Fail rather than change the lockfile.
    pub locked: bool,
    /// The cache directory; the platform's by default.
    pub cache: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct FetchReport {
    pub lock: Lock,
    /// Whether `botwork.lock` was written.
    pub written: bool,
}

/// A dependency waiting to be resolved: who names it, and from where a path
/// source is relative to.
struct Wanted {
    name: String,
    dependency: Dependency,
    dependent: String,
    base: PathBuf,
    /// Whether the dependent was fetched; such packages cannot name paths.
    remote: bool,
}

struct Resolved {
    source: Source,
    dependent: String,
    version: Version,
}

/// Resolve the project at `root` (the directory holding `botwork.toml`),
/// fetch what the cache lacks, and write `botwork.lock` if it changed.
pub fn fetch(root: &Path, options: &FetchOptions) -> Result<FetchReport, String> {
    let manifest = Manifest::read(&root.join(MANIFEST))?;
    let current = botwork_version();
    if let Some(package) = &manifest.package {
        compatible(package, &current)?;
    }
    let lock_path = root.join(LOCKFILE);
    let previous = if lock_path.is_file() {
        Some(Lock::read(&lock_path)?)
    } else {
        None
    };
    if options.locked && previous.is_none() {
        return Err("--locked needs a botwork.lock; run `botwork --fetch` without it first".into());
    }
    let cache = match &options.cache {
        Some(cache) => cache.clone(),
        None => cache::root()?,
    };
    let mut queue: VecDeque<Wanted> = manifest
        .dependencies
        .iter()
        .map(|(name, dependency)| Wanted {
            name: name.clone(),
            dependency: dependency.clone(),
            dependent: MANIFEST.to_owned(),
            base: root.to_owned(),
            remote: false,
        })
        .collect();
    let mut resolved: BTreeMap<String, Resolved> = BTreeMap::new();
    let mut packages = Vec::new();
    while let Some(wanted) = queue.pop_front() {
        let Wanted {
            name,
            dependency,
            dependent,
            base,
            remote,
        } = wanted;
        let source = match &dependency.source {
            Source::Path(path) => {
                if remote {
                    return Err(format!(
                        "{dependent} names `{name}` by path, but a fetched package can only name git and url sources"
                    ));
                }
                Source::Path(relative(root, &base.join(path))?)
            }
            other => other.clone(),
        };
        if let Some(earlier) = resolved.get(&name) {
            if earlier.source != source {
                return Err(format!(
                    "dependency conflict: {} needs `{name}` from {}, but {dependent} needs it from {source}",
                    earlier.dependent, earlier.source
                ));
            }
            accepts(
                &name,
                &dependent,
                dependency.version.as_ref(),
                &earlier.version,
            )?;
            continue;
        }
        if resolved.len() >= MAX_PACKAGES {
            return Err(format!("more than {MAX_PACKAGES} packages"));
        }
        let text = source.to_string();
        let pin = previous
            .as_ref()
            .and_then(|lock| lock.get(&name))
            .filter(|locked| locked.source == text);
        if options.locked && pin.is_none() {
            return Err(format!(
                "botwork.lock does not pin `{name}` from {source}; run `botwork --fetch` without --locked"
            ));
        }
        let (directory, path, commit, tree) = match &source {
            Source::Path(path) => {
                let directory = root.join(path);
                if !directory.join(MANIFEST).is_file() {
                    return Err(format!(
                        "{dependent} names `{name}` at {}, which has no {MANIFEST}",
                        directory.display()
                    ));
                }
                (directory, Some(path.clone()), None, None)
            }
            Source::Git { url, reference } => {
                let pin =
                    pin.and_then(|locked| locked.commit.as_deref().zip(locked.tree.as_deref()));
                let fetched = source::fetch_git(&cache, url, reference, pin, options.offline)
                    .map_err(|error| format!("`{name}`: {error}"))?;
                (fetched.root, None, fetched.commit, Some(fetched.tree))
            }
            Source::Url { url, sha256 } => {
                let pin = pin.and_then(|locked| locked.tree.as_deref());
                let fetched = source::fetch_url(&cache, url, sha256, pin, options.offline)
                    .map_err(|error| format!("`{name}`: {error}"))?;
                (fetched.root, None, None, Some(fetched.tree))
            }
        };
        let package_manifest = Manifest::read(&directory.join(MANIFEST))?;
        let package = package_manifest
            .package
            .as_ref()
            .ok_or_else(|| format!("`{name}` has a {MANIFEST} without a [package] section"))?;
        if package.name != name {
            return Err(format!(
                "{dependent} names `{name}`, but its {MANIFEST} names the package `{}`",
                package.name
            ));
        }
        compatible(package, &current)?;
        accepts(
            &name,
            &dependent,
            dependency.version.as_ref(),
            &package.version,
        )?;
        for (child, child_dependency) in &package_manifest.dependencies {
            queue.push_back(Wanted {
                name: child.clone(),
                dependency: child_dependency.clone(),
                dependent: format!("`{name}`"),
                base: directory.clone(),
                remote: !matches!(source, Source::Path(_)) || remote,
            });
        }
        packages.push(Locked {
            name: name.clone(),
            version: package.version.to_string(),
            source: text,
            path,
            commit,
            tree,
            dependencies: package_manifest.dependencies.keys().cloned().collect(),
        });
        resolved.insert(
            name,
            Resolved {
                source,
                dependent,
                version: package.version.clone(),
            },
        );
    }
    let lock = Lock {
        version: Lock::VERSION,
        packages,
    };
    let text = lock.render();
    let unchanged = std::fs::read_to_string(&lock_path).is_ok_and(|old| old == text);
    if unchanged {
        return Ok(FetchReport {
            lock,
            written: false,
        });
    }
    if options.locked {
        return Err("botwork.lock is out of date; run `botwork --fetch` without --locked".into());
    }
    std::fs::write(&lock_path, text)
        .map_err(|error| format!("{}: {error}", lock_path.display()))?;
    Ok(FetchReport {
        lock,
        written: true,
    })
}

/// Whether `package` works with this Botwork.
fn compatible(package: &Package, current: &Version) -> Result<(), String> {
    match &package.botwork {
        Some(requirement) if !requirement.matches(current) => Err(format!(
            "version incompatibility: `{}` {} needs Botwork {requirement}, but this is Botwork {current}",
            package.name, package.version
        )),
        _ => Ok(()),
    }
}

/// Whether `dependent` accepts `version` of `name`.
fn accepts(
    name: &str,
    dependent: &str,
    requirement: Option<&VersionReq>,
    version: &Version,
) -> Result<(), String> {
    match requirement {
        Some(requirement) if !requirement.matches(version) => Err(format!(
            "version incompatibility: {dependent} needs `{name}` {requirement}, but it is {version}"
        )),
        _ => Ok(()),
    }
}

/// `path` relative to the project `root`, with `/` separators and `.` and
/// `..` resolved lexically, so the lockfile reads the same everywhere.
fn relative(root: &Path, path: &Path) -> Result<String, String> {
    let absolute = |path: &Path| {
        let mut parts: Vec<String> = Vec::new();
        let mut prefix = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => prefix.push(component.as_os_str()),
                Component::CurDir => {}
                Component::ParentDir => {
                    parts.pop();
                }
                Component::Normal(name) => parts.push(
                    name.to_str()
                        .ok_or_else(|| format!("{}: the path is not UTF-8", path.display()))?
                        .to_owned(),
                ),
            }
        }
        Ok::<_, String>((prefix, parts))
    };
    let (root_prefix, root_parts) = absolute(root)?;
    let (prefix, parts) = absolute(path)?;
    if root_prefix != prefix {
        return Err(format!(
            "{} is on another drive than the project",
            path.display()
        ));
    }
    let common = root_parts
        .iter()
        .zip(&parts)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative: Vec<&str> = vec![".."; root_parts.len() - common];
    relative.extend(parts[common..].iter().map(String::as_str));
    Ok(if relative.is_empty() {
        ".".to_owned()
    } else {
        relative.join("/")
    })
}
