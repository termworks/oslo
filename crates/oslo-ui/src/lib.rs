//! Interactive completion, hints, colouring and multi-line input services.

pub mod abbr;
pub mod ask;
/// A headline and a rail of labelled rows — the shape every report the shell prints has.
pub mod block;
pub mod command_index;
pub mod completion;
pub mod dropdown;
pub mod edit;
pub mod editor;
pub mod explore;
pub mod finder;
pub mod frecency_store;
pub mod git;
pub mod highlight;
mod hinting;
/// Colouring a string by naming the colour — `colored`'s shape, over oslo's own `Style`.
pub mod ink;
pub mod keys;
pub mod manager;
pub mod marks;
pub mod matching;
pub mod nav;
pub mod paint;
pub mod pending;
pub mod prompt;
pub mod query;
pub mod recall;
#[cfg(feature = "vista")]
pub mod repair;
/// `on-report` — letting a config draw what the shell was going to draw.
pub mod report;
pub mod row;
pub mod scanner;
pub mod settings;
pub mod shell;
pub mod spec;
pub mod suggest;
pub mod syntax;
pub mod term;
pub mod theme;
pub mod transcript;
pub mod vi;
pub mod words;

pub use command_index::invalidate as invalidate_command_cache;
pub use syntax::{DEFAULT_PS2, InputStatus};
pub use words::{Quote, Word, current_word, extract_current_word};

use dropdown::CompletionCandidate;
use frecency_store::FrecencyStore;
use shell::Shell;
use spec::SpecRegistry;
use std::sync::{Arc, Mutex};

/// Completion kinds worth remembering.
///
/// Filenames are deliberately absent: they are unbounded and mostly one-off, and letting them
/// into the table would drown the command ranking it exists to serve.
const RANKED_KINDS: &[&str] = &["command", "builtin", "subcommand"];

pub struct OsloHelper {
    env: Arc<Mutex<dyn Shell>>,
    spec_registry: SpecRegistry,
    frecency: FrecencyStore,
    /// Whether Tab may take the terminal over to draw the dropdown.
    ///
    /// Off when there is no terminal, which is what makes `complete` callable from a test: the
    /// dropdown would otherwise swallow every ambiguous candidate list and return nothing.
    menu: bool,
    /// Whether the editor itself accumulates continuation lines.
    ///
    /// See [`OsloHelper::set_editor_multiline`].
    editor_multiline: bool,
}

impl OsloHelper {
    /// Build the helper for `env`.
    ///
    /// Two side effects hang off whether `env` belongs to an interactive shell: the dropdown takes
    /// the terminal over, and the frecency table folds in what this profile has run. `$-` is the
    /// signal rather than `isatty`, because `cargo test` inherits a terminal on stdin and a test
    /// must not read the user's store.
    /// Generic so the coercion happens here rather than at every call site: a caller hands over
    /// whatever it already holds and this is the one place that forgets the concrete type.
    pub fn new<S: Shell + 'static>(env: Arc<Mutex<S>>) -> Self {
        let env: Arc<Mutex<dyn Shell>> = env;
        let interactive = env
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .interactive();
        let frecency = if interactive {
            FrecencyStore::from_history()
        } else {
            FrecencyStore::in_memory()
        };
        Self {
            env,
            spec_registry: SpecRegistry::new(),
            frecency,
            menu: interactive,
            editor_multiline: true,
        }
    }

    pub fn set_menu(&mut self, enabled: bool) {
        self.menu = enabled;
    }

    /// Select whether the editor or its caller accumulates incomplete input.
    pub fn set_editor_multiline(&mut self, enabled: bool) {
        self.editor_multiline = enabled;
    }

    /// Whether `buffer` is a complete program, needs another line, or is a syntax error.
    pub fn input_status(&self, buffer: &str) -> InputStatus {
        syntax::classify(buffer)
    }

    /// The prompt to show while reading a continuation line: `$PS2`, or `> `.
    pub fn continuation_prompt(&self) -> String {
        self.env
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .var("PS2")
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_PS2.to_string())
    }

    /// Count every command word in an accepted program toward frecency.
    pub fn record_command_use(&self, line: &str) {
        for name in words::command_words(line) {
            self.frecency.record(&name);
        }
    }

    /// This command's frecency score. Exposed for tests and for ranking outside the helper.
    pub fn frecency_score(&self, name: &str) -> f64 {
        self.frecency.score(name)
    }

    /// Paint a line with the current theme and environment.
    pub fn paint(&self, line: &str) -> String {
        if line.is_empty() {
            return String::new();
        }
        // **A Lua line is painted as Lua.** Everything below reads the line as a command and its
        // arguments, and at a Lua prompt that is not merely unhelpful: `print` is not on `$PATH`
        // and `nil` is not a command, so both came out in the *unknown command* style — red and
        // underlined, the same as a typo. Every valid Lua line looked like an error.
        if prompt::language().is_some_and(|language| language == "lua") {
            let theme = theme::current();
            // Asked of the session, so a name that exists is drawn as what it is. The globals are
            // fetched once per paint rather than once per name — this runs on every keystroke.
            let globals: std::collections::HashMap<String, bool> =
                completion::lua::global_names().into_iter().collect();
            return highlight::lua::paint(line, &theme, theme::depth(), &|name: &str| {
                globals.get(name).copied()
            });
        }
        // **The guard is held for the whole paint**, so the closures below can ask the environment
        // directly. Snapshotting into two `HashSet`s existed only because they each re-took the
        // lock; it cost a `String` and a hash per builtin, function and alias — around 170 of them
        // — on every keystroke, to answer two or three `contains` calls.
        let Ok(env) = self.env.lock() else {
            return line.to_string();
        };
        let path = env.var("PATH").unwrap_or_default().to_string();
        let is_builtin = |name: &str| env.is_builtin(name);
        let is_function = |name: &str| env.is_function(name) || env.alias(name).is_some();
        let ctx = highlight::Context {
            path: &path,
            is_builtin: &is_builtin,
            is_function: &is_function,
            // A line long enough for the syscalls to add up is one nobody is reading the colours
            // of. See `highlight::MAX_PATH_CHECKS`.
            check_paths: line.len() <= 512,
        };
        highlight::paint(line, &ctx)
    }

    /// Whether this line's command is one whose past arguments are worthless to offer back.
    ///
    /// See [`settings::Suggest::skip_history`]. Judged on the first word alone, which is the
    /// command being run; anything a pipeline does further along is a different command with its
    /// own answer, and a ghost only ever continues the line as a whole.
    fn consumes_its_arguments(line: &str, names: &[String]) -> bool {
        let Some(command) = line.split_whitespace().next() else {
            return false;
        };
        // By the name as run, so `/bin/rm` and `rm` are the same command to a person who typed one
        // of them and meant the other.
        let command = command.rsplit('/').next().unwrap_or(command);
        names.iter().any(|name| name == command)
    }

    /// Ask the registered providers, building the context they are told about.
    ///
    /// **The `any()` test comes first and costs an atomic load.** Every shell until somebody
    /// installs a provider takes that branch on every keystroke, and it must not pay for a
    /// mechanism it is not using — building the context alone would mean two lock acquisitions and
    /// a `String` per key.
    fn provider_hint(&self, line: &str, pos: usize) -> Option<String> {
        Self::provider_ctx(line, pos).and_then(|ctx| suggest::ask(&ctx))
    }

    /// The gap-filling pass: providers that were told to answer only when nothing else did.
    fn provider_fill(&self, line: &str, pos: usize) -> Option<String> {
        Self::provider_ctx(line, pos).and_then(|ctx| suggest::ask_fill(&ctx))
    }

    fn provider_ctx(line: &str, pos: usize) -> Option<suggest::Ctx> {
        if !suggest::any() {
            return None;
        }
        Some(suggest::Ctx {
            line: line.to_string(),
            cursor: pos,
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            language: prompt::language().unwrap_or_else(|| "sh".to_string()),
        })
    }

    /// The ghost suggestion for `line`, in this prompt's source order, as plain text.
    ///
    /// Only at the end of the line: a suggestion *continues* what you have typed, so offering one
    /// for a cursor sitting mid-line would be a claim about the wrong position.
    pub fn suggest(&self, line: &str, pos: usize) -> Option<String> {
        if line.is_empty() || pos < line.len() {
            return None;
        }
        // The `suggest` feature, which is a runtime mask over the configured sources rather than a
        // second way to configure them: turning it back on restores whatever `oslo.suggest` said.
        if !oslo_base::feature::on(oslo_base::feature::at::SUGGEST) {
            return None;
        }
        let settings = settings::current();
        let recallable = !Self::consumes_its_arguments(line, &settings.suggest.skip_history);

        // **Each prompt has its own list**, because most sources answer with shell and cannot be
        // typed at a Lua prompt at all. `oslo.suggest.sh_sources` is the shell's, `lua_sources` is
        // Lua's, and the Lua default is `names` alone — an editor offers what exists, not what you
        // typed last week. See `settings::Suggest::lua_sources`.
        let lua = prompt::language().is_some_and(|language| language == "lua");
        let sources = match lua {
            true => &settings.suggest.lua_sources,
            false => &settings.suggest.sh_sources,
        };
        for source in sources {
            let found = match source {
                // oslo's own set, not a flat editor history: `recall` is language-filtered and
                // knows which directory you are standing in, so `cargo run --ex` answers with
                // this project's example.
                settings::Source::History if recallable => recall::suggest(line),
                settings::Source::History => None,
                // **One source, two answers.** "Complete what is being typed" is the same idea at
                // both prompts; what it completes is not. A command name at a shell prompt, a Lua
                // name at a Lua one.
                settings::Source::Completion if lua => hinting::lua_hint(line, pos),
                settings::Source::Completion => self.command_hint(line, pos),
                // **Shell-shaped, so it declines at a Lua prompt** rather than offering something
                // that cannot be written there. Listing it under `lua_sources` is not an error —
                // it is a config asking for a thing with no Lua meaning, and the honest answer is
                // nothing rather than a filename in the middle of a Lua expression.
                settings::Source::Path if !lua => self.path_hint(line, pos),
                settings::Source::Path => None,
                // The model, which knows what usually follows what you have been doing. It
                // answers with a whole line, so what is offered is the remainder — the same shape
                // `recall` returns, since both continue what has been typed rather than replace
                // it. Costs about 4 µs against the hint's 2.3 (`bench/predict.rs`), and nothing
                // at all before the snapshot has loaded.
                //
                // **The name stays readable without the feature**, answering nothing rather than
                // refusing to parse. A config is shared between machines and a build without
                // `predict` should not reject one written for a build with it — an unusable source
                // is skipped exactly like a source that had nothing to say.
                #[cfg(feature = "vista")]
                settings::Source::Prediction if lua => None,
                #[cfg(feature = "vista")]
                settings::Source::Prediction => oslo_base::predict::suggest_here(Some(line), 1)
                    .into_iter()
                    .find(|guess| guess.line.len() > line.len() && guess.line.starts_with(line))
                    .map(|guess| guess.line[line.len()..].to_string()),
                #[cfg(not(feature = "vista"))]
                settings::Source::Prediction => None,
                // Whatever a config or a plugin registered. Asked in the place the config put it,
                // so a plugin's answer beats history only if you said it should.
                settings::Source::Provider => self.provider_hint(line, pos),
            };
            if found.is_some() {
                return found;
            }
        }
        // **Nothing answered, so the gap-fillers get their turn.** A provider set to `fill` has no
        // position in the order — it answers when nobody else did, which is a different question
        // from "answer before history" and cannot be expressed by placing it anywhere in the list.
        self.provider_fill(line, pos)
    }

    /// Paint a ghost suggestion in the autosuggestion colour.
    ///
    /// Separate from the suggestion itself so that what is *inserted* when you accept it is plain
    /// text: escapes in the line would end up in the history and in what runs.
    pub fn paint_hint(&self, hint: &str) -> String {
        let theme = theme::current();
        theme.syntax.autosuggestion.paint(hint, theme::depth())
    }

    /// What the line probably should have said, or nothing when it looks fine.
    ///
    /// **Only in shell**, for the same reason the command hint is: everything it can offer is a
    /// shell command, and proposing one at a Lua prompt would be proposing something that cannot
    /// run in the language being typed.
    #[cfg(feature = "vista")]
    pub fn repair(&self, line: &str) -> Option<String> {
        if prompt::language().is_some_and(|language| language != "sh") {
            return None;
        }
        let env = self.env.lock().unwrap_or_else(|held| held.into_inner());
        let path = env.var("PATH").unwrap_or_default().to_string();
        let has =
            |name: &str| env.is_builtin(name) || env.alias(name).is_some() || env.is_function(name);
        let begins = |stem: &str| {
            env.builtin_names().iter().any(|n| n.starts_with(stem))
                || env.aliases().keys().any(|n| n.starts_with(stem))
                || env.functions().keys().any(|n| n.starts_with(stem))
        };
        // **Vocab belongs here too.** `spelling` asks vocab two of its three questions — is this a
        // name, does a name begin with this — but the candidate list it corrects *towards* was
        // builtins, aliases and functions only. So a typo of an autoloadable function or a
        // structured verb had nothing to be corrected to, while a typo of a builtin did.
        let all = || {
            env.builtin_names()
                .into_iter()
                .chain(env.aliases().keys().cloned())
                .chain(env.functions().keys().cloned())
                .chain(oslo_base::vocab::all().into_iter().map(|(name, _)| name))
                .collect()
        };
        repair::of(
            line,
            &path,
            &repair::Known {
                has: &has,
                begins: &begins,
                all: &all,
            },
        )
    }

    /// Draw the correction that goes after a mistyped line, marking only what changed.
    ///
    /// Two styles, and they are one colour: the arrow and the words that were already right are the
    /// ordinary ghost, and the corrected words are that same colour reversed. See
    /// [`repair::annotate`] for why the bracketed words are the only thing emphasised.
    #[cfg(feature = "vista")]
    pub fn paint_repair(&self, typed: &str, fixed: &str) -> String {
        let theme = theme::current();
        repair::annotate(
            typed,
            fixed,
            &theme.syntax.autosuggestion,
            &theme.syntax.repair,
            theme::depth(),
        )
    }

    /// Complete the word at `pos`, recording an unambiguous answer as an acceptance.
    ///
    /// One candidate is inserted without asking, and that *is* an acceptance — so it feeds the
    /// frecency ranking exactly as choosing from the menu does.
    pub fn complete_word(&self, line: &str, pos: usize) -> (usize, Vec<CompletionCandidate>) {
        let (start, candidates) = self.candidates(line, pos);
        if let [only] = candidates.as_slice() {
            self.record_accepted(only);
        }
        (start, candidates)
    }

    /// Note an accepted candidate for frecency ranking.
    pub fn record_accepted(&self, candidate: &CompletionCandidate) {
        if candidate
            .kind
            .as_deref()
            .is_some_and(|k| RANKED_KINDS.contains(&k))
        {
            self.frecency.record(&candidate.display);
        }
    }
}

impl OsloHelper {}
