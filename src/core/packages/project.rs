//! Where `@name/path` imports lead: a project's lockfile maps each package it
//! resolved to a directory, a path source's or a cache entry, and every file
//! imports only the packages its own manifest names.
use super::*;

/// A project root with its manifest, its lockfile, and the cache that holds
/// its fetched packages.
#[derive(Clone, Debug)]
pub struct Project {
    root: PathBuf,
    manifest: Manifest,
    lock: Lock,
    /// Each locked package's directory, canonical, by name.
    directories: BTreeMap<String, PathBuf>,
    /// Each URL file's place in the cache, canonical once fetched, by URL.
    files: BTreeMap<String, PathBuf>,
}

/// The package a path names, if it names one: `@name/rest` gives the name
/// and the relative rest, which must stay inside the package.
pub fn package_path(path: &str) -> Option<Result<(&str, PathBuf), String>> {
    let rest = path.strip_prefix('@')?;
    let Some((name, file)) = rest.split_once('/') else {
        return Some(Err(format!(
            "`{path}` names a package but no file in it: write `@name/file`"
        )));
    };
    if !valid_name(name) {
        return Some(Err(format!("`{name}` is not a package name")));
    }
    let mut relative = PathBuf::new();
    for part in file.split('/') {
        match part {
            "" | "." => {}
            ".." => return Some(Err(format!("`{path}` leaves the package `{name}`"))),
            part if part.contains(['\\', ':']) => {
                return Some(Err(format!(
                    "`{path}` is not a path inside the package `{name}`"
                )))
            }
            part => relative.push(part),
        }
    }
    if relative.as_os_str().is_empty() {
        return Some(Err(format!("`{path}` names a package but no file in it")));
    }
    Some(Ok((name, relative)))
}

impl Project {
    /// The nearest directory at or above `start` that holds a `botwork.toml`.
    pub fn find(start: &Path) -> Option<PathBuf> {
        start
            .ancestors()
            .find(|directory| directory.join(MANIFEST).is_file())
            .map(Path::to_owned)
    }

    /// The project at `root`, with its lockfile and the cache at `cache` (the
    /// platform's when None). Packages missing from the cache are reported
    /// when imported, not here.
    pub fn load(root: &Path, cache: Option<&Path>) -> Result<Self, String> {
        let root = crate::core::paths::canonicalize(root)
            .map_err(|error| format!("{}: {error}", root.display()))?;
        let manifest = Manifest::read(&root.join(MANIFEST))?;
        let lock_path = root.join(LOCKFILE);
        let lock = if lock_path.is_file() {
            Lock::read(&lock_path)?
        } else {
            Lock {
                version: Lock::VERSION,
                packages: Vec::new(),
                files: Vec::new(),
            }
        };
        let cache = match cache {
            Some(cache) => cache.to_owned(),
            None if !manifest.files.is_empty()
                || lock.packages.iter().any(|package| package.tree.is_some()) =>
            {
                cache::root()?
            }
            None => PathBuf::new(),
        };
        // The manifest's hash pins each file; the lockfile only records it.
        let mut files = BTreeMap::new();
        for (url, sha256) in &manifest.files {
            let file = cache::file(&cache, sha256, &super::manifest::file_name(url)?);
            let file = crate::core::paths::canonicalize(&file).unwrap_or(file);
            files.insert(url.clone(), file);
        }
        let mut directories = BTreeMap::new();
        for package in &lock.packages {
            let directory = match (&package.path, &package.tree) {
                (Some(path), _) => root.join(path),
                (None, Some(tree)) => cache::package(&cache, tree)?,
                (None, None) => {
                    return Err(format!(
                        "botwork.lock: `{}` has neither a path nor a tree",
                        package.name
                    ))
                }
            };
            // A package not yet fetched keeps its expected place.
            let directory = crate::core::paths::canonicalize(&directory).unwrap_or(directory);
            directories.insert(package.name.clone(), directory);
        }
        Ok(Self {
            root,
            manifest,
            lock,
            directories,
            files,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The cached file a URL import names: the URL must be in the project's
    /// `[files]`, and fetched.
    pub fn url_file(&self, url: &str) -> Result<PathBuf, String> {
        let file = self.files.get(url).ok_or_else(|| {
            format!(
                "`{url}` is not in the [files] of {}; name it there with its SHA-256 and run `botwork --fetch`",
                self.root.join(MANIFEST).display()
            )
        })?;
        if !file.is_file() {
            return Err(format!(
                "`{url}` is not in the cache; run `botwork --fetch`"
            ));
        }
        Ok(file.clone())
    }

    /// The URL a cached file was imported by, to name its module.
    pub fn url_of(&self, file: &Path) -> Option<&str> {
        self.files
            .iter()
            .find(|(_, cached)| cached.as_path() == file)
            .map(|(url, _)| url.as_str())
    }

    /// The file that the import `name`/`file` from `importer` names, checked
    /// against the lockfile: `name` must be a dependency of the package that
    /// holds `importer` (or of the project itself), resolved, and present.
    pub fn resolve(&self, importer: &Path, name: &str, file: &Path) -> Result<PathBuf, String> {
        // The innermost package directory holding the importer; packages fetched
        // into the cache never nest, but path packages may.
        let holder = self
            .directories
            .iter()
            .filter(|(_, directory)| importer.starts_with(directory))
            .max_by_key(|(_, directory)| directory.components().count());
        let (dependent, dependencies): (String, Vec<&str>) = match holder {
            Some((package, _)) => (
                format!("the package `{package}`"),
                self.lock
                    .get(package)
                    .map(|locked| locked.dependencies.iter().map(String::as_str).collect())
                    .unwrap_or_default(),
            ),
            None => (
                format!("{}", self.root.join(MANIFEST).display()),
                self.manifest
                    .dependencies
                    .keys()
                    .map(String::as_str)
                    .collect(),
            ),
        };
        if !dependencies.contains(&name) {
            return Err(format!(
                "`@{name}` is not a dependency of {dependent}; name it in its {MANIFEST}"
            ));
        }
        let locked = self
            .lock
            .get(name)
            .ok_or_else(|| format!("botwork.lock does not list `{name}`; run `botwork --fetch`"))?;
        if holder.is_none() {
            let wanted = self.manifest.dependencies[name].source.to_string();
            if !matches!(self.manifest.dependencies[name].source, Source::Path(_))
                && wanted != locked.source
            {
                return Err(format!(
                    "botwork.lock pins `{name}` from {}, but {MANIFEST} names {wanted}; run `botwork --fetch`",
                    locked.source
                ));
            }
        }
        let directory = &self.directories[name];
        if !directory.is_dir() {
            return Err(format!(
                "`{name}` is not in the package cache ({}); run `botwork --fetch`",
                directory.display()
            ));
        }
        Ok(directory.join(file))
    }

    /// The canonical directory of the locked package `name`.
    pub fn directory(&self, name: &str) -> Option<&Path> {
        self.directories.get(name).map(PathBuf::as_path)
    }
}

/// When a project's manifest and lockfile last changed, to reload it.
type Stamp = [Option<(std::time::SystemTime, u64)>; 2];

fn stamp(root: &Path) -> Stamp {
    [MANIFEST, LOCKFILE].map(|name| {
        std::fs::metadata(root.join(name))
            .ok()
            .and_then(|metadata| Some((metadata.modified().ok()?, metadata.len())))
    })
}

/// Projects loaded for editor analysis, most recent first.
static PROJECTS: std::sync::Mutex<Vec<(Stamp, std::sync::Arc<Project>)>> =
    std::sync::Mutex::new(Vec::new());
const PROJECTS_KEPT: usize = 8;

/// The file that the import `path` names from a file in `directory`, when it
/// names a package (`@name/file`): through the project whose root or package
/// directories hold `directory`, loaded once and reloaded when its manifest
/// or lockfile changes. For editor analysis, which follows imports into
/// packages without a run.
pub fn import_file(directory: &Path, path: &str) -> Option<Result<PathBuf, String>> {
    let parsed = package_path(path)?;
    Some(parsed.and_then(|(name, file)| {
        let directory = crate::core::paths::canonicalize(directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?;
        let mut projects = PROJECTS.lock().unwrap_or_else(|error| error.into_inner());
        projects.retain(|(kept, project)| *kept == stamp(project.root()));
        let held = projects.iter().position(|(_, project)| {
            directory.starts_with(project.root())
                || project
                    .directories
                    .values()
                    .any(|package| directory.starts_with(package))
        });
        let project = match held {
            Some(index) => {
                let entry = projects.remove(index);
                projects.insert(0, entry);
                std::sync::Arc::clone(&projects[0].1)
            }
            None => {
                let root = Project::find(&directory).ok_or_else(|| {
                    format!(
                        "package and URL imports need a {MANIFEST} at or above {}",
                        directory.display()
                    )
                })?;
                let project = std::sync::Arc::new(Project::load(&root, None)?);
                projects.insert(0, (stamp(project.root()), std::sync::Arc::clone(&project)));
                projects.truncate(PROJECTS_KEPT);
                project
            }
        };
        project.resolve(&directory, name, &file)
    }))
}
