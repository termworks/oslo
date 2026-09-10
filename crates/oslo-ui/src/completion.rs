//! Builds completion candidates for Tab.

pub mod lua;
mod paths;
pub(crate) use paths::executable;
pub(crate) use paths::glob_matches_anything;
pub(crate) use paths::takes_only_directories;
pub mod provider;
mod segment;
mod spec;
mod sugar;

use segment::{after_break, brace_segment};

use super::OsloHelper;
use super::command_index::CommandIndex;
use super::dropdown::CompletionCandidate;
use super::matching::{Fuzzed, Match, matchers, matches_ignoring_case};
use super::settings;
use super::words::{Quote, Word, current_word, quote_replacement, unquote};

/// Whether `candidate` starts with what the user typed.
///
/// `oslo.completion.case_sensitive` decides. It defaults to off, so typing `RE` offers `README.md`
/// — which is what a shell that completes filenames on a case-insensitive muscle memory should do.
/// The setting was read from the config and then ignored, so turning it on did nothing at all.
///
/// Case folding is per character rather than by lowercasing the whole string: allocating a String
/// per candidate would mean thousands of allocations per keystroke on a large `$PATH`.
///
/// The flag is a parameter rather than read from the process-global settings here, so the tests
/// can exercise both answers without racing each other — the settings are shared, and the test
/// binary is multi-threaded.
fn matches_prefix(candidate: &str, typed: &str, case_sensitive: bool) -> bool {
    // The pass currently being tried, when the caller is walking the chain. Set around one
    // builder's run rather than threaded through every call site: the builders are recursive and
    // the alternative is an extra argument on a dozen functions that only two of them care about.
    if let Some(matcher) = MATCHER.with(|slot| slot.get()) {
        return matcher.matches(candidate, typed);
    }
    if case_sensitive {
        return candidate.starts_with(typed);
    }
    matches_ignoring_case(candidate, typed)
}

thread_local! {
    static MATCHER: std::cell::Cell<Option<Match>> = const { std::cell::Cell::new(None) };
}

/// Whether the variable being completed was written `${NAME` rather than `$NAME`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Braced {
    Yes,
    No,
}

/// A hook a config installs to complete one command itself.
///
/// Handed the words typed so far and the word being completed; answers the candidates, or `None`
/// to fall through to oslo's own. Registered per command name by `oslo.completion.for_command`.
pub type CommandCompleter =
    std::rc::Rc<dyn Fn(&str, &[&str], &str) -> Option<Vec<(String, Option<String>)>>>;

thread_local! {
    /// Thread-local rather than global: the hook calls Lua, which is not `Send`, and only the
    /// editor's thread completes anything.
    static FOR_COMMAND: std::cell::RefCell<Option<CommandCompleter>> =
        const { std::cell::RefCell::new(None) };
}

/// Install the hook `oslo.completion.for_command` describes. `None` removes it.
pub fn set_command_completer(hook: Option<CommandCompleter>) {
    FOR_COMMAND.with(|slot| *slot.borrow_mut() = hook);
}

/// What columns can be named at a point in a half-typed line.
///
/// Handed the whole line and the cursor, because the answer depends on what is **upstream of the
/// pipe**, which no per-command hook can see. Answers:
///
/// * `None` — not a column position; the menu falls through to filenames, as it must for `ls <Tab>`.
/// * `Some([…])` — these columns.
/// * `Some([])` — a column position whose columns are not knowable. The menu offers nothing and
///   **does not fall through**: a filename where a column belongs is the wrong nothing.
///
/// The shell installs it, because the registry that knows the answer lives there and the dependency
/// runs shell → ui. The same inversion as [`set_command_completer`], for the same reason.
pub type ColumnSource = std::rc::Rc<dyn Fn(&str, usize) -> Option<ColumnsHere>>;

/// The columns that belong at a point in a line, and how much of the line they replace.
///
/// **`replace_from` is why this is not just a list.** A bare operand is replaced whole —
/// `sort-by mod` becomes `sort-by modified`. A name being typed *inside a filter* is not:
/// `where 'size > 1 and nam` must replace the `nam` and leave the rest of the expression exactly
/// where it is. Splicing at the word's own start there would overwrite the whole quoted expression
/// with one column name — the same class of mistake [`Word::carried`] exists to prevent for a brace
/// expansion.
pub struct ColumnsHere {
    pub columns: Vec<String>,
    /// Byte offset in the line where a chosen candidate is written.
    pub replace_from: usize,
}

thread_local! {
    static COLUMNS_AT: std::cell::RefCell<Option<ColumnSource>> =
        const { std::cell::RefCell::new(None) };
}

/// Install the column source. `None` removes it.
pub fn set_column_source(hook: Option<ColumnSource>) {
    COLUMNS_AT.with(|slot| *slot.borrow_mut() = hook);
}

/// Ask the installed source what columns belong at `pos`.
fn columns_at(line: &str, pos: usize) -> Option<ColumnsHere> {
    COLUMNS_AT.with(|slot| slot.borrow().as_ref().map(|hook| hook(line, pos)))?
}

/// Whether the prompt is reading Lua rather than shell.
///
/// Unknown counts as shell: a prompt that has not said is a plain one, and the shell answers are
/// the ones that were always given.
fn is_lua() -> bool {
    super::prompt::language().is_some_and(|language| language == "lua")
}

/// Ask the config to complete this command, if it wants to.
fn config_candidates(
    command: &str,
    prior: &[&str],
    current: &str,
    quote: Quote,
) -> Option<Vec<CompletionCandidate>> {
    // Cloned out of the cell before calling: the hook runs Lua, and Lua can complete another word,
    // which would come back through here and panic on the outstanding borrow.
    let hook = FOR_COMMAND.with(|slot| slot.borrow().clone())?;
    let answers = hook(command, prior, current)?;
    Some(
        answers
            .into_iter()
            .map(|(display, description)| CompletionCandidate {
                // Quoted like every other candidate: `start` points at the *opening quote*, so what
                // is written back has to re-supply it. Inserted raw, `git commit -m "fi<Tab>`
                // taking `fix: broken pipe` left `git commit -m fix: broken pipe` — the quote gone
                // and two stray operands in its place.
                replacement: quote_replacement(&display, quote),
                display,
                description,
                // **`config`, rather than a kind the hook did not report.** This was `None`, which
                // reads as "no kind is known" and is honest — but `oslo.completion.sources` filters
                // on the kind a candidate carries, and `None` fails that test unconditionally: a
                // config with both `for_command` and `sh_sources` set got an *empty* menu, because
                // the hook's candidates replaced oslo's own and the filter then dropped all of
                // them. `provider.rs` names this hole and closes it only for providers.
                //
                // It says where the candidate came from rather than what it is, which is the one
                // true thing available here — the earlier `option` was a guess, and it labelled a
                // config's branches and hostnames as options in the column read to tell them apart.
                kind: Some("config".to_string()),
                path: None,
                detail: None,
            })
            .collect(),
    )
}

impl OsloHelper {
    /// Return context-quoted candidates and their replacement byte offset.
    pub fn candidates(&self, line: &str, pos: usize) -> (usize, Vec<CompletionCandidate>) {
        // **A Lua line is completed as Lua, and only as Lua.** Everything below answers a shell
        // question — a command, a path, a `$variable`, an `@mark` — and at a Lua prompt those are
        // not merely unhelpful but wrong: `$HO` completed to `$HOME`, which is a lexer error in
        // the language being typed. So this returns whatever the Lua side answers, including
        // nothing, rather than falling through to answers that cannot be written here.
        if is_lua() {
            return lua::candidates(line, pos).unwrap_or((pos, Vec::new()));
        }

        // oslo's own shorthands first: both look like ordinary words and neither completes like
        // one, so the retarget has to happen before anything reads `stem`.
        // Where a chosen candidate is written, when something narrower than the word decides — a
        // column name being typed inside a filter replaces the identifier, not the expression.
        let mut splice: Option<usize> = None;
        let sugared = sugar::at_segment(sugar::equals_segment(sugar::escaped_segment(
            current_word(line, pos),
        )));
        let word = brace_segment(after_break(sugared));
        let mut out = Vec::new();
        // Set when a fuzzy pass answered, and read by the final sort — see there.
        let mut fuzzed: Option<Fuzzed> = None;

        if let Some(prefix) = word.stem.strip_prefix('$') {
            // `${NAME` is the same question with a brace on it, and the answer has to carry the
            // closing one — `${HO` completed to `$HOME` would leave an expansion that never ends.
            match prefix.strip_prefix('{') {
                Some(braced) => self.variable_candidates(braced, Braced::Yes, word.quote, &mut out),
                None => self.variable_candidates(prefix, Braced::No, word.quote, &mut out),
            }
        } else if word.stem.starts_with('@')
            && word.quote == Quote::None
            && !word.stem.contains('/')
            && !word.command_position
        {
            // A name still being typed. `@work/` has already been retargeted at the directory it
            // stands for, so only the bare name reaches here.
            //
            // **Not at the start of a line**, where `@name` does not expand either: that position is
            // reserved, and offering completions for it would promise something the shell will not
            // then do.
            //
            // **And not inside quotes**, for the same reason and a worse symptom. A quoted `@name`
            // is a literal to the expander, so completing it promises an expansion that will not
            // happen — and `named_dir_candidates` writes back as if the word were unquoted, so
            // `ls "@pr<Tab>` came back as `ls @proj/` with the opening quote deleted. Every sibling
            // builder in `sugar.rs` declines a quoted word; this branch was the one that did not.
            self.named_dir_candidates(word.stem.as_str(), &mut out);
            if out.is_empty() {
                self.path_candidates(&word, &mut out);
            }
        } else if word.command_position && word.stem.contains('/') {
            // **A command with a `/` in it is a path, not a name.** Nothing on `$PATH` is reached by
            // one, so the name builders below can never answer — and until this branch existed
            // `./bui<Tab>`, `/usr/bin/gre<Tab>` and `~/bin/my<Tab>` did nothing at all, while the
            // highlighter happily coloured the same word as a real command once it was typed out.
            self.path_candidates(&word, &mut out);
        } else if word.command_position {
            // **Only in shell.** A command name is a shell answer: a builtin, an alias, a function,
            // something on `$PATH`. At a Lua prompt none of them can run, and offering them is the
            // same mistake the ghost suggestion used to make — `l` answered with `ls` in a language
            // that has no `ls`.
            //
            // Paths, variables and config-supplied candidates are left alone: those are still
            // meaningful in Lua, and this is the one source that is not.
            if super::prompt::language().is_none_or(|language| language == "sh") {
                // Each way of matching in turn, stopping at the first that finds anything. An
                // exact match must never arrive diluted with looser ones.
                for matcher in matchers(settings::current().completion.fuzzy) {
                    self.command_candidates_with(&word, matcher, &mut out);
                    if !out.is_empty() {
                        // A fuzzy pass has no useful order of its own — every candidate that
                        // matched at all matched, so without this the list arrives in whatever
                        // order `$PATH` happened to be walked in. The pattern is carried to the
                        // final sort rather than applied here: sorting twice means the second
                        // sort wins, and it did.
                        if let Match::Fuzzy(fuzzy) = matcher {
                            fuzzed = Some(Fuzzed::new(word.stem.as_str(), fuzzy));
                        }
                        break;
                    }
                    if self.case_sensitive() {
                        // The config asked for exactness; the looser passes are not wanted.
                        break;
                    }
                }
            }
        } else if let Some(from_config) =
            word.prior_words
                .first()
                .map(|w| unquote(w))
                .and_then(|cmd| {
                    config_candidates(&cmd, &word.prior_words, word.stem.as_str(), word.quote)
                })
        {
            // A config that answers for this command replaces oslo's own candidates rather than
            // adding to them: `oslo.completion.for_command("git", …)` means the config knows git
            // better than the built-in spec does, and mixing the two would offer both.
            out = from_config;
        } else if let Some(here) = columns_at(line, pos) {
            // **A column name, and the most common thing typed at a structured prompt.** The stream
            // upstream of the pipe decides what there is; see `oslo_shell::data::complete`. An empty
            // answer is still an answer — the columns are not knowable, and a filename is not a
            // column — so this branch owns the position either way.
            //
            // What has been typed since the splice point is the prefix to match. For a bare operand
            // that is the whole word; inside a filter it is the identifier alone, which is why the
            // source says where to splice rather than only what to offer.
            splice = Some(here.replace_from);
            let typed = line.get(here.replace_from..pos).unwrap_or_default();
            out.extend(
                here.columns
                    .into_iter()
                    .filter(|column| column.starts_with(typed))
                    .map(|column| {
                        let mut candidate = CompletionCandidate::new(column.clone(), column, None);
                        // Its own kind, so `oslo.completion.sh_sources` can order or drop it the
                        // way it does every other source.
                        candidate.kind = Some("column".to_string());
                        candidate
                    }),
            );
        } else {
            // **A position a spec declared and that came back empty is an answer.** Falling through
            // to the filenames there is the wrong nothing: `deploy --env <Tab>` offers what the
            // flag accepts, and the directory listing is not it.
            let owned = self.spec_candidates(&word, &mut out);
            if out.is_empty() && !owned {
                self.path_candidates(&word, &mut out);
            }
        }

        // What a provider offers, merged in rather than replacing anything. Collected before the
        // filter and the sort below so a provider's candidates are filtered by kind and ranked
        // against oslo's own — which is the difference between adding to a menu and owning it.
        let boosts = self.provider_candidates(&word, &mut out);

        // Frecency first, name second. Without the first key this is alphabetical, which is how
        // `exit` came to suggest `exitsnoop-bpfcc`.
        // `oslo.completion.sources`: drop the kinds the config did not ask for. Applied after the
        // builders rather than inside them, so a kind is filtered by the name it already carries
        // and adding a new kind needs no change here.
        if let Some(wanted) = &crate::settings::current().completion.sh_sources {
            out.retain(|c| {
                c.kind
                    .as_deref()
                    .is_some_and(|k| wanted.iter().any(|w| w == k))
            });
        }

        // `oslo.completion.sort`. Frecency first, name second — without the first key this is
        // alphabetical, which is how `exit` came to suggest `exitsnoop-bpfcc`. A config that
        // prefers a predictable order can ask for `alpha` and get name only.
        //
        // **How well it matched outranks both**, when a fuzzy pass is what found these. Every
        // candidate in a fuzzy set matched, so frecency cannot separate them — unrun commands all
        // score zero and the list came back in plain alphabetical order, which is how `cnr` offered
        // `airscan-discover` above `cargo-nextest-runner` and why `gco` never reached
        // `git checkout` first. Frecency still orders candidates that matched equally well.
        let by_name = crate::settings::current().completion.sort == crate::settings::Sort::Alpha;
        out.sort_by(|a, b| {
            if by_name {
                return a.display.cmp(&b.display);
            }
            if let Some(pattern) = &fuzzed {
                let fa = pattern.score(&a.display).unwrap_or(i32::MIN);
                let fb = pattern.score(&b.display).unwrap_or(i32::MIN);
                if fa != fb {
                    return fb.cmp(&fa);
                }
            }
            let sa =
                self.frecency.score(&a.display) + boosts.get(&a.display).copied().unwrap_or(0.0);
            let sb =
                self.frecency.score(&b.display) + boosts.get(&b.display).copied().unwrap_or(0.0);
            sb.partial_cmp(&sa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.display.cmp(&b.display))
        });
        out.dedup_by(|a, b| a.replacement == b.replacement);

        (splice.unwrap_or(word.start), out)
    }

    /// `oslo.completion.case_sensitive`, read once per completion rather than per candidate.
    fn case_sensitive(&self) -> bool {
        crate::settings::current().completion.case_sensitive
    }

    fn variable_candidates(
        &self,
        prefix: &str,
        braced: Braced,
        quote: Quote,
        out: &mut Vec<CompletionCandidate>,
    ) {
        let env = self.env.lock().unwrap_or_else(|held| held.into_inner());
        for name in env.vars().keys() {
            if matches_prefix(name, prefix, self.case_sensitive()) {
                let value = match braced {
                    Braced::Yes => format!("${{{name}}}"),
                    Braced::No => format!("${name}"),
                };
                out.push(CompletionCandidate {
                    display: value.clone(),
                    // `$` survives quoting only outside double quotes; inside them it would be
                    // escaped and stop expanding, so a variable is always offered unquoted.
                    replacement: if quote == Quote::None {
                        value
                    } else {
                        quote_replacement(&value, quote)
                    },
                    // As above: the ` variable ` badge already says this.
                    description: None,
                    kind: Some("variable".to_string()),
                    path: None,
                    detail: None,
                });
            }
        }
    }

    fn command_candidates(&self, word: &Word<'_>, out: &mut Vec<CompletionCandidate>) {
        let stem = word.stem.as_str();
        // Kind and detail are decided here, where the environment is already locked. An alias is
        // its own kind rather than a `builtin`: they behave differently, and lumping them meant
        // the badge told you `builtin` about something you had defined yourself a minute earlier.
        let (path, shell_names) = {
            let env = self.env.lock().unwrap_or_else(|held| held.into_inner());
            let mut names: Vec<(String, &str, Option<String>)> = Vec::new();
            for b in env.builtin_names() {
                if matches_prefix(&b, stem, self.case_sensitive()) {
                    names.push((b.to_string(), "builtin", None));
                }
            }
            for (name, target) in env.aliases() {
                if matches_prefix(name, stem, self.case_sensitive()) {
                    // What it expands to travels with it: that is the one thing about an alias
                    // its name does not tell you, and it is why aliases keep a second column
                    // where a directory does not.
                    names.push((name.clone(), "alias", Some(target.clone())));
                }
            }
            for f in env.functions().keys() {
                if matches_prefix(f, stem, self.case_sensitive()) {
                    names.push((f.clone(), "function", None));
                }
            }
            (env.var("PATH").unwrap_or_default().to_string(), names)
        };

        for (name, kind, detail) in shell_names {
            let mut candidate = self.command_candidate(word, name, kind);
            candidate.detail = detail;
            out.push(candidate);
        }
        // The structured verbs and the tools a config registered. Not on `$PATH` and not builtins,
        // so nothing else here can offer them — `whe<Tab>` found nothing at all.
        for (name, kind) in oslo_base::vocab::all() {
            if matches_prefix(&name, stem, self.case_sensitive()) {
                out.push(self.command_candidate(word, name, kind));
            }
        }
        // The index is shared, not rebuilt: this used to `read_dir` all of `$PATH` per keystroke.
        for name in CommandIndex::executables(&path).iter() {
            if matches_prefix(name, stem, self.case_sensitive()) {
                out.push(self.command_candidate(word, name.clone(), "command"));
            }
        }
    }

    /// **No description here.** Resolving one means asking the spec loader, which tries three
    /// directories by two extensions and fully parses any hit — and this runs for *every* matching
    /// `$PATH` name, thousands of them on the first bare Tab, to fill a column that only the dozen
    /// visible rows ever show and that `oslo.completion.descriptions = false` removes outright.
    ///
    /// The renderer asks instead, for the rows it is actually drawing, the same way it already
    /// resolves a file's size and age. See `dropdown::columns::with_descriptions`.
    fn command_candidate(&self, word: &Word<'_>, name: String, kind: &str) -> CompletionCandidate {
        CompletionCandidate {
            display: name.clone(),
            replacement: quote_replacement(&name, word.quote),
            description: None,
            kind: Some(kind.to_string()),
            path: None,
            detail: None,
        }
    }

    /// Command candidates found one particular way. See [`Match`].
    fn command_candidates_with(
        &self,
        word: &Word<'_>,
        matcher: Match,
        out: &mut Vec<CompletionCandidate>,
    ) {
        MATCHER.with(|slot| slot.set(Some(matcher)));
        self.command_candidates(word, out);
        MATCHER.with(|slot| slot.set(None));
    }

    /// Merge in what the registered providers offer, answering each one's score offset by display.
    ///
    /// **Before the kind filter and the sort**, so a provider's candidates are filtered by the kind
    /// it declared and ranked against oslo's own rather than being stapled to one end of the list.
    fn provider_candidates(
        &self,
        word: &Word<'_>,
        out: &mut Vec<CompletionCandidate>,
    ) -> std::collections::HashMap<String, f64> {
        let mut boosts = std::collections::HashMap::new();
        if !provider::any() {
            return boosts;
        }
        let Some(primary) = word.prior_words.first() else {
            return boosts;
        };
        let command = self.resolve_head(&unquote(primary));
        let ctx = provider::Ctx {
            command,
            words: word.prior_words.iter().map(|w| unquote(w)).collect(),
            current: word.stem.as_str().to_string(),
            // **The count of words already typed**, which is what "1 for the first word after the
            // command" means: `git <Tab>` has one prior word and is argument 1, `git commit <Tab>`
            // has two and is argument 2. Subtracting one and clamping made both of them 1, so the
            // documented `if ctx.arg == 2 then return branches() end` never fired and `== 1` fired
            // twice. Non-empty here — the caller returns early on no first word — so nothing needs
            // clamping.
            arg: word.prior_words.len(),
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        };
        for (candidate, offset) in provider::offers(&ctx) {
            // Only what continues the word being typed. A menu that offered everything regardless
            // of what you have already typed would be a list, not a completion.
            if !matches_prefix(
                &candidate.display,
                ctx.current.as_str(),
                self.case_sensitive(),
            ) {
                continue;
            }
            if offset != 0.0 {
                boosts.insert(candidate.display.clone(), offset);
            }
            // Quoted here rather than in `provider::offers`, which has no `Word` to ask: `start`
            // points at the opening quote, so a replacement written back has to re-supply it. Raw,
            // an offer accepted inside `"…` deleted the quote it was typed in.
            out.push(CompletionCandidate {
                replacement: quote_replacement(&candidate.display, word.quote),
                ..candidate
            });
        }
        boosts
    }
}

#[cfg(test)]
#[path = "completion/case_tests.rs"]
mod case_tests;
