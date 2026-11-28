//! Alias substitution, done where POSIX puts it: on the source text, before it is parsed.
//!
//! oslo used to substitute aliases at *execution* time, replacing a simple command's first word
//! with the alias body. That is enough for `alias ll='ls -la'` and wrong for everything else,
//! because an alias body is not a list of arguments — it is **source text**, and it is allowed to
//! contain anything. `alias forever='while :; do'` is a real idiom (modernish's `mktemp` module
//! opens its retry loop with it); expanding it after parsing cannot work, because by then the
//! `done` at the other end has already been a syntax error.
//!
//! ## What decides a substitution
//!
//! A word is substituted when it is in *command position* — the first word of a simple command —
//! and when it is a plain name. Three further rules, each of which real scripts depend on:
//!
//! * **Chaining.** The replacement is rescanned, so `alias ll='ls -la'` over `alias ls='echo LS'`
//!   yields `echo LS -la`. A name already being expanded is not expanded again, which is what
//!   makes the near-universal `alias ls='ls -F'` terminate.
//! * **A trailing blank.** If an alias body ends with a blank, the *next* word is a candidate too.
//!   That is how `alias sudo='sudo '` makes `sudo ll` work.
//! * **Definitions in the text itself.** A script may define an alias and use it further down, and
//!   bash allows that because it parses one command at a time. oslo parses a whole unit at once,
//!   so this scanner reads `alias name=value` commands as it walks and honours them from the
//!   *following line* onward — which is exactly where bash starts honouring them, and why
//!   `alias x=y; x` on one line finds no `x` in either shell.
//!
//! Quoted text, comments and here-document bodies are copied through untouched. The heredoc rule
//! is not a nicety: `config.guess` writes a C program through `<<EOF`, and a body is data.

mod definitions;
mod scan;
/// When a verb name outranks an alias of the same name.
mod verbs;
use verbs::a_verb_follows;

use oslo_base::nesting::heredoc_delimiters;
use scan::{Balance, Quote, is_assignment, is_function_definition, is_plain_name, word_end};
use std::borrow::Cow;
use std::collections::HashMap;

/// How deep a chain of aliases may go before this gives up and emits the text as it stands.
///
/// The active-name set already makes a cycle impossible; this only bounds a pathological chain of
/// distinct names, and 16 is far past anything a person writes.
const MAX_DEPTH: usize = 16;

/// Reserved words after which a command may begin, so the next word is in command position.
///
/// `case` and `for` are deliberately absent: what follows them is a *word*, not a command, and
/// substituting there would rewrite the thing being matched on.
const INTRODUCERS: &[&str] = &[
    "if", "then", "else", "elif", "while", "until", "do", "!", "{", "}", "(", ")", "&&", "||", ";",
    ";;", "|", "&",
];

/// Where a `case` has got to. Its patterns are *words*, not commands, so nothing between `in`
/// and the `)` that opens an arm may be substituted — and a newline in there does not start a
/// command either, which is what first rewrote `ll)` into `ls -la)`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    AwaitingIn,
    Patterns,
    Body,
}

/// Substitute aliases throughout `source`, as an interactive prompt reads it.
///
/// `lookup` answers what the *environment* knows; definitions the text makes for itself are found
/// by this scanner and take effect from the line after they appear. `any_defined` says whether the
/// environment holds any alias at all, which is what makes the early-out below possible.
pub fn substitute<'a>(
    source: &'a str,
    any_defined: bool,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Cow<'a, str> {
    scan(source, true, true, any_defined, lookup)
}

/// [`substitute`], starting with `expand_aliases` in the state `expanding` says and following the
/// literal `shopt -s expand_aliases` / `shopt -u expand_aliases` the text runs, from the next line.
/// A verb outranks an alias only when the shell is interactive.
pub fn substitute_while<'a>(
    source: &'a str,
    expanding: bool,
    any_defined: bool,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Cow<'a, str> {
    let verbs_first = crate::exec::pipeline::is_interactive();
    scan(source, expanding, verbs_first, any_defined, lookup)
}

fn scan<'a>(
    source: &'a str,
    expanding: bool,
    verbs_first: bool,
    any_defined: bool,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Cow<'a, str> {
    // Almost every program contains no alias at all, and the scan is not free. A source with no
    // `alias` command in it can only be affected by an alias the environment already holds, so
    // when there are none of those either there is nothing to do — and returning the source
    // borrowed skips the full copy the scanner would otherwise make of every line typed.
    if !any_defined && !source.contains("alias") {
        return Cow::Borrowed(source);
    }
    if !expanding && !source.contains("expand_aliases") {
        return Cow::Borrowed(source);
    }
    let mut scanner = Scanner {
        expanding,
        verbs_first,
        partial: None,
        lookup,
        defined: HashMap::new(),
        out: String::with_capacity(source.len()),
        quote: Quote::None,
        escaped: false,
        cmd_pos: true,
        after_pipe: false,
        check_next_word: false,
        after_assignment: false,
        line: 0,
        active: Vec::new(),
        pending: None,
        cases: Vec::new(),
        word_list: false,
    };
    scanner.run(source);
    Cow::Owned(scanner.out)
}

struct Scanner<'a> {
    /// Whether `expand_aliases` is on for the line being scanned.
    expanding: bool,
    /// Whether a structured verb in a pipeline outranks an alias of the same name.
    verbs_first: bool,
    /// The lines of a command whose quoted text has not closed yet, for its definitions.
    partial: Option<String>,
    lookup: &'a dyn Fn(&str) -> Option<String>,
    /// Aliases the text defines, and the line each was defined on.
    defined: HashMap<String, (usize, String)>,
    out: String,
    quote: Quote,
    escaped: bool,
    /// Whether the next word begins a command.
    cmd_pos: bool,
    /// Whether the command position we are in was opened by a single `|` rather than by `;`, a
    /// newline, `&&` or anything else. Only there is a structured verb reachable.
    after_pipe: bool,
    /// Set when the alias just substituted ended with a blank.
    check_next_word: bool,
    /// Whether the word just emitted was an assignment prefix (`name=…`).
    after_assignment: bool,
    line: usize,
    /// The names currently being expanded, innermost last.
    active: Vec<String>,
    /// A `$( … )`, `${ … }` or backquoted run being copied through, possibly across lines.
    pending: Option<Balance>,
    /// The `case` constructs currently open, innermost last.
    cases: Vec<Case>,
    /// Inside a `for`/`select` word list, which runs until `do`.
    word_list: bool,
}

impl Scanner<'_> {
    /// Walk the source a line at a time, copying here-document bodies through untouched.
    fn run(&mut self, source: &str) {
        let lines: Vec<&str> = source.split('\n').collect();
        let mut index = 0;
        while index < lines.len() {
            let line = lines[index];
            self.line = index;

            // Computed before the line is rewritten, and only outside quotes: inside a multi-line
            // string a `<<` is text. Same approximation the nesting scan makes.
            let heredocs = if self.quote == Quote::None && self.pending.is_none() {
                heredoc_delimiters(line)
            } else {
                Vec::new()
            };

            self.feed(line);
            if index + 1 < lines.len() {
                self.out.push('\n');
            }
            // A newline separates commands, so the next line starts one — unless it is inside a
            // quoted string or continued by a backslash, where it is just a character. Without
            // this, `cat <<EOF` left `cmd_pos` false and the first command *after* the heredoc
            // was never considered for substitution.
            if self.quote == Quote::None && !self.escaped && self.pending.is_none() {
                // A `for` list ends at the newline; a `case` pattern list does not.
                self.word_list = false;
                if !self.in_case_patterns() {
                    self.cmd_pos = true;
                    // `cmd |` continued onto the next line is still one pipeline, so the newline
                    // only ends the stage when it is not itself following a pipe.
                    self.after_pipe = self.ends_with_pipe();
                    self.check_next_word = false;
                }
            }
            // A line's alias definitions take effect on the next line, never on this one. A
            // quoted value that runs on is gathered until its quote closes.
            if self.pending.is_none() {
                let mut text = self.partial.take().unwrap_or_default();
                text.push_str(line);
                if scan::leaves_quote_open(&text) {
                    text.push('\n');
                    self.partial = Some(text);
                } else {
                    self.record_definitions(&text);
                }
            }
            index += 1;

            for (delimiter, strip_tabs) in heredocs {
                while index < lines.len() {
                    let body = lines[index];
                    self.out.push_str(body);
                    if index + 1 < lines.len() {
                        self.out.push('\n');
                    }
                    index += 1;
                    let candidate = if strip_tabs {
                        body.trim_start_matches('\t')
                    } else {
                        body
                    };
                    if candidate == delimiter {
                        break;
                    }
                }
                self.cmd_pos = true;
                self.after_pipe = false;
            }
        }
    }

    /// Scan one stretch of text — a source line, or an alias body being rescanned.
    fn feed(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        // A construct left open by the previous line carries on here, still copied verbatim.
        if let Some(mut balance) = self.pending.take() {
            // A new line begins at a word boundary, so a `#` on it can start a comment.
            balance.start_line();
            match balance.consume(&mut self.out, &chars, 0) {
                Some(next) => i = next,
                None => {
                    self.pending = Some(balance);
                    return;
                }
            }
        }
        while i < chars.len() {
            let c = chars[i];

            if self.escaped {
                self.out.push(c);
                self.escaped = false;
                i += 1;
                continue;
            }
            match self.quote {
                Quote::Single => {
                    self.out.push(c);
                    if c == '\'' {
                        self.quote = Quote::None;
                    }
                    i += 1;
                    continue;
                }
                Quote::Double => {
                    self.out.push(c);
                    match c {
                        '\\' => self.escaped = true,
                        '"' => self.quote = Quote::None,
                        _ => {}
                    }
                    i += 1;
                    continue;
                }
                Quote::None => {}
            }

            match c {
                '\\' => {
                    self.out.push(c);
                    self.escaped = true;
                    i += 1;
                }
                '\'' => {
                    self.out.push(c);
                    self.quote = Quote::Single;
                    i += 1;
                }
                '"' => {
                    self.out.push(c);
                    self.quote = Quote::Double;
                    i += 1;
                }
                // The rest of the line is a comment. Only reachable at the start of a word: a `#`
                // inside one is consumed by `word_end` along with the rest of it.
                '#' => {
                    self.out.extend(&chars[i..]);
                    return;
                }
                ' ' | '\t' => {
                    self.out.push(c);
                    i += 1;
                }
                ';' => {
                    // `;;` ends a `case` arm and the next thing is another pattern, not a command.
                    if chars.get(i + 1) == Some(&';') {
                        self.out.push_str(";;");
                        i += 2;
                        if self.cases.last() == Some(&Case::Body) {
                            *self.cases.last_mut().expect("checked") = Case::Patterns;
                        }
                        self.cmd_pos = false;
                    } else {
                        self.out.push(c);
                        i += 1;
                        // A `for`/`select` word list ends here, so what follows is the `do` — a
                        // command position. modernish writes `LOOP for i in 1 to 10; DO … DONE`,
                        // and treating the list as still open left `DO` unexpanded, which then
                        // left the list open for the rest of the file.
                        self.word_list = false;
                        self.cmd_pos = !self.in_case_patterns();
                    }
                    self.check_next_word = false;
                }
                '(' => {
                    // `(( … ))` in command position is an arithmetic command, not two subshells.
                    if self.cmd_pos && chars.get(i + 1) == Some(&'(') {
                        match self.copy_run(&chars, i, '(', ')') {
                            Some(after) => i = after,
                            None => return,
                        }
                        self.cmd_pos = false;
                        self.check_next_word = false;
                    } else {
                        self.out.push(c);
                        i += 1;
                        // A `(` inside a pattern list opens a `case` arm's pattern.
                        self.cmd_pos = !self.in_word_context();
                        self.check_next_word = false;
                    }
                }
                ')' => {
                    self.out.push(c);
                    i += 1;
                    // The `)` that closes a pattern list opens the arm's body.
                    if self.cases.last() == Some(&Case::Patterns) {
                        *self.cases.last_mut().expect("checked") = Case::Body;
                    }
                    self.cmd_pos = true;
                    self.after_pipe = false;
                    self.check_next_word = false;
                }
                // Everything `$` opens is copied through untouched, for two different reasons.
                //
                // `$(( … ))` is arithmetic and `${ … }` is a parameter expansion: neither holds
                // commands, and scanning into them rewrote `$(( n + 1 ))` into
                // `$(( echo BAD + 1 ))` for anyone with an alias called `n`.
                //
                // `$( … )` *is* shell text, but it is not this pass's to rewrite: the body is
                // kept as source in the AST and parsed — through this same pass — when the
                // substitution runs. Substituting here as well applied every alias **twice**, so
                // modernish's `alias let='let --'` turned `let "(i+=1)<4"` into `let -- -- "…"`
                // and every arithmetic test in it died. Backticks are the same story.
                '$' => {
                    if let Some(&next) = chars.get(i + 1)
                        && (next == '(' || next == '{')
                    {
                        self.out.push('$');
                        let close = if next == '(' { ')' } else { '}' };
                        match self.copy_run(&chars, i + 1, next, close) {
                            Some(after) => i = after,
                            None => return,
                        }
                        // `LC_ALL=$(locale) cmd` still has `cmd` as its command word, so an
                        // assignment prefix keeps the position across the expansion glued to it.
                        self.cmd_pos = self.after_assignment;
                        self.check_next_word = false;
                    } else {
                        // Part of an ordinary word: `$foo`, `$1`, a bare `$`.
                        let end = word_end(&chars, i);
                        self.out.extend(&chars[i..end.max(i + 1)]);
                        i = end.max(i + 1);
                        self.cmd_pos = false;
                        self.check_next_word = false;
                    }
                }
                // A backquoted command is re-parsed when it runs, exactly like `$( … )`.
                '`' => {
                    self.out.push(c);
                    i += 1;
                    while i < chars.len() {
                        let c = chars[i];
                        self.out.push(c);
                        i += 1;
                        if c == '\\' && i < chars.len() {
                            self.out.push(chars[i]);
                            i += 1;
                        } else if c == '`' {
                            break;
                        }
                    }
                    self.cmd_pos = false;
                    self.check_next_word = false;
                }
                '&' | '|' | '\n' => {
                    self.out.push(c);
                    self.word_list = false;
                    self.cmd_pos = !self.in_case_patterns();
                    // A single `|` opens a stage that has an upstream. `||` reaches here twice, and
                    // the second one takes it back: what follows an or-list is a fresh command with
                    // nothing piped into it.
                    self.after_pipe = self.ends_with_pipe();
                    self.check_next_word = false;
                    i += 1;
                }
                // A redirection's operand is a filename, not a command.
                '<' | '>' => {
                    self.out.push(c);
                    self.cmd_pos = false;
                    self.check_next_word = false;
                    i += 1;
                }
                _ => {
                    let end = word_end(&chars, i);
                    let word: String = chars[i..end].iter().collect();
                    i = end;
                    // A word that stopped at an expansion is only part of a word: `ll${x}` is not
                    // the alias `ll`, and substituting it would rewrite half a word.
                    let partial = chars.get(end) == Some(&'$');
                    if partial || !self.try_substitute(&word, &chars, i) {
                        self.out.push_str(&word);
                        self.after_word(&word);
                    }
                }
            }
        }
    }

    /// Substitute `word` if it names an alias that may be expanded here. Returns whether it did.
    fn try_substitute(&mut self, word: &str, chars: &[char], after: usize) -> bool {
        if !(self.cmd_pos || self.check_next_word) {
            return false;
        }
        if self.active.len() >= MAX_DEPTH || !is_plain_name(word) {
            return false;
        }
        // `name()` is a function definition, and its name is not a command being run.
        if is_function_definition(chars, after) {
            return false;
        }
        if self.active.iter().any(|a| a == word) {
            return false;
        }
        // **A structured verb inside a pipeline is the verb rather than the alias.**
        //
        // `alias df='dfc …'` and `alias get='sudo sysget'` are ordinary and useful, and they used to
        // make `df | where …` and `ls | get name` impossible: the alias expands before the pipeline
        // is planned, so the verb was gone before anything could carry rows to it — and the error
        // named a stage further along, because the aliased word had already stopped existing.
        //
        // **A pipeline, not merely an upstream.** `get` is only ever reachable after a pipe, but
        // `df` is a *producer* and belongs at the head of one, so "has something before it" would
        // have left `df | where …` broken. Being in a pipeline at all is the signal, which is also
        // the rule the rest of the shell already follows: a bare `df` is the external command, and
        // structure is what you get by piping it somewhere.
        //
        // So the two readings never overlap, and only a name oslo invented is affected. Only at an
        // interactive prompt: a script that turns on `expand_aliases` gets bash's expansion.
        if self.verbs_first
            && oslo_base::vocab::kind_of(word) == Some("verb")
            && (self.after_pipe || a_verb_follows(chars, after))
        {
            return false;
        }
        let Some(body) = self.body_for(word) else {
            return false;
        };

        self.cmd_pos = true;
        self.active.push(word.to_string());
        // Rescanned rather than copied, so that a chain of aliases resolves and the state the body
        // leaves behind — `while :; do` ends in command position — carries into what follows it.
        self.feed(&body);
        self.active.pop();
        // Set *after* the body is fed, not before: feeding it ends with `after_word`, which clears
        // this flag. `alias sudo='sudo '` exists to make the following word a candidate, and
        // setting the flag first meant the body immediately unset it.
        self.check_next_word = body.ends_with(' ') || body.ends_with('\t');
        true
    }

    /// The replacement text for `word`, from the script's own definitions or the environment.
    fn body_for(&self, word: &str) -> Option<String> {
        if !self.expanding {
            return None;
        }
        if let Some((defined_on, body)) = self.defined.get(word) {
            // Not on the line that defined it: `alias x=y; x` finds no `x`, in bash either.
            if *defined_on < self.line {
                return Some(body.clone());
            }
        }
        (self.lookup)(word)
    }

    /// What a word leaves behind: whether the next one still begins a command.
    fn after_word(&mut self, word: &str) {
        self.check_next_word = false;
        match word {
            "case" => self.cases.push(Case::AwaitingIn),
            "esac" => {
                self.cases.pop();
            }
            // `for x in a b c` and `select x in …` introduce a *word list*, not commands, and it
            // runs until `do`.
            "for" | "select" => self.word_list = true,
            "in" if self.cases.last() == Some(&Case::AwaitingIn) => {
                *self.cases.last_mut().expect("checked") = Case::Patterns;
            }
            "do" => self.word_list = false,
            _ => {}
        }
        // A command prefix is not the command: in `LC_ALL=C sort`, `sort` is still the command
        // word, and an alias for it must still be found.
        self.after_assignment = is_assignment(word);
        self.cmd_pos =
            !self.in_word_context() && (INTRODUCERS.contains(&word) || self.after_assignment);
        // A prefix is not a new stage, so it leaves the pipeline position alone; any other
        // introducer decides it, and only `|` opens a stage with an upstream.
        if self.cmd_pos && !self.after_assignment {
            self.after_pipe = word == "|";
        }
    }

    /// Whether what has been emitted so far ends at a single `|`.
    ///
    /// Read from the output rather than tracked, because the three characters that end a command
    /// arrive one at a time: `||` reaches the operator arm twice, and a newline may follow a `|`
    /// that is continuing a pipeline onto the next line.
    fn ends_with_pipe(&self) -> bool {
        let text = self.out.trim_end();
        text.ends_with('|') && !text.ends_with("||")
    }

    /// Copy a balanced run through, remembering it when it does not close on this line.
    fn copy_run(&mut self, chars: &[char], from: usize, open: char, close: char) -> Option<usize> {
        let mut balance = Balance::new(open, close);
        match balance.consume(&mut self.out, chars, from) {
            Some(after) => Some(after),
            None => {
                self.pending = Some(balance);
                None
            }
        }
    }

    /// Whether the scanner is somewhere that holds *words* rather than commands.
    fn in_word_context(&self) -> bool {
        self.word_list || self.in_case_patterns()
    }

    /// Between a `case`'s `in` and the `)` that opens an arm, where the words are patterns.
    ///
    /// Separate from [`Self::in_word_context`] because the two end differently: a `for` list is
    /// closed by the `;` or newline before its `do`, while a `case` pattern list survives both.
    fn in_case_patterns(&self) -> bool {
        matches!(
            self.cases.last(),
            Some(Case::AwaitingIn) | Some(Case::Patterns)
        )
    }
}

#[cfg(test)]
mod tests;
