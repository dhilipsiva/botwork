# Operating-system statements

The default Engine and CLI provide 32 operating-system statements within the
100-signature fixed catalogue. Paths and text are Strings. Binary files use
Arrays of Int bytes. Arguments retain ordinary left-to-right evaluation, strict
kind checks, source/call diagnostics, and assignment preservation on failure.

## Files and directories

| Signature | Result and behavior |
| --- | --- |
| `Path Exists \|path\|` | Bool; tests the final entry without following its symlink, including dangling links |
| `Path Kind \|path\|` | `file`, `directory`, `symlink`, `other`, or `missing` |
| `File Exists \|path\|` | Bool; follows symlinks and tests for a regular file |
| `Directory Exists \|path\|` | Bool; follows symlinks and tests for a directory |
| `File Size \|path\|` | A regular file's byte length as an exact decimal String |
| `Read File \|path\|` | Strict UTF-8 String; preserves line endings, BOM, NUL, and all content bytes |
| `Read Binary File \|path\|` | Array of Int values from 0 through 255 |
| `Write File \|path\| Text \|text\|` | Create or truncate a regular file; write exact UTF-8 without an added newline |
| `Append To File \|path\| Text \|text\|` | Create or append exact UTF-8; no newline is added |
| `Write Binary File \|path\| Bytes \|bytes\|` | Create or truncate after validating every element as an Int byte |
| `Create File \|path\| Text \|text\|` | Exclusive creation; any existing destination entry causes failure |
| `Copy File \|source\| To \|destination\|` | Stream a regular source into an exclusively created destination |
| `Move Path \|source\| To \|destination\|` | Native rename; replacement rules depend on the platform/filesystem |
| `Remove File \|path\|` | Remove a file or supported symlink entry; missing paths succeed |
| `Create Directory \|path\|` | Create missing parents; an existing directory succeeds |
| `List Directory \|path\|` | Sorted immediate entry names, including hidden entries; no recursion |
| `Remove Directory \|path\| Recursively \|recursive\|` | Bool false removes an empty directory; true removes a tree; missing paths succeed |
| `Create Temporary Directory In \|directory\| Prefix \|prefix\|` | Create a unique directory and return its absolute path |

All mutations except temporary-directory creation return None. Copy never
overwrites a destination, even when it is the source itself, a hard link to it,
or a dangling symlink. It copies content, without preserving permissions,
timestamps, ownership, or other metadata. A failed copy can leave a partial new
destination. Create File likewise uses atomic exclusive creation, with no
separate existence check. Write/Append follow ordinary path symlinks and operate
on the opened regular file. Write truncates only after validating arguments,
admitting its result, and confirming the open handle is a regular file.

Read, Write, Append, Copy, and File Size require regular files. Opening a special
file can itself block before its handle can be classified. Missing existence
targets return false; permission errors, symlink loops, and non-directory parents
are failures rather than false. File Size returns a String to avoid the language's
Int range and Float rounding. Empty writes still create/truncate as requested.

List Directory sorts exact UTF-8 names by byte order without case folding or
Unicode normalization. Entries are names, not full paths; use Join Path to resolve
them. Concurrent changes mean a listing or file read is not an atomic snapshot.
Non-UTF-8 names/content fail instead of being replaced with lossy characters.

Entry-based Path Exists, Path Kind, and Remove Directory remove trailing path
separators and `.` components before addressing the final entry. This prevents a
suffix such as `link/` from making recursive cleanup enter the link target.
Recursive removal delegates to Rust's platform implementation and removes
symlink entries rather than recursively following them. It rejects a resolved
root or terminal `..` component. Intermediate symlink components still follow
ordinary path resolution; this is not a filesystem sandbox. See the platform
qualifications in [Rust's recursive removal contract](https://doc.rust-lang.org/std/fs/fn.remove_dir_all.html).

Temporary prefixes may be empty or contain up to 64 ASCII letters, digits,
dashes, or underscores. Names add 12 random characters using the locked
`tempfile` library. Output size and temporary admission occur before directory
creation. On Unix the requested creation mode is 0700, further restricted by the
process umask; other platforms use their inherited filesystem permissions.
Successful directories persist until explicitly removed. Use Finally as below.
This statement does not register automatic end-of-run cleanup or guarantee cleanup
after process termination or abandoned result delivery.
[Builder behavior](https://docs.rs/tempfile/3.27.0/tempfile/struct.Builder.html)
defines creation and permission handling.

## Paths and environment

| Signature | Result and behavior |
| --- | --- |
| `Working Directory` | The run's directory snapshot |
| `Join Path \|parts\|` | Native join of an Array of Strings; empty Array gives an empty String |
| `Absolute Path \|path\|` | Resolve against the run directory; an empty input returns that directory exactly |
| `Canonical Path \|path\|` | Resolve an existing path and symlinks to a native canonical absolute path |
| `Parent Path \|path\|` | Lexical parent; empty String for a single relative name, None when absent |
| `File Name \|path\|` | Final normal filename component, or None |
| `File Extension \|path\|` | Final extension without its dot; empty String for a trailing dot, None when absent |
| `Path Is Absolute \|path\|` | Bool using native path syntax |
| `Path Components \|path\|` | Array of lexical components, including native roots/prefixes and `..` |
| `Environment Variable Exists \|name\|` | Whether the run's immutable environment snapshot has the variable `name` names |
| `Get Environment Variable \|name\|` | String, or None for a missing key; empty values remain empty Strings |
| `Environment Variables` | Map of every snapshotted environment name and value |
| `Operating System` | Rust target OS name, such as `linux`, `windows`, or `macos` |
| `Path Separator` | The platform's primary path separator |

Relative filesystem paths use the run directory, including calls from imported
modules. Absolute paths and parent components are allowed. No statement changes
the host working directory or environment. Paths have no shell expansion:
`~`, `$HOME`, `*`, and similar text remain literal. All paths reject NUL;
filesystem operations reject empty paths. Lexical helpers permit empty Strings.
None results can be tested against `No Operation` or inspected using `Type Of`.

Join follows native `PathBuf::push` rules: a later absolute component replaces
the prior path; Windows rooted and drive-prefixed paths have their native rules.
Lexical helpers normalize redundant separators and `.` as Rust components do,
but do not collapse `..` across symlinks. Absolute Path preserves the joined
spelling apart from its empty-input rule; it does not require existence. Only
Canonical Path accesses the filesystem. See [Rust path semantics](https://doc.rust-lang.org/std/path/struct.Path.html).

Environment access reads the snapshot configured by RunOptions inheritance and
overlays. Names match exactly, except on Windows, where they match without regard
to ASCII case as the operating system matches them: `PATH` there names the
variable Windows spells `Path`, and an overlay replaces it in the overlay's
spelling. `Environment Variables` keeps each name as the snapshot spells it.
Missing differs from empty.
Names must be nonempty and contain neither `=` nor NUL. Existence testing does
not decode the value; Get and Environment Variables reject non-UTF-8 data.
Environment statements use an Engine run or the CLI's host snapshot. Low-level
hosts can opt in with `Context::with_host_environment`; a standalone Context
without a configured environment reports BW7002. See [embedded run configuration](embedded-runs.md).

## Limits, errors, and platform behavior

BW3003 covers wrong kinds, invalid paths/prefixes/bytes, and non-UTF-8 values.
BW4002 covers filesystem failures with the operation, path, and OS reason.
A path beneath a regular file is such a failure on every platform, not a missing
path: Windows reports it as missing, so there Botwork checks the nearest existing
ancestor.
BW8001 denotes resource exhaustion. Ordinary errors remain catchable; stops and
resource failures follow the existing non-catchable stop policy.

Value and temporary quotas apply to complete file contents and directory or
environment results. File reads admit the opened handle's length before payload
allocation, then admit additional growth before each retained chunk. A shrinking
file releases unused credit. An oversized length hint is rejected even if the
file later shrinks. Binary reads also obey Array entry/node limits; each byte
occupies an Int's four logical payload bytes. Listings admit entries and names
incrementally; environment results are measured before copying snapshot data.

Read/write/copy loops use a 16 KiB buffer and check stop control between I/O calls.
Path construction has a separate 1 MiB logical workspace cap, with conservative
join admission before growth. OS-owned paths/entry names are library workspace;
returned Strings still require normal value admission before copying. Copy
streams without retaining the whole source; it has no separate total disk-byte
quota. Directory creation/removal delegates a complete operation to the standard
library. Neither recursive removal nor an individual syscall can be forcibly
interrupted; cancellation may wait for started work to return.

During asynchronous execution, filesystem statements share the existing pool of
32 native workers. Pure path and environment helpers remain inline. Engine runs
share their immutable directory/environment owners with workers. Low-level
Contexts additionally admit a directory-snapshot copy against snapshot path bytes.
No mutable DSL bindings enter workers. The fixed catalogue is exempt from user
registry retention budgets; its 100 slots count toward snapshot work. Initialization
is idempotent and preserves earlier host overrides.

Completed effects are not rolled back on later I/O errors, cancellation, failed
result delivery, or cleanup failures. Writes can be partial, concurrent appends
can interleave, and successful writes do not imply durable fsync. Files and ordinary
directories use native creation permissions/umask. Filesystem locks, permissions,
mounts, and platform path limits can reject otherwise valid inputs.

On Unix `/` separates components and backslash is an ordinary filename character.
Windows accepts its native drive/UNC rules and canonical paths can include a
verbatim prefix. Removal of open/read-only files and directory symlinks differs
by platform. Move uses `std::fs::rename`: cross-filesystem moves fail, and existing
destination replacement depends on type and filesystem/OS support; there is no
copy-and-delete fallback. See [Rust rename behavior](https://doc.rust-lang.org/std/fs/fn.rename.html).
Validation covers Linux GNU and musl builds; Windows/macOS behavior is documented
from the library contracts rather than claimed as tested.

<!-- botwork-test: operating-system-statements -->
```botwork
|folder| = Create Temporary Directory In |"."| Prefix |"botwork_"|
Try {
    |file| = Join Path |[folder, "message.txt"]|
    Write File |file| Text |"hello"|
    Append To File |file| Text |" world"|
    Copy File |file| To |@{ Join Path |[folder, "copy.txt"]| }|
    Log |@{ Read File |file| }|
    Log |@{ File Size |file| }|
    Log |@{ List Directory |folder| }|
    Log |@{ File Extension |file| }|
} Finally {
    Remove Directory |folder| Recursively |true|
}
```

Output:

```text
hello world
11
["copy.txt", "message.txt"]
txt
```

More statements, with their output in comments. The paths are relative to the
run directory:

<!-- botwork-test: operating-system-variants -->
```botwork
Try {
    Create Directory |"reports/2026"|
    Log |@{ Directory Exists |"reports/2026"| }|                    # true
    Create File |"reports/2026/summary.txt"| Text |"ok"|
    Log |@{ File Exists |"reports/2026/summary.txt"| }|             # true
    Log |@{ Path Kind |"reports"| }|                                 # directory
    Move Path |"reports/2026/summary.txt"| To |"reports/summary.txt"|
    Log |@{ Path Exists |"reports/2026/summary.txt"| }|             # false
    Write Binary File |"reports/data.bin"| Bytes |[0, 127, 255]|
    Log |@{ Read Binary File |"reports/data.bin"| }|                 # [0, 127, 255]
    Remove File |"reports/data.bin"|
    Log |@{ File Name |"reports/summary.txt"| }|                    # summary.txt
    Log |@{ Parent Path |"reports/summary.txt"| }|                  # reports
    Log |@{ Path Components |"reports/summary.txt"| }|              # ["reports", "summary.txt"]
    Log |@{ Path Is Absolute |"reports"| }|                         # false
    Log |@{ Path Is Absolute |@{ Absolute Path |"reports"| }| }|    # true
    Log |@{ Path Is Absolute |@{ Canonical Path |"reports"| }| }|   # true
    Log |@{ Path Is Absolute |@{ Working Directory }| }|            # true
} Finally {
    Remove Directory |"reports"| Recursively |true|
}
|system| = Operating System
Log |@{ Collection Contains |["linux", "macos", "windows"]| Item |system| }|  # true
Log |@{ Path Separator } == "/" or system == "windows"|         # true
Log |@{ Environment Variable Exists |"PATH"| }|                 # true
Log |@{ Length Of |@{ Environment Variables }| } > 0|             # true
```

The same program is [example 32](../examples/32-operating-system.botwork).
