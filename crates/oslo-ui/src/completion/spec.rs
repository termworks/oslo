//! Completing from a command's declared arguments, and from the alias table.
//!
//! Split from `completion.rs` when it crossed the 600-line limit, along the seam the subject
//! already had: every other builder answers from a name or the filesystem, and this one answers
//! from a *declaration* — argc comments, a shipped spec, a config's `oslo.completion.spec`, a
//! carapace spec file — after resolving whatever alias stands in front of it.
//!
//! Reading the line is [`walk`]; deciding what a position offers is [`crate::spec::resolve`]. What
//! is left here is the join between them and the shell facts neither knows: the alias table, the
//! quoting, and oslo's own path completion.

mod walk;

use super::{CompletionCandidate, matches_prefix};
use crate::OsloHelper;
use crate::spec::action::Query;
use crate::spec::resolve::resolve;
use crate::spec::{Action, Arg, OptionSpec, Parsing};
use crate::words::{Quote, Word, quote_replacement, unquote};
use walk::At;

impl OsloHelper {
    /// The command a name really stands for, following aliases.
    ///
    /// Transitive, because aliases chain — `alias g=git`, `alias gc='git commit'` — and bounded,
    /// because they can also loop: `alias a=b` with `alias b=a` is legal to write and must not
    /// hang the shell on Tab.
    ///
    /// **The whole expansion, not just its head.** `alias gco='git checkout'` used to complete as
    /// plain `git`, so `gco -<Tab>` offered git's top-level options — `--version`, `-C` — where
    /// `git checkout -<Tab>` offers `--force` and `-b`. The subcommand the alias names was dropped,
    /// and everybody aliases `gco`.
    fn resolve_alias(&self, name: &str) -> Vec<String> {
        let Ok(env) = self.env.lock() else {
            return vec![name.to_string()];
        };
        let mut words = vec![name.to_string()];
        for _ in 0..16 {
            let head = words[0].clone();
            let Some(expansion) = env.alias(&head) else {
                break;
            };
            let expanded: Vec<String> = expansion
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>();
            let Some(first) = expanded.first() else {
                break;
            };
            if *first == head {
                // `alias ls='ls --color'` — the classic self-reference. It expands to itself, so
                // the answer is already right and following it again would not terminate.
                break;
            }
            // The words this round added come first, then whatever earlier rounds left behind:
            // `alias g=git`, `alias gc='g commit'` has to reach `git commit`, in that order.
            let carried = words[1..].to_vec();
            words = expanded;
            words.extend(carried);
        }
        words
    }

    /// Just the name, for callers that only need to know what is being run.
    pub(super) fn resolve_head(&self, name: &str) -> String {
        self.resolve_alias(name)
            .first()
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }

    /// Answer from the spec for this command, if it has one.
    ///
    /// Reports whether the spec **owned** the position. A position a spec declared and that came
    /// back empty is an answer — `git checkout <Tab>` in a repository with no branches offers
    /// nothing, and falling through to the filenames would be the wrong nothing.
    pub(super) fn spec_candidates(
        &self,
        word: &Word<'_>,
        out: &mut Vec<CompletionCandidate>,
    ) -> bool {
        // `prior_words` holds this command's words only, so `ls | git comm<TAB>` looks up `git`
        // and not `ls`.
        let Some((primary, rest)) = word.prior_words.split_first() else {
            return false;
        };
        // Through the alias table first. Everyone aliases `git`, and `g comm<TAB>` offering
        // nothing is a gap the shell has no excuse for: the alias table is already loaded and this
        // function is already holding the environment.
        let expanded = self.resolve_alias(&unquote(primary));
        let Some((head, from_alias)) = expanded.split_first() else {
            return false;
        };
        let Some(spec) = self.spec_registry.find_spec(head) else {
            return false;
        };

        // **The alias's own words are walked first.** `gco` *is* `git checkout`, so the two halves
        // of the line are the words the alias supplied and then the words that were typed after it.
        let words: Vec<String> = from_alias
            .iter()
            .cloned()
            .chain(rest.iter().map(|token| unquote(token)))
            .collect();
        let walk = walk::walk(&spec, &words);

        let stem = word.stem.as_str();
        let query = Query {
            args: walk.args.clone(),
            words: std::iter::once(head.clone())
                .chain(words.iter().cloned())
                .collect(),
            value: stem.to_string(),
            flags: walk.flags.clone(),
            dir: String::new(),
        };
        // `--env=dev` is one word to the shell and two questions here: `after_break` has already
        // handed over the half after the `=`, and what it cut off says which flag that half
        // belongs to.
        if let Some(flag) = inline_flag(&walk, word) {
            return self.offer_values(&flag.values, &query, word, out);
        }

        // A word beginning with a dash is a flag being named. The walk answers for the line and this
        // answers for the word, which is the half the prior words cannot say anything about.
        if naming_a_flag(&walk, stem) {
            let mut answered = self.offer_flags(&walk, word, out);
            // …and the subcommands, because a subcommand may be spelled with a dash too:
            // `nix-store --gc`, `cmake -E`. At the first position both are legitimate answers to
            // the same word, so the menu offers both rather than guessing which was meant.
            if let At::Positional(index) = walk.at {
                answered |= self.offer_subcommands(&walk, word, index, out);
            }
            // **An empty flag menu is not an answer.** A dashed word matching nothing declared —
            // `./x -` under a spec with no short flags — used to report the position as owned and
            // so suppressed path completion too, leaving the Tab key dead where it had worked
            // before the spec existed. Nothing offered means nobody answered.
            return answered;
        }

        match walk.at {
            At::FlagValue(flag) => self.offer_values(&flag.values, &query, word, out),
            At::Positional(index) => {
                let named = self.offer_subcommands(&walk, word, index, out);
                let action = position(&walk.node.positional, &walk.node.positional_any, index);
                self.offer_values(action, &query, word, out) || named
            }
            At::Dash(index) => {
                let action = position(&walk.node.dash, &walk.node.dash_any, index);
                self.offer_values(action, &query, word, out)
            }
        }
    }

    /// Every flag that could be typed here, matching what has been typed of one.
    ///
    /// Answers whether it pushed anything, because an empty flag menu must not be mistaken for an
    /// answer — see the caller.
    fn offer_flags(
        &self,
        walk: &walk::Walk<'_>,
        word: &Word<'_>,
        out: &mut Vec<CompletionCandidate>,
    ) -> bool {
        let before = out.len();
        self.offer_cluster(walk, word, out);
        let stem = word.stem.as_str();
        let fold = self.case_sensitive();
        for opt in walk.flags_on_offer() {
            for name in &opt.names {
                if matches_prefix(name, stem, fold) {
                    out.push(candidate(word, name, Some(&opt.description), "flag"));
                }
            }
        }
        out.len() != before
    }

    /// One more letter on a run of short flags: `ls -la<Tab>` offers `-lah`.
    ///
    /// **Short flags cluster, and most commands expect it** — 679 of the 720 shipped Fig specs do.
    /// Without this, `-la` matched no whole flag name, the menu came back empty, and the position
    /// was still reported as owned, so the most ordinary keystroke in the shell offered nothing at
    /// all *and* suppressed the filenames it used to offer.
    ///
    /// Only when every letter already typed is a declared short flag — the same all-or-nothing rule
    /// the walk uses, so an unrelated `-xyz` stays the unknown flag it looks like.
    fn offer_cluster(
        &self,
        walk: &walk::Walk<'_>,
        word: &Word<'_>,
        out: &mut Vec<CompletionCandidate>,
    ) {
        let stem = word.stem.as_str();
        let Some(bundle) = walk::cluster(walk.node, &walk.inherited, stem) else {
            return;
        };
        // A letter that takes a value ends the run: `tar -xzf` wants a filename next, and offering
        // `-xzfv` would be putting a flag where that filename goes.
        if bundle.last().is_some_and(|opt| opt.takes != Arg::None) {
            return;
        }
        let held: Vec<char> = stem.trim_start_matches('-').chars().collect();
        for opt in walk.flags_on_offer() {
            for name in &opt.names {
                if name.starts_with("--") {
                    continue;
                }
                let Some(letter) = name
                    .strip_prefix('-')
                    .and_then(|rest| rest.chars().next().filter(|_| rest.chars().count() == 1))
                else {
                    continue;
                };
                if held.contains(&letter) {
                    continue;
                }
                let together = format!("{stem}{letter}");
                out.push(candidate(word, &together, Some(&opt.description), "flag"));
            }
        }
    }

    /// The subcommands of the command the walk stopped at. Only ever the first position: `git add
    /// commit` names two paths, not a command inside a command.
    fn offer_subcommands(
        &self,
        walk: &walk::Walk<'_>,
        word: &Word<'_>,
        index: usize,
        out: &mut Vec<CompletionCandidate>,
    ) -> bool {
        if index != 0 || walk.node.subcommands.is_empty() {
            return false;
        }
        let stem = word.stem.as_str();
        let fold = self.case_sensitive();
        let before = out.len();
        for sub in walk.node.subcommands.iter().filter(|sub| !sub.hidden) {
            // Aliases complete too: `co` beside a description of `checkout` is the only way the
            // menu can explain why two rows are the same command.
            for name in std::iter::once(&sub.name).chain(sub.aliases.iter()) {
                if matches_prefix(name, stem, fold) {
                    out.push(candidate(word, name, Some(&sub.description), "subcommand"));
                }
            }
        }
        out.len() != before
    }

    /// What one declared position offers.
    ///
    /// Answers whether the position was declared at all: see [`Self::spec_candidates`] on why an
    /// empty answer from a declared position is still an answer.
    fn offer_values(
        &self,
        action: &Action,
        query: &Query,
        word: &Word<'_>,
        out: &mut Vec<CompletionCandidate>,
    ) -> bool {
        if action.is_none() {
            return false;
        }
        let word = word.clone();
        let mut query = query.clone();
        query.value = word.stem.clone();
        let resolved = resolve(action, &query);

        // `$list(,)` — the word is several values and only the last is being completed. Retargeting
        // again rather than rewriting the offers: the elements already typed stay on the line.
        let (word, taken) = match resolved.split.as_deref().filter(|sep| !sep.is_empty()) {
            Some(sep) => match word.stem.rfind(sep) {
                Some(last) => {
                    let taken: Vec<String> =
                        word.stem[..last].split(sep).map(str::to_string).collect();
                    match retarget(&word, last + sep.len()) {
                        Some(piece) => (piece, taken),
                        None => return true,
                    }
                }
                None => (word, Vec::new()),
            },
            None => (word, Vec::new()),
        };

        // **Once the word names another machine, only that machine can answer.** Not the local
        // filesystem, and not a second host either: after `tron:` every host row would splice into
        // `tron:othermachine`, a word naming neither. The position was declared, so this answers
        // `true` whatever it found and the caller does not fall back to ordinary path completion.
        if names_another_machine(action, &word) {
            self.remote_candidates(&word, out);
            return true;
        }

        let fold = self.case_sensitive();
        for offer in &resolved.offers {
            if resolved.unique && taken.contains(&offer.value) {
                continue;
            }
            if matches_prefix(&offer.value, &word.stem, fold) {
                let mut row = candidate(&word, &offer.value, offer.description.as_deref(), "value");
                if let Some(tag) = &offer.tag {
                    row.kind = Some(tag.clone());
                }
                out.push(row);
            }
        }
        if let Some(paths) = &resolved.paths {
            let wanted = super::paths::Wanted {
                only_dirs: paths.only_dirs,
                only_runnable: paths.only_executables,
                suffixes: paths.suffixes.clone(),
                root: resolved.dir.clone(),
            };
            self.path_candidates_for(&word, &wanted, out);
        }
        true
    }

    /// What the machine named before the `:` has in the directory being typed.
    ///
    /// Silent unless a lister is installed, which is what makes this safe to reach from a crate
    /// that must not run programs: with none, the position answers nothing and the menu stays shut
    /// — the behaviour before there was any remote listing at all.
    ///
    /// The value of each row is the **whole path**, not the name: the `:` is a word break, so what
    /// gets replaced on the line is everything after it, and a bare `log` would turn
    /// `host:/var/lo` into `host:log`.
    fn remote_candidates(&self, word: &Word<'_>, out: &mut Vec<CompletionCandidate>) {
        let Some(host) = word.prefix.strip_suffix(':') else {
            return;
        };
        let (dir, fragment) = crate::spec::remote::split(&word.stem);
        let Some(found) = crate::spec::remote::entries(host, dir) else {
            return;
        };
        let fold = self.case_sensitive();
        for entry in found {
            // Hidden names only once somebody has typed the dot, as local path completion does.
            if entry.name.starts_with('.') && !fragment.starts_with('.') {
                continue;
            }
            if !matches_prefix(&entry.name, fragment, fold) {
                continue;
            }
            // A directory keeps its `/`, so the next Tab continues into it rather than ending the
            // word.
            let slash = if entry.directory { "/" } else { "" };
            let whole = format!("{dir}{}{slash}", entry.name);
            let kind = if entry.directory {
                "directory"
            } else {
                "remote"
            };
            out.push(candidate(word, &whole, None, kind));
        }
    }
}

/// Whether the word being completed is a path on a *different* machine.
///
/// **`scp f host:/etc/<Tab>` listed the local `/etc`** — 265 entries off this machine, offered as
/// though they were the other one's. Nothing in the menu says which filesystem a row came from, so
/// a wrong answer in the right shape is worse here than no answer: the name it inserts exists, and
/// the copy that uses it fails somewhere else entirely.
///
/// oslo cannot list the far side yet. That needs an `ssh` round trip, and this runs on the Tab
/// keystroke with the terminal in raw mode — the reason every macro that *can* fork is given a
/// deadline. Until it can, the honest answer is nothing.
///
/// **Keyed on the position offering `$hosts`**, which is what marks an operand as one that may name
/// a machine: `ssh`, `scp`, `sftp`, `rsync`, and any spec written the same way. Everywhere else a
/// colon is an ordinary character and a file called `a:b` still completes. The `:` is already a word
/// break — bash's `COMP_WORDBREAKS` — so by here it is the word's *prefix* and the stem is only what
/// follows it.
fn names_another_machine(action: &Action, word: &Word<'_>) -> bool {
    let Action::List(list) = action else {
        return false;
    };
    if !list.iter().any(|entry| entry.trim() == "$hosts") {
        return false;
    }
    let Some(before) = word.prefix.strip_suffix(':') else {
        return false;
    };
    // `:/tmp` names no machine, and `./a:b` and `/a:b` are local files with a colon in the name.
    !before.is_empty() && !before.contains('/')
}

/// Whether the word being typed is a flag still being named.
///
/// **A lone `-` counts.** In the *walk* a finished `-` is an argument — it is how a command is told
/// to read standard input — but a `-` with the cursor after it is somebody asking what the flags
/// are, which is the most common way anybody asks. After a `--`, or under `parsing: disabled`, a
/// leading dash means nothing at all.
fn naming_a_flag(walk: &walk::Walk<'_>, stem: &str) -> bool {
    stem.starts_with('-')
        && walk.node.parsing != Parsing::Disabled
        && !matches!(walk.at, At::Dash(_) | At::FlagValue(_))
}

/// The flag an `=`-broken word is giving a value to.
///
/// Only an `=`, and only a word that began with a dash: `scp host:/pa` breaks on a `:` and names no
/// flag, and `FOO=bar` is an assignment.
fn inline_flag<'a>(walk: &walk::Walk<'a>, word: &Word<'_>) -> Option<&'a OptionSpec> {
    let named = word.prefix.strip_suffix('=')?;
    if !named.starts_with('-') || walk.node.parsing == Parsing::Disabled {
        return None;
    }
    walk.flags_on_offer()
        .find(|opt| opt.matches(named).is_some())
}

/// The action for position `index`: the one declared for it, or the one declared for every other.
fn position<'a>(declared: &'a [Action], any: &'a Action, index: usize) -> &'a Action {
    declared.get(index).unwrap_or(any)
}

/// The word from `at` onwards, as a word of its own.
///
/// `$list(,)` on `a,b,c` completes `c`: the stem has to be *cut*, not merely offset, or a `$files`
/// beside it would look for a directory called `a,b`. `start` moves with it so a candidate is
/// written over the last element alone and the ones before it stay on the line.
///
/// `None` when the word cannot be cut without lying about what will be inserted — a quote or an
/// escape makes the raw text and the stem different lengths, so an offset into one is not an offset
/// into the other.
fn retarget<'a>(word: &Word<'a>, at: usize) -> Option<Word<'a>> {
    if at == 0 {
        return Some(word.clone());
    }
    if word.quote != Quote::None || word.text != word.stem || !word.text.is_char_boundary(at) {
        return None;
    }
    Some(Word {
        start: word.start + at,
        text: &word.text[at..],
        stem: word.stem[at..].to_string(),
        carried: 0,
        ..word.clone()
    })
}

/// One row, quoted the way the word it replaces was.
fn candidate(
    word: &Word<'_>,
    name: &str,
    description: Option<&str>,
    kind: &str,
) -> CompletionCandidate {
    CompletionCandidate {
        display: name.to_string(),
        replacement: quote_replacement(name, word.quote),
        description: description.filter(|d| !d.is_empty()).map(str::to_string),
        kind: Some(kind.to_string()),
        path: None,
        detail: None,
    }
}

#[cfg(test)]
#[path = "spec/tests.rs"]
mod tests;
