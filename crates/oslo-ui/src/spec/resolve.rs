//! Turning a declared position into offers, once the line is known.
//!
//! ```text
//!   Action::List(["dev", "staging\tshared", "$files([.yaml])", "$tag(env)"])
//!        │
//!        ├─ ${…} substituted from the line          spec::vars
//!        ├─ each entry read as literal / macro / modifier   spec::action
//!        ├─ macros answered here, or by the runner the shell installed
//!        └─ modifiers applied to what came back
//!        ▼
//!   Resolved { offers, paths, split, dir }
//! ```
//!
//! # `$files` is not answered here
//!
//! It is reported as a *request* for path completion instead, in [`Resolved::paths`], and the
//! caller runs oslo's own. That is the whole reason for reading these specs in a shell rather than
//! in a completion binary: oslo's path builder already knows about tildes, globs, quoting, the
//! directory entry count and the size column, and a second implementation living here would be a
//! worse one that also has to be kept in step.
//!
//! The cost is that value modifiers — `$prefix`, `$suffix`, `$filter` — do not reach path
//! candidates. `$chdir` does, because it decides *where*, not *what*.

pub mod traverse;

use super::action::{Action, Modifier, Offer, Piece, Query};
use super::vars;

/// Take in a second path marker beside the first, widening what is accepted.
///
/// **Two markers mean both, not the second one.** `["$files", "$directories"]` is how 225 of the
/// converted specs say "anything on disk" — `ls`, `cp`, `mv` among them — and each marker used to
/// overwrite the last, so `$directories` arriving second turned "anything" into "directories only"
/// and every file vanished from the menu. Each field widens rather than replaces: dirs-only holds
/// only while every marker asked for it, and one marker with no suffix filter means any name.
fn widen(slot: &mut Option<Paths>, add: Paths) {
    let Some(have) = slot else {
        *slot = Some(add);
        return;
    };
    have.only_dirs &= add.only_dirs;
    have.only_executables &= add.only_executables;
    // Empty means every name, so an unfiltered marker beside a filtered one drops the filter.
    if have.suffixes.is_empty() || add.suffixes.is_empty() {
        have.suffixes.clear();
    } else {
        have.suffixes.extend(add.suffixes);
    }
}

/// A position that asked for path completion, and what it will accept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Paths {
    /// `$directories`.
    pub only_dirs: bool,
    /// `$executables`.
    pub only_executables: bool,
    /// `$files([.go, go.mod])` — names ending in any of these. Empty means every file.
    pub suffixes: Vec<String>,
}

/// Everything a position offers.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub offers: Vec<Offer>,
    /// Set when the position asked for path completion. Runs alongside `offers`, not instead.
    pub paths: Option<Paths>,
    /// `$list(,)` — the word is a delimited list, and only its last element is being completed.
    pub split: Option<String>,
    /// `$uniquelist` — and an element already in the word is not offered a second time.
    pub unique: bool,
    /// Where to look, after `$chdir`. Empty means the working directory.
    pub dir: String,
}

impl Resolved {
    /// Whether the position said anything at all. A position that did not is one the caller should
    /// answer the way it would have without a spec.
    pub fn is_empty(&self) -> bool {
        self.offers.is_empty() && self.paths.is_none()
    }
}

/// What `action` offers for this line.
pub fn resolve(action: &Action, query: &Query) -> Resolved {
    match action {
        Action::None => Resolved::default(),
        Action::Call(compute) => Resolved {
            offers: compute(query),
            dir: query.dir.clone(),
            ..Resolved::default()
        },
        Action::List(list) => list_offers(list, query),
    }
}

fn list_offers(list: &[String], query: &Query) -> Resolved {
    let mut out = Resolved {
        dir: query.dir.clone(),
        ..Resolved::default()
    };
    // The query the macros see, which `$chdir` can move out from under them. Cloned once rather
    // than per entry: a value list is read left to right and a modifier applies from where it sits.
    let mut here = query.clone();

    for text in list {
        let entry = super::action::entry(&vars::expand(text, query));
        match entry.piece {
            // A modifier at the head applies to everything produced so far.
            None => apply(&entry.modifiers, &mut out, &mut here, 0),
            Some(piece) => {
                let at = out.offers.len();
                produce(&piece, &here, &mut out);
                // …and one behind a `|||` applies to this entry alone.
                apply(&entry.modifiers, &mut out, &mut here, at);
            }
        }
    }
    out
}

/// Add what one literal or macro offers.
fn produce(piece: &Piece, query: &Query, out: &mut Resolved) {
    match piece {
        Piece::Literal { value, description } => {
            if !value.is_empty() {
                out.offers.push(Offer {
                    value: value.clone(),
                    description: description.clone(),
                    tag: None,
                });
            }
        }
        Piece::Macro { name, arg } => match name.as_str() {
            "files" => widen(
                &mut out.paths,
                Paths {
                    suffixes: super::action::bracketed(arg),
                    ..Paths::default()
                },
            ),
            "directories" => widen(
                &mut out.paths,
                Paths {
                    only_dirs: true,
                    ..Paths::default()
                },
            ),
            "executables" => widen(
                &mut out.paths,
                Paths {
                    only_executables: true,
                    ..Paths::default()
                },
            ),
            // A row saying why there is nothing to offer. oslo's dropdown has no such row: every
            // line in it is something the Tab key will insert, and a message is not.
            "message" => {}
            // **What the machine knows** — `$hosts`, `$pids`, `$users`, `$mounts` and the rest.
            //
            // Tried before the shell, because these are precisely the ones that must not fork: a
            // source answers from `/proc`, `/sys`, `/etc` or the environment, and the whole reason
            // they exist is that the alternative is `ps`, `getent` or `systemctl` on the Tab key.
            // See [`super::sources`].
            _ => match super::sources::offers(name, query) {
                Some(found) => out.offers.extend(found.into_iter().map(|one| Offer {
                    value: one.value,
                    description: Some(one.note),
                    // Every other spec offer is labelled `value`, which says nothing when the rows
                    // are processes or mount points.
                    tag: Some(one.kind.to_string()),
                })),
                // Everything left needs a shell: `$(git branch)`, `$bash(…)`, `$spec(other.yaml)`.
                None => {
                    if let Some(runner) = super::action::runner() {
                        out.offers.extend(runner(name, arg, query));
                    }
                }
            },
        },
    }
}

/// Apply modifiers to the offers from `at` onwards.
fn apply(modifiers: &[Modifier], out: &mut Resolved, query: &mut Query, at: usize) {
    for modifier in modifiers {
        match modifier {
            Modifier::Chdir(arg) => {
                if let Some(dir) = traverse::target(arg, &query.dir) {
                    query.dir = dir.clone();
                    out.dir = dir;
                }
            }
            Modifier::Filter(values) => {
                out.offers.truncate_from(at, |o| !values.contains(&o.value));
            }
            Modifier::Retain(values) => {
                out.offers.truncate_from(at, |o| values.contains(&o.value));
            }
            Modifier::FilterArgs => {
                let args = query.args.clone();
                out.offers.truncate_from(at, |o| !args.contains(&o.value));
            }
            Modifier::List(sep) => out.split = Some(sep.clone()),
            Modifier::UniqueList(sep) => {
                out.split = Some(sep.clone());
                out.unique = true;
            }
            // Completing one part of a delimited word is the same question a list asks, and oslo
            // answers both by retargeting the word at its last piece.
            Modifier::MultiParts(seps) => out.split = seps.first().cloned(),
            Modifier::Prefix(prefix) => {
                for offer in &mut out.offers[at..] {
                    offer.value.insert_str(0, prefix);
                }
            }
            Modifier::Suffix(suffix) => {
                for offer in &mut out.offers[at..] {
                    offer.value.push_str(suffix);
                }
            }
            Modifier::Tag(tag) => {
                for offer in &mut out.offers[at..] {
                    offer.tag = Some(tag.clone());
                }
            }
            Modifier::Ignored => {}
        }
    }
}

/// `retain`, but only over the tail — the offers an earlier entry produced are not this one's to
/// drop.
trait TruncateFrom {
    fn truncate_from(&mut self, at: usize, keep: impl Fn(&Offer) -> bool);
}

impl TruncateFrom for Vec<Offer> {
    fn truncate_from(&mut self, at: usize, keep: impl Fn(&Offer) -> bool) {
        // **Counting from zero, because `retain` does.** Seeded with `at`, the very first offer was
        // numbered `at` and so failed the `here < at` guard — which meant a `|||`-scoped filter
        // applied to the whole list and quietly deleted what earlier entries had produced. The
        // guarantee this trait exists for was the thing it did not keep.
        let mut index = 0usize;
        self.retain(|offer| {
            let here = index;
            index += 1;
            here < at || keep(offer)
        });
    }
}

#[cfg(test)]
#[path = "resolve/tests.rs"]
mod tests;
