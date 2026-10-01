//! Fetching a git or url source into the cache, or finding a path source.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    process::{Command, Stdio},
};

/// A git or url package in the cache.
pub struct Fetched {
    pub root: PathBuf,
    pub commit: Option<String>,
    pub tree: String,
}

/// Run `git` with `arguments` and return its standard output. Only the
/// https, ssh, and file protocols are allowed, nothing prompts, and the
/// arguments never pass through a shell.
fn git<S: AsRef<OsStr>>(arguments: &[S]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args([
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
            "-c",
            "protocol.ssh.allow=always",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => "git sources need `git` on PATH".to_owned(),
            _ => format!("running git: {error}"),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let start = stderr
            .char_indices()
            .map(|(index, _)| index)
            .find(|index| stderr.len() - index <= 2048)
            .unwrap_or(stderr.len());
        return Err(format!("git: {}", &stderr[start..]));
    }
    Ok(output.stdout)
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

/// The bare repository that caches the git remote `url`.
fn repository(cache: &Path, url: &str) -> Result<PathBuf, String> {
    let digest = Sha256::digest(url.as_bytes());
    let name: String = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let repository = cache.join("git").join(format!("{name}.git"));
    if !repository.join("HEAD").is_file() {
        std::fs::create_dir_all(&repository)
            .map_err(|error| format!("{}: {error}", repository.display()))?;
        git(&[
            OsStr::new("init"),
            OsStr::new("--bare"),
            OsStr::new("--quiet"),
            repository.as_os_str(),
        ])?;
    }
    Ok(repository)
}

/// The commit `reference` names in `repository`, if it has it.
fn commit(repository: &Path, reference: &str) -> Option<String> {
    git(&[
        OsStr::new("-C"),
        repository.as_os_str(),
        OsStr::new("rev-parse"),
        OsStr::new("--verify"),
        OsStr::new("--quiet"),
        OsStr::new("--end-of-options"),
        OsStr::new(&format!("{reference}^{{commit}}")),
    ])
    .ok()
    .map(text)
}

/// Fetch a git source: the commit its reference names, or the commit `pin`
/// (from the lockfile) when given; then its files, checked against `tree`.
pub fn fetch_git(
    cache: &Path,
    url: &str,
    reference: &GitRef,
    pin: Option<(&str, &str)>,
    offline: bool,
) -> Result<Fetched, String> {
    if let Some((commit, tree)) = pin {
        let root = cache::package(cache, tree)?;
        if root.is_dir() {
            return Ok(Fetched {
                root,
                commit: Some(commit.to_owned()),
                tree: tree.to_owned(),
            });
        }
    }
    let repository = repository(cache, url)?;
    let wanted = match (pin, reference) {
        (Some((commit, _)), _) => commit.to_owned(),
        (None, GitRef::Rev(rev)) => rev.clone(),
        (None, GitRef::Tag(tag)) => format!("refs/tags/{tag}"),
        (None, GitRef::Branch(branch)) => format!("refs/heads/{branch}"),
    };
    // Branches move, so an unpinned branch is fetched afresh.
    let moving = pin.is_none() && matches!(reference, GitRef::Branch(_));
    if moving || commit(&repository, &wanted).is_none() {
        if offline {
            return Err(format!(
                "{url} is not in the cache, and --offline forbids fetching it"
            ));
        }
        git(&[
            OsStr::new("-C"),
            repository.as_os_str(),
            OsStr::new("fetch"),
            OsStr::new("--quiet"),
            OsStr::new("--prune"),
            OsStr::new("--no-tags"),
            OsStr::new("--"),
            OsStr::new(url),
            OsStr::new("+refs/heads/*:refs/heads/*"),
            OsStr::new("+refs/tags/*:refs/tags/*"),
        ])?;
    }
    let resolved = commit(&repository, &wanted)
        .ok_or_else(|| format!("{url} has no {}", describe(reference)))?;
    let scratch = cache::scratch(cache)?;
    let extracted = scratch.path().join("files");
    // Read the whole archive, within a cap, before extracting it, so git never
    // waits on a pipe nobody reads.
    let mut archive = Command::new("git")
        .args(["-C"])
        .arg(&repository)
        .args(["archive", "--format=tar", "--end-of-options"])
        .arg(&resolved)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("running git: {error}"))?;
    let limit = MAX_TREE_BYTES * 2;
    let mut bytes = Vec::new();
    let read = std::io::Read::read_to_end(
        &mut std::io::Read::take(archive.stdout.take().expect("piped"), limit + 1),
        &mut bytes,
    );
    if read.is_err() || bytes.len() as u64 > limit {
        let _ = archive.kill();
    }
    let status = archive
        .wait()
        .map_err(|error| format!("running git: {error}"))?;
    read.map_err(|error| format!("reading git's archive: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!(
            "{url}: the archive of {resolved} is larger than {limit} bytes"
        ));
    }
    if !status.success() {
        return Err(format!("git archive of {resolved} failed"));
    }
    tree::extract(bytes.as_slice(), &extracted)?;
    let tree = cache::store(cache, &extracted, pin.map(|(_, tree)| tree))?;
    Ok(Fetched {
        root: cache::package(cache, &tree)?,
        commit: Some(resolved),
        tree,
    })
}

fn describe(reference: &GitRef) -> String {
    match reference {
        GitRef::Tag(tag) => format!("tag `{tag}`"),
        GitRef::Rev(rev) => format!("commit `{rev}`"),
        GitRef::Branch(branch) => format!("branch `{branch}`"),
    }
}

/// Fetch a url source: a gzipped tar archive whose SHA-256 must be `sha256`,
/// and whose files must hash to `pin` when the lockfile pins them.
pub fn fetch_url(
    cache: &Path,
    url: &str,
    sha256: &str,
    pin: Option<&str>,
    offline: bool,
) -> Result<Fetched, String> {
    if let Some(tree) = pin {
        let root = cache::package(cache, tree)?;
        if root.is_dir() {
            return Ok(Fetched {
                root,
                commit: None,
                tree: tree.to_owned(),
            });
        }
    }
    if offline {
        return Err(format!(
            "{url} is not in the cache, and --offline forbids fetching it"
        ));
    }
    let archive = download::get(url)?;
    let actual = tree::sha256(&archive);
    if actual != sha256 {
        return Err(format!(
            "integrity check failed: {url} has SHA-256 {actual}, but botwork.toml pins {sha256}"
        ));
    }
    let scratch = cache::scratch(cache)?;
    let extracted = scratch.path().join("files");
    tree::extract(flate2::read::GzDecoder::new(archive.as_slice()), &extracted)
        .map_err(|error| format!("{url}: {error}"))?;
    let files = tree::root(&extracted)?;
    let tree = cache::store(cache, &files, pin)?;
    Ok(Fetched {
        root: cache::package(cache, &tree)?,
        commit: None,
        tree,
    })
}
