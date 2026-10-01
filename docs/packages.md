# Packages

A package is a directory of Botwork modules, and of WebAssembly, Python, or
JavaScript modules, with a `botwork.toml` that names it. A project lists the
packages it uses in its own `botwork.toml`; `botwork --fetch` resolves them,
records exactly what it found in `botwork.lock`, and keeps their files in a
cache, so later runs need no network ([D10](decisions.md#d10-packages-and-registry)).
Scripts import a package's files as `@name/path`.

## A project's manifest

`botwork.toml` sits at the root of a project, usually beside its scripts:

```toml
[dependencies]
helpers = { path = "../helpers" }
http-kit = { git = "https://github.com/acme/http-kit.git", tag = "v1.4.0", version = "^1.4" }
pricing = { url = "https://example.com/pricing-2.0.0.tar.gz", sha256 = "9f2c…" }
```

Each dependency names one source:

| Source | Keys | Pinned by |
| --- | --- | --- |
| A directory | `path`, relative to the manifest | Nothing: its files are read in place |
| A git repository | `git`, and one of `tag`, `rev` (a full commit hash), or `branch` | The commit, and the hash of its files |
| A gzipped tarball | `url`, and `sha256`, the archive's SHA-256 in lowercase hex | The archive's hash, and the hash of its files |

- `version` (optional) is a [semver](https://semver.org) requirement the
  package's own version must meet, such as `"^1.4"`.
- Git URLs use `https`, `ssh`, or `file`; git sources need `git` on `PATH`.
- URLs use `https`. Plain `http` is allowed only for this machine
  (`localhost` or a loopback address), for local mirrors and tests: the
  `sha256` pins the content either way.
- Names are lowercase letters, digits, and single hyphens, starting with a
  letter, at most 64 bytes.

## A package's manifest

A package's `botwork.toml` has a `[package]` section, and may have
dependencies of its own:

```toml
[package]
name = "http-kit"
version = "1.4.0"
botwork = ">=0.2"

[dependencies]
retry = { git = "https://github.com/acme/retry.git", tag = "v2.0.0" }
```

`version` is the package's semver version. `botwork` (optional) is the
Botwork versions it works with; fetching it with another fails. A project may
have a `[package]` section too, but needs none. A package fetched from git or
a URL can depend only on git and URL sources: a `path` would point outside it.

## Fetching

```sh
botwork --fetch            # the project in the current directory
botwork --fetch ../project # or another
```

`--fetch` reads the project's `botwork.toml` and each package's in turn,
fetches what the cache lacks, and writes `botwork.lock`. It prints each
package it locked and whether the lockfile changed:

```text
[fetch] helpers 0.1.0 from path ../helpers
[fetch] http-kit 1.4.0 from git https://github.com/acme/http-kit.git tag v1.4.0
[fetch] botwork.lock written
```

Resolution is strict, and any of these fails the fetch with exit status 1,
leaving the lockfile as it was:

- **One source per package.** Every dependent that names a package must name
  the same source; two tags, two URLs, or a path and a tag are a dependency
  conflict, which names both dependents.
- **Versions.** A package must meet each dependent's `version` requirement,
  and its own `botwork` requirement must accept this Botwork: otherwise the
  fetch reports a version incompatibility.
- **Names.** A dependency's manifest must name the package it is listed as.
- **Integrity.** A URL's archive must hash to its `sha256`. A package that
  `botwork.lock` already pins must hash to the pinned tree when refetched, so
  a changed tag or a tampered archive is caught.

| Option | Effect |
| --- | --- |
| `--offline` | Use only `botwork.lock` and the cache; fetch nothing. |
| `--locked` | Fail rather than change `botwork.lock`, as CI should. |

## Files by URL

A project can also import single files by URL, each pinned by its SHA-256 in
a `[files]` table of its `botwork.toml`:

```toml
[files]
"https://example.com/lib/text.botwork" = "3f1d…"
"https://example.com/tools/statements.wasm" = "8c0a…"
```

```
Import |"https://example.com/lib/text.botwork"| As |text|
```

- `botwork --fetch` downloads each file the cache lacks, checks its SHA-256,
  and lists it in `botwork.lock`; `--offline` uses only the cache.
- A run never downloads: an import of a URL the manifest does not name, or
  that is not fetched yet, is BW6001 saying so.
- A file must end in `.botwork`, `.wasm`, `.py`, `.js`, `.mjs`, or `.cjs`,
  with a plain name, and its URL must use `https` (or `http` on this machine).
- A Botwork module imported by URL is named by its URL in diagnostics and
  checks. It may import other URLs the project names, and packages, but not
  relative paths: there is no directory beside it.
- Only a project's manifest names files; a package's cannot.

## The lockfile

`botwork.lock` lists every package the project resolved, by name, with its
version, its source, the commit a git reference resolved to, and the SHA-256
tree hash of its files, and every file it fetched by URL, with its SHA-256. Commit it with the project: with the lockfile, every
machine runs the same files, and `botwork --fetch --locked` there fetches
exactly them.

A tree hash covers the package's regular files: SHA-256 over one line per
file, in byte order of its `/`-separated path, holding the path, a NUL, and
the file's own SHA-256. Packages hold only regular files and directories;
links and other entries are refused when fetched.

## The cache

Fetched packages live in a directory named by their tree hash, and URL files
in one named by their SHA-256, under
`BOTWORK_CACHE_DIR` if set, else the platform's user cache directory:
`$XDG_CACHE_HOME/botwork` (or `~/.cache/botwork`) on Linux,
`~/Library/Caches/botwork` on macOS, and `%LOCALAPPDATA%\botwork` on Windows.
Entries are written once, after their hash is checked, and then only read; a
run trusts them. Git sources also keep a bare clone there, so a later fetch
only downloads what changed. Deleting the cache is safe: `botwork --fetch`
refills it.

Archives are extracted conservatively: every entry must be a regular file or
directory inside the package, with no absolute path, `..`, or name a
platform reads specially, within 256 MiB and 100,000 entries. A tarball with
a single top-level directory, as release archives have, is unwrapped.

## Importing package files

```
Import |"@http-kit/auth.botwork"| As |auth|
Import |"@helpers/tools.wasm"| As |tools|
```

`@name/path` names `path` inside the package `name`. A Botwork, WebAssembly,
Python, or JavaScript file imports this way as it does by a relative path,
under the same [module rules](language.md#local-modules): namespaces, BW6003
collisions, caching, and cycles.

- **The project.** A run finds the project at its first `@` import, from the
  importing file's directory upward to the nearest `botwork.toml`, and reads
  its `botwork.lock`. Runs never fetch.
- **Dependencies.** A file imports only the packages its own manifest names:
  a project's scripts those of the project's `botwork.toml`, and a package's
  files those of its own. Using a package that only the project names fails.
- **Inside the package.** The path cannot leave the package: `..` is refused,
  and so is a link in a path package that leads out of it.
- **Failures** are BW6001, saying what to do: a missing `botwork.toml`, a
  package the lockfile does not list or the cache lacks (run
  `botwork --fetch`), or a lockfile that pins another source than the
  manifest names.

[`--check`](check.md) and the [language server](lsp.md) follow `@` imports
into packages too, through the same lockfile.

## Trust

A package's Botwork, Python, and JavaScript modules run with the importing
run's full capabilities, as any local module does: fetch packages only from
sources you trust, and review changes to `botwork.lock`. Its
[WebAssembly modules](wasm.md) run sandboxed. The lockfile's hashes make what
runs reproducible; they do not vouch for it. [Extension trust](trust.md) sets
out what each kind of code can reach. A verified package registry is planned
after 1.0.

## Tests

`cargo test --locked --test packages` fetches path, git, and URL packages and
URL files (local git repositories and a server on this machine), runs scripts
that import them, and covers offline runs, integrity failures, conflicts,
version incompatibilities, dependencies a package may not use, paths that
leave a package, `--locked`, every kind of module, modules named by their
URL, and the language server. Unit tests
in `src/core/packages/tests.rs` cover manifests, lockfiles, tree hashes, and
archive extraction. The git tests are skipped without `git` unless
`BOTWORK_REQUIRE_GIT` is set, as it is in CI.
