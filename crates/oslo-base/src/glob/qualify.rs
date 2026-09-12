//! Glob qualifiers: `*.log(older 7d, larger 1M, newest 5)`.
//!
//! What zsh writes as `*(.om[1,5])`, spelled in words. One parser and one filter for the three
//! places that take them — the prompt's `pattern(…)`, the `glob` builtin's `--older 7d`, and Lua's
//! `oslo.glob(p, { older = "7d" })` — so a qualifier means the same thing wherever it is written.
//!
//! ```text
//!   file dir link exec socket fifo empty        what an entry is
//!   older 7d   newer 2h                         modification time: s m h d w M y
//!   larger 1M  smaller 10k                      size: k M G T, powers of 1024
//!   mine  user bob  group dev  perm 644         ownership and mode
//!   hidden  nocase  depth 1-3                   how the walk goes
//!   not PATTERN                                 drop what a second glob matches
//!   re 'REGEX'  path re 'REGEX'                 the name, or the whole path, matches a regex
//!   num 1-100                                   the first number in the name is in range
//!   by name|size|time|ext|depth  rev            the order
//!   first N  last N  newest N  oldest N  largest N  smallest N
//!   tracked  untracked  modified  ignored  gitignore
//! ```
//!
//! **Regex is only ever here**, never in a bare word: `foo.*` is a glob and a filename, and a
//! shell that read it as a regex would change what a script means.

use super::ShellPattern;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::time::{Duration, SystemTime};

/// One test an entry must pass.
#[derive(Debug)]
pub enum Test {
    File,
    Dir,
    Link,
    Exec,
    Socket,
    Fifo,
    Empty,
    Older(Duration),
    Newer(Duration),
    Larger(u64),
    Smaller(u64),
    Mine,
    User(u32),
    Group(u32),
    Perm(u32),
    Not(ShellPattern),
    Re { regex: regex::Regex, whole: bool },
    Num(i64, i64),
    Git(Git),
}

/// What `git ls-files` is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Git {
    Tracked,
    Untracked,
    Modified,
    Ignored,
    /// Everything but the ignored: `.gitignore` honoured.
    NotIgnored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    Name,
    Size,
    Time,
    Ext,
    Depth,
}

/// A parsed qualifier list.
#[derive(Debug, Default)]
pub struct Qualifiers {
    pub tests: Vec<Test>,
    pub by: Option<By>,
    pub rev: bool,
    /// Keep this many from the start of the ordered list, or from the end when negative.
    pub take: Option<i64>,
    pub hidden: bool,
    pub nocase: bool,
    pub depth: Option<(usize, usize)>,
}

/// Parse `older 7d, larger 1M`.
pub fn parse(text: &str) -> Result<Qualifiers, String> {
    from_items(&items(text)?)
}

/// The same list, already split into items and words — the `glob` builtin's `--older 7d` and
/// Lua's `{ older = "7d" }` arrive this way, so nothing is quoted only to be parsed again.
pub fn from_items(items: &[Vec<String>]) -> Result<Qualifiers, String> {
    let mut q = Qualifiers::default();
    for item in items {
        let words: Vec<&str> = item.iter().map(String::as_str).collect();
        if !words.is_empty() {
            apply_item(&mut q, &words)?;
        }
    }
    Ok(q)
}

/// The qualifiers that take a value, for callers spelling them as options: `--older 7d`.
pub const VALUED: &[&str] = &[
    "older", "newer", "larger", "bigger", "smaller", "user", "group", "perm", "not", "re", "num",
    "depth", "by", "first", "last", "newest", "oldest", "largest", "smallest",
];

/// Whether `word` names a qualifier — how the prompt tells `*(file)` from bash's `*(a|b)`.
pub fn is_name(word: &str) -> bool {
    word == "path" || VALUED.contains(&word) || from_items(&[vec![word.to_string()]]).is_ok()
}

/// Split at top-level commas and then into words, honouring `'…'` and `"…"`.
fn items(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut out = vec![Vec::new()];
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' | '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some(c) if c == ch => break,
                        Some(c) => word.push(c),
                        None => return Err(format!("unterminated {ch} in `{text}`")),
                    }
                }
            }
            ',' => {
                push_word(&mut out, &mut word, &mut in_word);
                out.push(Vec::new());
            }
            c if c.is_whitespace() => push_word(&mut out, &mut word, &mut in_word),
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    push_word(&mut out, &mut word, &mut in_word);
    Ok(out.into_iter().filter(|item| !item.is_empty()).collect())
}

fn push_word(out: &mut [Vec<String>], word: &mut String, in_word: &mut bool) {
    if *in_word {
        if let Some(item) = out.last_mut() {
            item.push(std::mem::take(word));
        }
        *in_word = false;
    }
}

fn apply_item(q: &mut Qualifiers, words: &[&str]) -> Result<(), String> {
    let arg = |n: usize| -> Result<&str, String> {
        words
            .get(n)
            .copied()
            .ok_or_else(|| format!("`{}` needs a value", words[0]))
    };
    let count = |n: usize| -> Result<i64, String> {
        arg(n)?
            .parse::<i64>()
            .map_err(|_| format!("`{}`: `{}` is not a number", words[0], arg(n).unwrap_or("")))
    };
    let taken = match words[0] {
        "file" | "files" => push(q, Test::File),
        "dir" | "dirs" | "directory" => push(q, Test::Dir),
        "link" | "links" => push(q, Test::Link),
        "exec" | "executable" => push(q, Test::Exec),
        "socket" => push(q, Test::Socket),
        "fifo" | "pipe" => push(q, Test::Fifo),
        "empty" => push(q, Test::Empty),
        "mine" => push(q, Test::Mine),
        "hidden" => {
            q.hidden = true;
            1
        }
        "nocase" => {
            q.nocase = true;
            1
        }
        "rev" | "reverse" => {
            q.rev = true;
            1
        }
        "older" => push(q, Test::Older(duration(arg(1)?)?)) + 1,
        "newer" => push(q, Test::Newer(duration(arg(1)?)?)) + 1,
        "larger" | "bigger" => push(q, Test::Larger(size(arg(1)?)?)) + 1,
        "smaller" => push(q, Test::Smaller(size(arg(1)?)?)) + 1,
        "user" => push(q, Test::User(uid(arg(1)?)?)) + 1,
        "group" => push(q, Test::Group(gid(arg(1)?)?)) + 1,
        "perm" => push(q, Test::Perm(perm(arg(1)?)?)) + 1,
        "not" => push(q, Test::Not(ShellPattern::from_unquoted(arg(1)?))) + 1,
        "re" => push(q, regex_test(arg(1)?, false)?) + 1,
        "path" if words.get(1) == Some(&"re") => push(q, regex_test(arg(2)?, true)?) + 2,
        "num" => {
            let (lo, hi) = range(arg(1)?)?;
            push(q, Test::Num(lo, hi)) + 1
        }
        "depth" => {
            let (lo, hi) = range(arg(1)?)?;
            q.depth = Some((lo.max(0) as usize, hi.max(0) as usize));
            2
        }
        "by" => {
            q.by = Some(match arg(1)? {
                "name" => By::Name,
                "size" => By::Size,
                "time" | "mtime" | "modified" => By::Time,
                "ext" | "extension" => By::Ext,
                "depth" => By::Depth,
                other => {
                    return Err(format!(
                        "`by {other}`: order by name, size, time, ext or depth"
                    ));
                }
            });
            2
        }
        "first" => take(q, count(1)?, None, false),
        "last" => take(q, -count(1)?, None, false),
        "newest" => take(q, count(1)?, Some(By::Time), true),
        "oldest" => take(q, count(1)?, Some(By::Time), false),
        "largest" => take(q, count(1)?, Some(By::Size), true),
        "smallest" => take(q, count(1)?, Some(By::Size), false),
        "tracked" => push(q, Test::Git(Git::Tracked)),
        "untracked" => push(q, Test::Git(Git::Untracked)),
        "modified" => push(q, Test::Git(Git::Modified)),
        "ignored" => push(q, Test::Git(Git::Ignored)),
        "gitignore" => push(q, Test::Git(Git::NotIgnored)),
        other => return Err(format!("`{other}` is not a qualifier")),
    };
    match words.get(taken) {
        Some(extra) => Err(format!("`{}`: unexpected `{extra}`", words[0])),
        None => Ok(()),
    }
}

fn push(q: &mut Qualifiers, test: Test) -> usize {
    q.tests.push(test);
    1
}

fn take(q: &mut Qualifiers, n: i64, by: Option<By>, rev: bool) -> usize {
    q.take = Some(n);
    if let Some(by) = by {
        q.by = Some(by);
        q.rev = rev;
    }
    2
}

/// `7d`, `2h`, `30m`, `90s`, `2w`, `6M`, `1y`.
fn duration(text: &str) -> Result<Duration, String> {
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, unit) = text.split_at(split);
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("`{text}` is not a duration; try 7d or 2h"))?;
    let seconds = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        "M" => 2_592_000,
        "y" => 31_536_000,
        _ => return Err(format!("`{text}`: units are s m h d w M y")),
    };
    Ok(Duration::from_secs(n.saturating_mul(seconds)))
}

/// `10k`, `1M`, `2G`, `512` — powers of 1024, as `ls -h` counts.
fn size(text: &str) -> Result<u64, String> {
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, unit) = text.split_at(split);
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("`{text}` is not a size; try 10k or 1M"))?;
    let scale: u64 = match unit {
        "" | "b" | "B" => 1,
        "k" | "K" => 1 << 10,
        "m" | "M" => 1 << 20,
        "g" | "G" => 1 << 30,
        "t" | "T" => 1 << 40,
        _ => return Err(format!("`{text}`: units are k M G T")),
    };
    Ok(n.saturating_mul(scale))
}

/// `3`, `1-3`, `5-` (open), `-5`.
fn range(text: &str) -> Result<(i64, i64), String> {
    let bad = || format!("`{text}` is not a range; try 1-3");
    match text.split_once('-') {
        None => text.parse().map(|n| (n, n)).map_err(|_| bad()),
        Some((lo, hi)) => {
            let lo = if lo.is_empty() {
                i64::MIN
            } else {
                lo.parse().map_err(|_| bad())?
            };
            let hi = if hi.is_empty() {
                i64::MAX
            } else {
                hi.parse().map_err(|_| bad())?
            };
            Ok((lo, hi))
        }
    }
}

fn uid(name: &str) -> Result<u32, String> {
    if let Ok(n) = name.parse() {
        return Ok(n);
    }
    match nix::unistd::User::from_name(name) {
        Ok(Some(user)) => Ok(user.uid.as_raw()),
        _ => Err(format!("`{name}` is not a user")),
    }
}

fn gid(name: &str) -> Result<u32, String> {
    if let Ok(n) = name.parse() {
        return Ok(n);
    }
    match nix::unistd::Group::from_name(name) {
        Ok(Some(group)) => Ok(group.gid.as_raw()),
        _ => Err(format!("`{name}` is not a group")),
    }
}

fn perm(text: &str) -> Result<u32, String> {
    u32::from_str_radix(text, 8).map_err(|_| format!("`{text}` is not an octal mode; try 644"))
}

fn regex_test(source: &str, whole: bool) -> Result<Test, String> {
    regex::Regex::new(source)
        .map(|regex| Test::Re { regex, whole })
        .map_err(|e| format!("`{source}`: {e}"))
}

/// The fixed directory a pattern starts from — everything before its first component that globs.
///
/// What `depth` counts from: `src/**/*.rs` is depth 0 at `src/main.rs`, whatever lies above `src`.
pub fn base_of(pattern: &str) -> &str {
    let mut end = 0;
    for (at, _) in pattern.match_indices('/') {
        if pattern[end..at].contains(['*', '?', '[']) {
            break;
        }
        end = at + 1;
    }
    &pattern[..end]
}

/// Filter, order and slice `paths`, each as the shell spells it. `base` is where `depth` counts
/// from — see [`base_of`].
pub fn apply(paths: Vec<String>, q: &Qualifiers, base: &str, collate: bool) -> Vec<String> {
    let git = GitSets::for_tests(&q.tests);
    let now = SystemTime::now();
    let mut kept: Vec<Entry> = paths
        .into_iter()
        .map(|path| Entry::of(path, base))
        .filter(|e| q.depth.is_none_or(|(lo, hi)| (lo..=hi).contains(&e.depth)))
        .filter(|e| q.tests.iter().all(|t| passes(t, e, now, &git)))
        .collect();
    if let Some(by) = q.by {
        let key = |e: &Entry| -> (u128, String) {
            match by {
                By::Size => (u128::from(e.size), String::new()),
                By::Time => (e.mtime, String::new()),
                By::Depth => (e.depth as u128, String::new()),
                By::Ext => (0, e.ext().to_string()),
                By::Name => (0, String::new()),
            }
        };
        kept.sort_by(|a, b| {
            key(a)
                .cmp(&key(b))
                .then_with(|| order_names(&a.path, &b.path, collate))
        });
    }
    if q.rev {
        kept.reverse();
    }
    if let Some(n) = q.take {
        let n_abs = n.unsigned_abs() as usize;
        kept = match n >= 0 {
            true => kept.into_iter().take(n_abs).collect(),
            false => {
                let skip = kept.len().saturating_sub(n_abs);
                kept.into_iter().skip(skip).collect()
            }
        };
    }
    kept.into_iter().map(|e| e.path).collect()
}

fn order_names(a: &str, b: &str, collate: bool) -> std::cmp::Ordering {
    match collate {
        true => super::collate::compare(a, b),
        false => a.cmp(b),
    }
}

/// One path and what the tests ask of it, read once.
pub struct Entry {
    pub path: String,
    pub lstat: Option<std::fs::Metadata>,
    pub stat: Option<std::fs::Metadata>,
    pub size: u64,
    pub mtime: u128,
    pub depth: usize,
}

impl Entry {
    /// `base` is where depth is counted from.
    pub fn of(path: String, base: &str) -> Entry {
        let real = crate::lossless::to_os(&path);
        let lstat = std::fs::symlink_metadata(&real).ok();
        let stat = std::fs::metadata(&real).ok();
        let size = stat.as_ref().map_or(0, |m| m.len());
        let mtime = stat
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let below = path.strip_prefix(base).unwrap_or(&path);
        let depth = below.trim_end_matches('/').matches('/').count();
        Entry {
            path,
            lstat,
            stat,
            size,
            mtime,
            depth,
        }
    }

    pub fn name(&self) -> &str {
        let trimmed = self.path.trim_end_matches('/');
        trimmed.rsplit('/').next().unwrap_or(trimmed)
    }

    pub fn ext(&self) -> &str {
        let name = self.name();
        match name.rfind('.') {
            Some(at) if at > 0 => &name[at + 1..],
            _ => "",
        }
    }

    pub fn is_dir(&self) -> bool {
        self.stat.as_ref().is_some_and(|m| m.is_dir())
    }

    pub fn kind(&self) -> &'static str {
        use std::os::unix::fs::FileTypeExt;
        let Some(lstat) = &self.lstat else {
            return "missing";
        };
        let t = lstat.file_type();
        match () {
            _ if t.is_symlink() => "link",
            _ if t.is_dir() => "dir",
            _ if t.is_socket() => "socket",
            _ if t.is_fifo() => "fifo",
            _ => "file",
        }
    }
}

fn passes(test: &Test, e: &Entry, now: SystemTime, git: &GitSets) -> bool {
    use std::os::unix::fs::FileTypeExt;
    let lstat = e.lstat.as_ref();
    let stat = e.stat.as_ref();
    let age = || {
        stat.and_then(|m| m.modified().ok())
            .and_then(|t| now.duration_since(t).ok())
    };
    match test {
        Test::File => stat.is_some_and(|m| m.is_file()),
        Test::Dir => e.is_dir(),
        Test::Link => lstat.is_some_and(|m| m.file_type().is_symlink()),
        Test::Exec => stat.is_some_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0),
        Test::Socket => stat.is_some_and(|m| m.file_type().is_socket()),
        Test::Fifo => stat.is_some_and(|m| m.file_type().is_fifo()),
        Test::Empty => match stat {
            Some(m) if m.is_dir() => std::fs::read_dir(crate::lossless::to_os(&e.path))
                .is_ok_and(|mut d| d.next().is_none()),
            Some(m) => m.len() == 0,
            None => false,
        },
        Test::Older(d) => age().is_some_and(|a| a > *d),
        Test::Newer(d) => age().is_some_and(|a| a < *d),
        Test::Larger(n) => e.size > *n,
        Test::Smaller(n) => e.size < *n,
        Test::Mine => lstat.is_some_and(|m| m.uid() == nix::unistd::geteuid().as_raw()),
        Test::User(uid) => lstat.is_some_and(|m| m.uid() == *uid),
        Test::Group(gid) => lstat.is_some_and(|m| m.gid() == *gid),
        Test::Perm(mode) => stat.is_some_and(|m| m.permissions().mode() & 0o7777 == *mode),
        Test::Not(pattern) => !pattern.matches(e.name()) && !pattern.matches(&e.path),
        Test::Re { regex, whole } => regex.is_match(if *whole { &e.path } else { e.name() }),
        Test::Num(lo, hi) => first_number(e.name()).is_some_and(|n| (*lo..=*hi).contains(&n)),
        Test::Git(kind) => git.holds(*kind, &e.path),
    }
}

fn first_number(name: &str) -> Option<i64> {
    let start = name.find(|c: char| c.is_ascii_digit())?;
    let digits: String = name[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// The answers `git ls-files` gave, asked once per expansion for the kinds the tests name.
#[derive(Default)]
struct GitSets {
    sets: Vec<(Git, std::collections::HashSet<String>)>,
}

impl GitSets {
    fn for_tests(tests: &[Test]) -> GitSets {
        let mut sets = Vec::new();
        for test in tests {
            let Test::Git(kind) = test else { continue };
            let ask = match kind {
                Git::Tracked => vec!["ls-files", "-z"],
                Git::Untracked => vec!["ls-files", "-z", "--others", "--exclude-standard"],
                Git::Modified => vec!["ls-files", "-z", "--modified"],
                Git::Ignored | Git::NotIgnored => {
                    vec![
                        "ls-files",
                        "-z",
                        "--others",
                        "--ignored",
                        "--exclude-standard",
                        "--directory",
                    ]
                }
            };
            let listed = std::process::Command::new("git")
                .args(&ask)
                .stderr(std::process::Stdio::null())
                .output()
                .map(|out| {
                    out.stdout
                        .split(|b| *b == 0)
                        .filter(|p| !p.is_empty())
                        .map(|p| String::from_utf8_lossy(p).trim_end_matches('/').to_string())
                        .collect()
                })
                .unwrap_or_default();
            sets.push((*kind, listed));
        }
        GitSets { sets }
    }

    fn holds(&self, kind: Git, path: &str) -> bool {
        let path = path
            .strip_prefix("./")
            .unwrap_or(path)
            .trim_end_matches('/');
        let listed = |k: Git| {
            self.sets
                .iter()
                .find(|(have, _)| *have == k)
                .is_some_and(|(_, set)| {
                    set.contains(path) || set.iter().any(|dir| path.starts_with(&format!("{dir}/")))
                })
        };
        match kind {
            Git::NotIgnored => !listed(Git::NotIgnored),
            other => listed(other),
        }
    }
}

#[cfg(test)]
#[path = "qualify/tests.rs"]
mod tests;
