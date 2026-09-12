//! `oslo.fs` — the filesystem, answering with tables rather than text.
//!
//! `oslo.fs.ls(dir)` gives entries with `name`, `size`, `type` and `mtime`. That is the whole
//! difference from shelling out to `ls -l` and parsing it: no column alignment to get wrong, no
//! locale to change the date format underneath you, no filename with a space in it to split at.
//!
//! Every call here can fail for a reason outside the caller's control — the file is gone, the
//! directory is not readable — so every call answers `nil, message` rather than raising. See
//! [`super::util::failed`].

use super::util::{
    failed, failed_between, failed_path, int, list, ok, opt_text, put, raw, record, text,
};
use oslo_base::value::{Table, Value};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::UNIX_EPOCH;

mod handles;

pub fn build() -> Value {
    let mut it = Table::new();

    reading(&mut it);
    writing(&mut it);
    listing(&mut it);
    metadata(&mut it);
    #[cfg(feature = "watch")]
    super::watch::install(&mut it);

    Value::table(it)
}

fn reading(it: &mut Table) {
    // oslo.fs.read(path) -> contents, or nil + message
    put(it, "read", |_, args| {
        let path = text(&args, 1, "oslo.fs.read")?;
        match fs::read(&path) {
            // **The bytes, exactly.** This went through `String::from_utf8_lossy` until Lua
            // strings could be bytes at oslo`s end too, so reading a PNG answered a PNG with every
            // non-text byte replaced by `U+FFFD` — a read that looked like it had worked. Text is
            // still text; only what was never text is now itself. See `oslo_base::value::Value`.
            Ok(bytes) => ok(Value::bytes(&bytes)),
            Err(e) => failed_path(&path, &e),
        }
    });

    // oslo.fs.lines(path) -> an iterator over the file's lines
    //
    // **The descriptor stays open between calls**, which is what makes this the right way to read
    // a log. It used to read the whole file and answer a table, because there was no `__close` to
    // shut a held descriptor with; there is one now.
    put(it, "lines", |_, args| {
        let path = text(&args, 1, "oslo.fs.lines")?;
        match fs::File::open(&path) {
            Ok(file) => ok(handles::reader(std::io::BufReader::new(file))),
            Err(e) => failed_path(&path, &e),
        }
    });

    put(it, "exists", |_, args| {
        let path = text(&args, 1, "oslo.fs.exists")?;
        // `symlink_metadata`, so a dangling symlink is reported as existing — it does, and
        // `rm` can remove it.
        ok(Value::Bool(fs::symlink_metadata(&path).is_ok()))
    });
}

fn writing(it: &mut Table) {
    // oslo.fs.write(path, contents) -> true, or nil + message
    put(it, "write", |_, args| {
        let path = text(&args, 1, "oslo.fs.write")?;
        let contents = raw(&args, 2, "oslo.fs.write")?;
        match fs::write(&path, contents) {
            Ok(()) => ok(Value::Bool(true)),
            Err(e) => failed_path(&path, &e),
        }
    });

    put(it, "append", |_, args| {
        use std::io::Write;
        let path = text(&args, 1, "oslo.fs.append")?;
        let contents = raw(&args, 2, "oslo.fs.append")?;
        let opened = fs::OpenOptions::new().create(true).append(true).open(&path);
        match opened.and_then(|mut f| f.write_all(&contents)) {
            Ok(()) => ok(Value::Bool(true)),
            Err(e) => failed_path(&path, &e),
        }
    });

    // oslo.fs.mkdir(path) — always `-p`.
    //
    // One function rather than two, because the plain form is almost never what anyone wants: a
    // script that creates a directory wants it to exist afterwards, not to fail because a parent
    // was missing or because it already succeeded once.
    put(it, "mkdir", |_, args| {
        let path = text(&args, 1, "oslo.fs.mkdir")?;
        match fs::create_dir_all(&path) {
            Ok(()) => ok(Value::Bool(true)),
            Err(e) => failed_path(&path, &e),
        }
    });

    // oslo.fs.remove(path, recursive)
    put(it, "remove", |_, args| {
        let path = text(&args, 1, "oslo.fs.remove")?;
        let recursive = args.get(1).is_some_and(Value::truthy);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) => return failed_path(&path, &e),
        };
        // A symlink is removed as a link, never followed — deleting what a link points at when
        // asked to delete the link is how a cleanup script destroys someone's home directory.
        let removed = if meta.is_dir() && recursive {
            fs::remove_dir_all(&path)
        } else if meta.is_dir() {
            fs::remove_dir(&path)
        } else {
            fs::remove_file(&path)
        };
        match removed {
            Ok(()) => ok(Value::Bool(true)),
            Err(e) => failed_path(&path, &e),
        }
    });

    put(it, "rename", |_, args| {
        let from = text(&args, 1, "oslo.fs.rename")?;
        let to = text(&args, 2, "oslo.fs.rename")?;
        match fs::rename(&from, &to) {
            Ok(()) => ok(Value::Bool(true)),
            Err(e) => failed_between(&from, &to, &e),
        }
    });

    put(it, "copy", |_, args| {
        let from = text(&args, 1, "oslo.fs.copy")?;
        let to = text(&args, 2, "oslo.fs.copy")?;
        match fs::copy(&from, &to) {
            Ok(bytes) => ok(Value::int(bytes as i64)),
            Err(e) => failed_between(&from, &to, &e),
        }
    });

    put(it, "symlink", |_, args| {
        let target = text(&args, 1, "oslo.fs.symlink")?;
        let link = text(&args, 2, "oslo.fs.symlink")?;
        match std::os::unix::fs::symlink(&target, &link) {
            Ok(()) => ok(Value::Bool(true)),
            Err(e) => failed_path(&link, &e),
        }
    });

    // oslo.fs.chmod(path, 0755) — the mode is a number, as `chmod` and `stat` both speak it.
    put(it, "chmod", |_, args| {
        let path = text(&args, 1, "oslo.fs.chmod")?;
        let mode = int(&args, 2, "oslo.fs.chmod")?;
        match fs::set_permissions(&path, fs::Permissions::from_mode(mode as u32)) {
            Ok(()) => ok(Value::Bool(true)),
            Err(e) => failed_path(&path, &e),
        }
    });

    // oslo.fs.touch(path) -> true, or nil + message
    //
    // Creates the file when it is not there, and moves its timestamps to now when it is — the two
    // halves of what `touch` means, and neither is one line of the rest of this module. Written by
    // hand it is `oslo.fs.exists` then `oslo.fs.append(path, "")`, which creates the file but
    // leaves the timestamp of an existing one alone: the half people actually wanted.
    put(it, "touch", |_, args| {
        let path = text(&args, 1, "oslo.fs.touch")?;
        let opened = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path);
        match opened.and_then(|file| file.set_modified(std::time::SystemTime::now())) {
            Ok(()) => ok(Value::Bool(true)),
            Err(e) => failed_path(&path, &e),
        }
    });

    // oslo.fs.mktemp(prefix) -> a path that did not exist a moment ago
    //
    // The file is created, not just named. Returning a name for the caller to open is the classic
    // temp-file race: between the answer and the open, anything can take the name.
    put(it, "mktemp", |_, args| {
        let prefix = opt_text(&args, 1, "oslo.fs.mktemp")?.unwrap_or_else(|| "oslo".to_string());
        match unique_temp(&prefix, false) {
            Ok(path) => ok(Value::str(path)),
            Err(e) => failed("mktemp", e),
        }
    });

    // oslo.fs.mktempdir(prefix) -> a handle on a directory that did not exist a moment ago
    //
    // **A handle rather than a path, because a temporary directory is the one thing in `oslo.fs`
    // with a lifetime.** Every other call here acts on a path somebody else owns; this one *makes*
    // something, and until there was `<close>` there was nowhere to say when it should go:
    //
    //     local tmp <close> = oslo.fs.mktempdir()
    //     oslo.fs.write(tmp.path .. "/notes", body)
    //
    // `tostring(tmp)` is the path, so a handle reads as one wherever a message wants it.
    put(it, "mktempdir", |_, args| {
        let prefix = opt_text(&args, 1, "oslo.fs.mktempdir")?.unwrap_or_else(|| "oslo".to_string());
        match unique_temp(&prefix, true) {
            Ok(path) => ok(handles::tempdir(path)),
            Err(e) => failed("mktempdir", e),
        }
    });
}

fn listing(it: &mut Table) {
    // oslo.fs.ls(dir) -> { {name=…, size=…, type=…, mtime=…}, … }
    //
    // Sorted by name, because a directory's order on disk is arbitrary and a script that prints
    // one unsorted looks broken every time the filesystem reorders it.
    put(it, "ls", |_, args| {
        let dir = opt_text(&args, 1, "oslo.fs.ls")?.unwrap_or_else(|| ".".to_string());
        let read = match fs::read_dir(&dir) {
            Ok(read) => read,
            Err(e) => return failed_path(&dir, &e),
        };
        let mut entries: Vec<(String, Value)> = Vec::new();
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let described = match entry.path().symlink_metadata() {
                Ok(meta) => describe(&name, &meta),
                // An entry whose metadata cannot be read is still an entry — reporting the name
                // with unknown details beats dropping it from the listing.
                Err(_) => record(vec![("name", Value::str(&name))]),
            };
            entries.push((name, described));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        ok(list(entries.into_iter().map(|(_, v)| v)))
    });

    // oslo.fs.walk(dir) -> an iterator over every path under `dir`, depth first, directories
    // before their contents
    //
    // **Lazy, because a tree has no size you can promise.** The table this used to answer was the
    // whole of `/nix/store` before the first line of the loop ran; the iterator opens one directory
    // at a time and stops the moment the loop does.
    //
    // Symlinks are not followed. A link back up the tree is what turns "walk this directory" into
    // an infinite loop, and a script written against a tree it does not control will meet one.
    put(it, "walk", |_, args| {
        let root = opt_text(&args, 1, "oslo.fs.walk")?.unwrap_or_else(|| ".".to_string());
        match fs::read_dir(&root) {
            Ok(reading) => ok(handles::walker(reading)),
            Err(e) => failed_path(&root, &e),
        }
    });

    // oslo.fs.glob(pattern) — the shell's own globber, so the two languages agree about what
    // `*.conf` means down to the last edge case.
    // oslo.fs.usage(dir) -> { bytes = …, files = …, dirs = … }, or nil + message
    //
    // **What `du -s` answers, and for the same reason it exists as a command**: adding up a tree is
    // a loop nobody wants to write twice, and writing it over `oslo.fs.walk` in Lua costs a
    // `stat` call *and* a boundary crossing per file. Symlinks are counted as links and never
    // followed, so a link back up the tree cannot make the total infinite.
    //
    // `bytes` is the sum of file sizes, not blocks — the number `ls` shows, not the one `du`
    // does. Those differ by the filesystem's allocation, and the question a script asks is almost
    // always "how much is this", not "how much does it cost my disk".
    //
    // **A subdirectory it cannot read is counted rather than fatal**, so `usage("/etc")` answers
    // for somebody who is not root. `unreadable == 0` means the total is everything; anything
    // else means it is a floor. The root not being there is still `nil` — that is a mistake in
    // the call rather than something met while walking.
    put(it, "usage", |_, args| {
        let root = text(&args, 1, "oslo.fs.usage")?;
        let mut total = Usage::default();
        if let Err(e) = measure(Path::new(&root), &mut total) {
            return failed_path(&root, &e);
        }
        ok(record(vec![
            ("bytes", Value::int(total.bytes as i64)),
            ("files", Value::int(total.files as i64)),
            ("dirs", Value::int(total.dirs as i64)),
            ("unreadable", Value::int(total.unreadable as i64)),
        ]))
    });

    // oslo.fs.disk(path) -> { total, free, available, files, files_free }, or nil + message
    //
    // **What `df` answers, for the filesystem `path` is on** — not for `path` itself. Any path on
    // the mount does, so `oslo.fs.disk(".")` is the usual call and `oslo.fs.disk("/")` is the one
    // people write first.
    //
    // `free` and `available` differ, and the difference is the point: a filesystem reserves a
    // percentage for root, so `available` is what *you* may write and `free` is what exists. `df`
    // shows the first and calls it "Avail"; a script checking whether it can save something wants
    // that one.
    put(it, "disk", |_, args| {
        let path = text(&args, 1, "oslo.fs.disk")?;
        let stats = match nix::sys::statvfs::statvfs(path.as_str()) {
            Ok(stats) => stats,
            Err(e) => return failed_path(&path, &std::io::Error::from(e)),
        };
        // `fragment_size` rather than `block_size`: the block counts are in fragments, and the two
        // are equal on every filesystem anybody uses — which is exactly why using the wrong one is
        // a bug that never shows up until it does.
        let unit = stats.fragment_size() as u64;
        let bytes = |blocks: u64| Value::int(blocks.saturating_mul(unit) as i64);
        ok(record(vec![
            ("total", bytes(stats.blocks())),
            ("free", bytes(stats.blocks_free())),
            ("available", bytes(stats.blocks_available())),
            ("files", Value::int(stats.files() as i64)),
            ("files_free", Value::int(stats.files_free() as i64)),
        ]))
    });

    // The same function as `oslo.glob`, under the namespace that holds the rest of the filesystem.
    put(it, "glob", |_, args| {
        super::glob::glob(&args, "oslo.fs.glob")
    });
    put(it, "match", |_, args| super::glob::matches(&args));
}

fn metadata(it: &mut Table) {
    // oslo.fs.stat(path) -> {name, size, type, mtime, mode, uid, gid}
    put(it, "stat", |_, args| {
        let path = text(&args, 1, "oslo.fs.stat")?;
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                let name = Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                ok(describe(&name, &meta))
            }
            Err(e) => failed_path(&path, &e),
        }
    });

    put(it, "realpath", |_, args| {
        let path = text(&args, 1, "oslo.fs.realpath")?;
        match fs::canonicalize(&path) {
            Ok(resolved) => ok(Value::str(resolved.to_string_lossy())),
            Err(e) => failed_path(&path, &e),
        }
    });

    put(it, "readlink", |_, args| {
        let path = text(&args, 1, "oslo.fs.readlink")?;
        match fs::read_link(&path) {
            Ok(target) => ok(Value::str(target.to_string_lossy())),
            Err(e) => failed_path(&path, &e),
        }
    });

    put(it, "cwd", |_, _| match std::env::current_dir() {
        Ok(dir) => ok(Value::str(dir.to_string_lossy())),
        Err(e) => failed("cwd", e),
    });

    // oslo.fs.find_up(name, from) -> the nearest `name` at or above `from`, or nil
    //
    // **`nil` plainly, not `nil, message`.** Every other call here answers a failure it could not
    // control; not finding the file is the question being answered, and a caller writing
    // `if oslo.fs.find_up(".env.lua") then` should not have to know the difference.
    //
    // `from` defaults to the working directory, because the caller that wants this most is a
    // directory predicate and the directory it is asked about is already where the shell is.
    put(it, "find_up", |_, args| {
        let name = text(&args, 1, "oslo.fs.find_up")?;
        let from = match opt_text(&args, 2, "oslo.fs.find_up")? {
            Some(given) => std::path::PathBuf::from(given),
            None => std::env::current_dir().unwrap_or_default(),
        };
        Ok(vec![match from
            .ancestors()
            .map(|ancestor| ancestor.join(&name))
            .find(|candidate| candidate.exists())
        {
            Some(found) => Value::str(found.to_string_lossy()),
            None => Value::Nil,
        }])
    });
}

/// One entry, as the table every listing hands back.
fn describe(name: &str, meta: &fs::Metadata) -> Value {
    let kind = if meta.is_dir() {
        "directory"
    } else if meta.is_symlink() {
        "symlink"
    } else if meta.is_file() {
        "file"
    } else {
        // A socket, fifo or device node. Named rather than mislabelled as a file, because a
        // script that opens one expecting a file will hang rather than fail.
        "other"
    };
    record(vec![
        ("name", Value::str(name)),
        ("size", Value::int(meta.len() as i64)),
        ("type", Value::str(kind)),
        ("mtime", Value::int(seconds(meta))),
        // **Whole nanoseconds, because whole seconds decides builds wrongly.** A recipe whose input
        // is edited and whose output is written inside the same second compares equal at second
        // resolution, so `oslo make` reported "up to date" for a build that had not happened —
        // measured, and the reason this field exists. `mtime` stays seconds because that is what a
        // person prints; anything *comparing* two files wants this one.
        //
        // Safe as one number: luna keeps the integer subtype, so 1787296745123456789 and the
        // nanosecond after it compare exactly rather than colliding the way a double would.
        ("mtime_ns", Value::int(nanos(meta))),
        // Permission bits only: the type bits are already in `type`, and `0o100644` printed as a
        // mode is what makes people think `chmod` needs six digits.
        (
            "mode",
            Value::int((meta.permissions().mode() & 0o7777) as i64),
        ),
        ("uid", Value::int(meta.uid() as i64)),
        ("gid", Value::int(meta.gid() as i64)),
    ])
}

/// Modification time as a Unix timestamp, which is what a script can compare and format.
fn seconds(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The same instant, to the nanosecond — what a comparison of two files has to use.
///
/// `as_nanos` is a `u128` and the cast narrows it; the value is nanoseconds since 1970, so it
/// overflows `i64` in the year 2262 and reads 0 for a file the clock cannot place. Both are the same
/// answer `seconds` gives, at a different scale.
fn nanos(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_nanos()).ok())
        .unwrap_or(0)
}

/// What a tree adds up to. See `oslo.fs.usage`.
#[derive(Default)]
struct Usage {
    bytes: u64,
    files: usize,
    dirs: usize,
    /// How many directories could not be read, and so are not in the total.
    unreadable: usize,
}

/// Add `root` and everything under it into `total`.
///
/// **An explicit stack rather than recursion**, so a deep tree cannot take the shell down with a
/// stack overflow — a directory nobody controls is exactly where one would come from.
///
/// `symlink_metadata`, so a link is counted as the link it is and never followed. Following one is
/// how a walk of a tree containing a link to its own parent never finishes.
///
/// # A directory it cannot read is counted, not fatal
///
/// The first version stopped on the first `EACCES`, which made `usage("/etc")` answer `nil` for
/// anybody who is not root — one unreadable subdirectory losing the whole total. `du` warns and
/// carries on, and that is the right shape; the only thing wrong with it is that a caller cannot
/// tell a complete answer from a partial one afterwards. So the skipped directories are *counted*:
/// `unreadable == 0` means the total is everything, and anything else means it is a floor.
///
/// The root itself is different, and stays fatal — see `oslo.fs.usage`. "That directory is not
/// there" is a mistake in the call, not a thing found while walking.
fn measure(root: &Path, total: &mut Usage) -> std::io::Result<()> {
    let mut pending = vec![root.to_path_buf()];
    // Read once here rather than inside the loop, so the root's own failure reaches the caller.
    let mut first = Some(fs::read_dir(root)?);
    while let Some(dir) = pending.pop() {
        let Some(listing) = first.take().or_else(|| fs::read_dir(&dir).ok()) else {
            total.unreadable += 1;
            continue;
        };
        for entry in listing.flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            if meta.is_dir() {
                total.dirs += 1;
                pending.push(entry.path());
            } else {
                total.files += 1;
                total.bytes += meta.len();
            }
        }
    }
    Ok(())
}

/// Create a file or directory under `$TMPDIR` whose name nothing else holds.
///
/// The name mixes the process id with a counter, and creation uses `create_new`, so the loop
/// retries rather than trusting the name to be free — two shells started in the same second have
/// different pids, and one shell's two calls have different counters.
fn unique_temp(prefix: &str, directory: bool) -> std::io::Result<String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    let pid = std::process::id();
    for _ in 0..1000 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = Path::new(&base).join(format!("{prefix}.{pid}.{n}"));
        let created = if directory {
            fs::create_dir(&path)
        } else {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map(|_| ())
        };
        match created {
            Ok(()) => return Ok(path.to_string_lossy().into_owned()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other(
        "could not find an unused name in 1000 tries",
    ))
}
