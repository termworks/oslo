//! Pathname expansion: a pattern matched against the filesystem, one directory at a time.
//!
//! **The only walker.** The executor, the prompt's Tab and highlighter, and Lua all expand through
//! here, so what completion offers and what the shell then runs cannot disagree about what `*` or
//! `**` means. It lives in `oslo-base` for the same reason the matcher does: the interface layer
//! cannot see the shell.
//!
//! # `**`, exactly as bash 5.3 has it
//!
//! ```text
//!   **          every entry below, recursively; a symlinked directory is listed, not entered
//!   dir/**      dir/ itself, then every entry below it
//!   **/         every directory below, each with a trailing /, symlinked ones included
//!   a/**/b      b in a/, and in every real directory below a/
//! ```
//!
//! Hidden entries are skipped at every depth unless `dotglob` is on, so `**` never wanders into
//! `.git`. A directory named outright — `.hdir/**`, `lnk/**` — is entered whatever it is.
//!
//! # One read per directory
//!
//! `a/**/*.rs` reads each directory to find the ones below it and then again to match `*.rs`. The
//! `Dirs` cache makes that one `readdir` per directory per expansion.

use super::{Item, compile_items, matches_items_with};
use std::collections::HashMap;
use std::fs;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What decides how a pattern meets the filesystem.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// `**` alone in a component crosses directories.
    pub globstar: bool,
    /// `*`, `?` and `**` match names that begin with a dot (never `.` or `..`).
    pub dotglob: bool,
    /// Sort matches by the locale's rules rather than by bytes; see [`super::collate`].
    pub collate: bool,
    /// Match names case-insensitively: `nocaseglob`.
    pub nocase: bool,
    /// The most directory entries one expansion may read, for callers on a keystroke. `None` —
    /// the shell's own — reads everything.
    pub budget: Option<usize>,
}

/// What a bounded expansion found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    /// Sorted and without duplicates.
    pub paths: Vec<String>,
    /// False when the budget ran out, so `paths` may be missing some.
    pub complete: bool,
    /// Directory entries read to answer, for a caller spending one budget across several patterns.
    pub read: usize,
}

/// The shell's own settings, switched by `shopt`.
///
/// Process-wide because the shell is one process: a subshell is a fork and inherits them, which is
/// exactly what bash does. Callers that are not the shell — Lua, Tab — pass their own [`Options`].
static GLOBSTAR: AtomicBool = AtomicBool::new(false);
static DOTGLOB: AtomicBool = AtomicBool::new(false);
static NOCASEGLOB: AtomicBool = AtomicBool::new(false);
static NULLGLOB: AtomicBool = AtomicBool::new(false);
static FAILGLOB: AtomicBool = AtomicBool::new(false);
static NOCASEMATCH: AtomicBool = AtomicBool::new(false);
static EXTGLOB: AtomicBool = AtomicBool::new(false);

/// `shopt -s extglob`: `@(a|b)` and the other groups are patterns, not text.
pub fn set_extglob(on: bool) {
    EXTGLOB.store(on, Ordering::Relaxed);
}

pub fn extglob() -> bool {
    EXTGLOB.load(Ordering::Relaxed)
}

/// `shopt -s globstar` / `shopt -u globstar`.
pub fn set_globstar(on: bool) {
    GLOBSTAR.store(on, Ordering::Relaxed);
}

/// `shopt -s dotglob` / `shopt -u dotglob`.
pub fn set_dotglob(on: bool) {
    DOTGLOB.store(on, Ordering::Relaxed);
}

/// `shopt -s nocaseglob` / `shopt -u nocaseglob`.
pub fn set_nocaseglob(on: bool) {
    NOCASEGLOB.store(on, Ordering::Relaxed);
}

/// `shopt -s nullglob`: a pattern that matches nothing expands to nothing.
pub fn set_nullglob(on: bool) {
    NULLGLOB.store(on, Ordering::Relaxed);
}

/// `shopt -s failglob`: a pattern that matches nothing is an error, and the command does not run.
pub fn set_failglob(on: bool) {
    FAILGLOB.store(on, Ordering::Relaxed);
}

/// `shopt -s nocasematch`: `case` and `[[ ]]` match case-insensitively.
pub fn set_nocasematch(on: bool) {
    NOCASEMATCH.store(on, Ordering::Relaxed);
}

pub fn nullglob() -> bool {
    NULLGLOB.load(Ordering::Relaxed)
}

pub fn failglob() -> bool {
    FAILGLOB.load(Ordering::Relaxed)
}

pub fn nocasematch() -> bool {
    NOCASEMATCH.load(Ordering::Relaxed)
}

/// The options the shell is running with right now.
pub fn shell_options() -> Options {
    Options {
        globstar: GLOBSTAR.load(Ordering::Relaxed),
        dotglob: DOTGLOB.load(Ordering::Relaxed),
        collate: super::collate::locale_collates(),
        nocase: NOCASEGLOB.load(Ordering::Relaxed),
        budget: None,
    }
}

/// One `/`-separated piece of a pattern.
#[derive(Debug, PartialEq, Eq)]
enum Component {
    /// Nothing in it globs, so it names a directory entry outright.
    Literal(String),
    Pattern(Vec<Item>),
    /// `**` with globstar on: the one piece that spans several path components.
    Globstar,
}

/// Expand a pattern given as `(character, still-a-metacharacter)` pairs.
///
/// `None` when nothing in it globs, so the caller keeps its own text without a walk. Otherwise the
/// matches, sorted and without duplicates — possibly none, which the caller turns into the literal
/// text, nothing, or an error, as `nullglob` and `failglob` decide.
pub fn expand(chars: &[(char, bool)], options: &Options) -> Option<Vec<String>> {
    expand_counted(chars, options).map(|expansion| expansion.paths)
}

/// [`expand`], saying whether [`Options::budget`] let it finish.
pub fn expand_counted(chars: &[(char, bool)], options: &Options) -> Option<Expansion> {
    let (components, trailing_slash) = split(chars, options);
    if !components
        .iter()
        .any(|c| matches!(c, Component::Pattern(_) | Component::Globstar))
    {
        return None;
    }
    let mut dirs = Dirs {
        left: options.budget,
        ..Dirs::default()
    };
    let mut found = walk(&components, trailing_slash, options, &mut dirs);
    super::collate::sort(&mut found, options.collate);
    found.dedup();
    Some(Expansion {
        paths: found,
        complete: !dirs.exhausted,
        read: dirs.total,
    })
}

/// Cut at every `/` and compile each piece; the flag records a trailing `/`.
fn split(chars: &[(char, bool)], options: &Options) -> (Vec<Component>, bool) {
    let mut pieces: Vec<Vec<(char, bool)>> = vec![Vec::new()];
    for &(ch, globs) in chars {
        if ch == '/' {
            pieces.push(Vec::new());
        } else if let Some(piece) = pieces.last_mut() {
            piece.push((ch, globs));
        }
    }
    let trailing_slash = pieces.len() > 1 && pieces.last().is_some_and(Vec::is_empty);
    if trailing_slash {
        pieces.pop();
    }
    let components = pieces.iter().map(|p| compile(p, options)).collect();
    (components, trailing_slash)
}

/// Compile one component. `**` is only special alone, unquoted, and with globstar on.
fn compile(chars: &[(char, bool)], options: &Options) -> Component {
    if options.globstar && chars.len() == 2 && chars.iter().all(|&(c, globs)| c == '*' && globs) {
        return Component::Globstar;
    }
    let (items, has_metacharacter) = compile_items(chars);
    if has_metacharacter {
        return Component::Pattern(items);
    }
    // From the items, not the characters: an escaped `\*` names a file called `*`.
    Component::Literal(
        items
            .iter()
            .filter_map(|item| match item {
                Item::Ch(c) => Some(*c),
                _ => None,
            })
            .collect(),
    )
}

/// Whether a directory entry named `name` matches, with the pathname-only leading-dot rule.
fn name_matches(items: &[Item], name: &str, options: &Options) -> bool {
    if name.starts_with('.') && !options.dotglob && items.first() != Some(&Item::Ch('.')) {
        return false;
    }
    matches_items_with(items, name, options.nocase)
}

/// What `readdir` said an entry is, before anything follows a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dir,
    Link,
    Other,
}

struct Entry {
    name: String,
    kind: Kind,
}

/// Directory listings read during one expansion, keyed by the prefix that names them.
#[derive(Default)]
struct Dirs {
    read: HashMap<String, Rc<[Entry]>>,
    /// Entries still allowed to be read, when there is a budget.
    left: Option<usize>,
    /// The budget ran out, so some directory was read short or not at all.
    exhausted: bool,
    /// Entries read so far.
    total: usize,
}

impl Dirs {
    /// The entries of the directory `prefix` names — `""` is the working directory.
    fn list(&mut self, prefix: &str) -> Rc<[Entry]> {
        if let Some(known) = self.read.get(prefix) {
            return Rc::clone(known);
        }
        if self.left == Some(0) {
            self.exhausted = true;
            return Rc::from(Vec::new());
        }
        let dir = if prefix.is_empty() { "." } else { prefix };
        let allowed = self.left.unwrap_or(usize::MAX);
        let entries: Rc<[Entry]> = match fs::read_dir(dir) {
            Ok(entries) => entries
                .flatten()
                .take(allowed)
                .map(|entry| Entry {
                    name: name_of(&entry.file_name()),
                    kind: match entry.file_type() {
                        Ok(t) if t.is_dir() => Kind::Dir,
                        Ok(t) if t.is_symlink() => Kind::Link,
                        _ => Kind::Other,
                    },
                })
                .collect(),
            Err(_) => Rc::from(Vec::new()),
        };
        self.total += entries.len();
        if let Some(left) = self.left.as_mut() {
            *left -= entries.len();
            // Reading exactly up to the limit is indistinguishable from being cut short there.
            if *left == 0 {
                self.exhausted = true;
            }
        }
        self.read.insert(prefix.to_string(), Rc::clone(&entries));
        entries
    }
}

/// A directory entry's name as the rest of the shell carries it: losslessly, see
/// [`crate::lossless`], so `rm b*` gets `bad\xffname` and not a name that does not exist.
fn name_of(name: &std::ffi::OsStr) -> String {
    crate::lossless::encode(name)
}

/// Whether an entry is a directory once a link is followed.
fn entry_is_dir(entry: &Entry, path: &str) -> bool {
    match entry.kind {
        Kind::Dir => true,
        Kind::Link => is_dir(path),
        Kind::Other => false,
    }
}

fn hidden(entry: &Entry, options: &Options) -> bool {
    entry.name.starts_with('.') && !options.dotglob
}

/// Match the components against the filesystem, one directory level at a time.
///
/// The accumulator holds path prefixes built from the pattern's own text, so `./a*` comes back as
/// `./a1`: nothing round-trips through a normalising path type.
fn walk(
    components: &[Component],
    trailing_slash: bool,
    options: &Options,
    dirs: &mut Dirs,
) -> Vec<String> {
    // Each prefix, and whether the pattern named it outright rather than matched it.
    let mut current = vec![Prefix {
        path: String::new(),
        named: true,
    }];

    for (index, component) in components.iter().enumerate() {
        let last = index + 1 == components.len();
        let mut next = Vec::new();
        for base in &current {
            match component {
                Component::Literal(name) => {
                    let path = format!("{}{name}", base.path);
                    if !last {
                        next.push(Prefix::named(path + "/"));
                    } else if exists(&path) && (!trailing_slash || is_dir(&path)) {
                        next.push(Prefix::named(finish(path, trailing_slash)));
                    }
                }
                Component::Pattern(items) => {
                    for entry in dirs.list(&base.path).iter() {
                        if !name_matches(items, &entry.name, options) {
                            continue;
                        }
                        let path = format!("{}{}", base.path, entry.name);
                        if !last {
                            if entry_is_dir(entry, &path) {
                                next.push(Prefix::matched(path + "/"));
                            }
                        } else if !trailing_slash || entry_is_dir(entry, &path) {
                            next.push(Prefix::matched(finish(path, trailing_slash)));
                        }
                    }
                }
                Component::Globstar => {
                    globstar(base, last, trailing_slash, options, dirs, &mut next);
                }
            }
        }
        current = next;
        if current.is_empty() {
            break;
        }
    }
    current.into_iter().map(|prefix| prefix.path).collect()
}

/// A path the walk has reached, ending in `/` while more components follow.
struct Prefix {
    path: String,
    /// Written in the pattern rather than matched by it — which decides whether a final `**`
    /// spells it with its slash: `dir1/**` gives `dir1/`, `*/**` gives `dir1`.
    named: bool,
}

impl Prefix {
    fn named(path: String) -> Prefix {
        Prefix { path, named: true }
    }

    fn matched(path: String) -> Prefix {
        Prefix { path, named: false }
    }
}

/// What one `**` contributes from one base.
///
/// A base the pattern reached is part of the answer when it is a directory (`dir1/**` includes
/// `dir1/`); the working directory, being unnamed, is not.
fn globstar(
    base: &Prefix,
    last: bool,
    trailing_slash: bool,
    options: &Options,
    dirs: &mut Dirs,
    out: &mut Vec<Prefix>,
) {
    let path = &base.path;
    let here = path.is_empty() || is_dir(path);
    if !here {
        return;
    }
    let mut below = Vec::new();
    descend(path, options, dirs, &mut below);
    if !last {
        out.push(Prefix {
            path: path.clone(),
            named: base.named,
        });
        out.extend(
            below
                .into_iter()
                .filter(|(_, kind)| *kind == Reached::Directory)
                .map(|(path, _)| Prefix::matched(path + "/")),
        );
        return;
    }
    if !path.is_empty() {
        let spelled = match base.named || trailing_slash {
            true => path.clone(),
            false => path.trim_end_matches('/').to_string(),
        };
        out.push(Prefix::matched(spelled));
    }
    for (path, kind) in below {
        if !trailing_slash {
            out.push(Prefix::matched(path));
        } else if kind != Reached::Leaf {
            out.push(Prefix::matched(path + "/"));
        }
    }
}

/// What a `**` walk found at one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reached {
    /// A real directory, which the walk entered.
    Directory,
    /// A symbolic link to a directory: listed, never entered.
    Linked,
    Leaf,
}

/// Every visible entry below `base`, depth first. Only real directories are entered, so a link to
/// an ancestor cannot make the walk endless.
fn descend(base: &str, options: &Options, dirs: &mut Dirs, out: &mut Vec<(String, Reached)>) {
    for entry in dirs.list(base).iter() {
        if hidden(entry, options) {
            continue;
        }
        let path = format!("{base}{}", entry.name);
        let reached = match entry.kind {
            Kind::Dir => Reached::Directory,
            Kind::Link if is_dir(&path) => Reached::Linked,
            _ => Reached::Leaf,
        };
        out.push((path.clone(), reached));
        if reached == Reached::Directory {
            descend(&format!("{path}/"), options, dirs, out);
        }
    }
}

fn finish(path: String, trailing_slash: bool) -> String {
    if trailing_slash { path + "/" } else { path }
}

/// Whether the path names anything, a dangling symlink included.
fn exists(path: &str) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn is_dir(path: &str) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_dir())
}

#[cfg(test)]
#[path = "walk/tests.rs"]
mod tests;
