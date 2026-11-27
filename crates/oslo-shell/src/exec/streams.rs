//! The stack of streams a coordinate reads from, and substituting one into a command's words.
//!
//! A *stream* is text something produced: a pipeline stage that has finished, or a whole command at
//! the prompt. Both go on the same stack, so stepping back through a pipeline and stepping back
//! through the session are the same motion — see [`oslo_base::coords`] for the coordinate itself.
//!
//! # Which way the index goes
//!
//! ```text
//! cat hosts.txt | grep web | ssh {0:0}
//!                      │           └─ 0  this stage's input: what `grep web` printed
//!                      └───────────── 1  one stage further back: what `cat` printed
//!
//! ssh {-1:0:0}   ← -1  the previous prompt, whatever it was
//! ```
//!
//! **Zero and up walk back through this pipeline; below zero walks back through the session.** They
//! are different collections and giving them one axis would mean `{3:…}` silently crossing from one
//! into the other when a pipeline happened to be short. The sign says which you meant, and it reads
//! the way negative indices already read on a line: from the other end.
//!
//! # A value is one argument
//!
//! Substitution happens on the **syntax tree**, before any expansion runs, and every value becomes
//! a single-quoted word part. Single quotes are literal throughout in every shell there is, so a
//! line holding a space, a `*` or a `$` arrives at the command whole and is never field-split or
//! re-globbed. Reusing the quoting the shell already has beats inventing a new origin and teaching
//! six expansions about it — and a shell that field-splits its own substitutions is a shell that
//! executes filenames.

use oslo_base::ast::{
    AssignmentValue, CaseItem, Command, CommandList, CompoundCommand, Redirection, Word, WordPart,
};
use oslo_base::coords::{self, Coord};

/// How many prompts back a coordinate may reach. See [`oslo_base::prompts`].
pub use oslo_base::prompts::KEPT as PROMPTS_KEPT;

/// The most of one stream that is kept, in bytes.
///
/// Shared with `keep`/`copy --last` deliberately — two limits on "how much output do we hold"
/// would be two numbers to reason about and one of them would be wrong.
pub use oslo_base::capture::MAX as STREAM_MAX;

/// Remember the line a prompt ran, for `{-n:…}` to address.
///
/// The ring itself is [`oslo_base::prompts`], because the line editor previews a session
/// coordinate before Enter and does not depend on this crate. These two names stay here so the
/// runtime keeps calling what it always called.
pub fn remember_prompt(line: &str) {
    oslo_base::prompts::remember(line);
}

/// Forget every remembered line. `history -c` clears this too — a line the user asked to be gone
/// must not stay reachable by coordinate.
pub fn forget_prompts() {
    oslo_base::prompts::forget();
}

/// The streams a coordinate can reach.
#[derive(Debug, Default, Clone)]
pub struct Streams {
    /// This pipeline's finished stages, oldest first. Index 0 of a coordinate is the *last* of
    /// these — the stage feeding the command being built.
    stages: Vec<String>,
    /// What each of those stages *was*, as words, parallel to `stages`. Kept as words rather than
    /// a line because an argument may contain a space — see [`coords::select_words`].
    commands: Vec<Vec<String>>,
    /// Previous prompts, newest first, so `-1` is `prompts[0]`.
    prompts: Vec<String>,
}

impl Streams {
    /// A stack whose negative side is the session's remembered prompt lines.
    ///
    /// Built per pipeline rather than kept: the stages are this pipeline's and the prompts are the
    /// session's, and copying a handful of short lines is cheaper than reasoning about a shared
    /// mutable stack across a fork.
    pub fn for_this_pipeline() -> Streams {
        Streams {
            stages: Vec::new(),
            commands: Vec::new(),
            prompts: oslo_base::prompts::all(),
        }
    }
}

impl Streams {
    /// Note what a pipeline stage printed.
    pub fn push_stage(&mut self, text: impl Into<String>) {
        self.stages.push(cap(text.into()));
    }

    /// Note what a pipeline stage *was*, as the words it was written with.
    ///
    /// Pushed alongside [`Streams::push_stage`] and kept the same length, so `{%0}` and `{0}` name
    /// the same stage.
    pub fn push_command(&mut self, words: Vec<String>) {
        self.commands.push(words);
    }

    /// The text a coordinate's stream dimension names, if there is one.
    ///
    /// `None` where nothing was captured, which reads as an empty selection rather than an error.
    pub fn text(&self, coord: &Coord) -> Option<&str> {
        let at = self.stream_index(coord);
        match at >= 0 {
            // Counting back from the newest stage: 0 is the one that just finished.
            true => {
                let back = at as usize;
                self.stages
                    .len()
                    .checked_sub(back + 1)
                    .map(|i| &self.stages[i][..])
            }
            // Previous prompts, newest first.
            false => self.prompts.get((-at - 1) as usize).map(String::as_str),
        }
    }

    /// The words of the command a `{%…}` coordinate names.
    ///
    /// **Both directions answer**, and they answer from different places for the same reason the
    /// output side does. Forward is a stage of this pipeline, whose words were recorded as it ran.
    /// Backward is a previous prompt, where the line that was typed is all there ever was — so
    /// `{%-1:0}` and `{-1:0:0}` are two spellings of the same word, and the `%` one is the one that
    /// says what it means.
    pub fn command_words(&self, coord: &Coord) -> Option<Vec<String>> {
        let at = self.stream_index(coord);
        match at >= 0 {
            true => self
                .commands
                .len()
                .checked_sub(at as usize + 1)
                .map(|i| self.commands[i].clone()),
            false => self
                .prompts
                .get((-at - 1) as usize)
                .map(|line| line.split_whitespace().map(str::to_string).collect()),
        }
    }

    /// Which stream a coordinate names, as a signed index.
    ///
    /// A range of *streams* is not meaningful — `{0..2:0:0}` would mean "the same line of three
    /// different commands", which is a question nobody asks and a syntax nobody would reach for by
    /// accident. The first is taken, so the coordinate still reads.
    fn stream_index(&self, coord: &Coord) -> isize {
        match coord.stream {
            coords::Sel::At(at) => at,
            coords::Sel::Span { from, .. } => from.unwrap_or(0),
        }
    }
}

mod quoted;
use quoted::{holds_a_quoted_coordinate, rewrite_inside_quotes};

/// Keep the head, not the tail: a coordinate counts from the start, and `{-1}` on a truncated
/// stream is honestly the last line *of what was kept*.
fn cap(mut text: String) -> String {
    if text.len() > STREAM_MAX {
        // **Back off to a character boundary.** The text arrives from `from_utf8_lossy` over a
        // buffer already cut at exactly this many bytes, so a character severed by that cut has
        // become a *three-byte* `U+FFFD` straddling the offset — and `String::truncate` asserts on
        // a boundary. A megabyte of output ending in the wrong place took the shell down with it.
        let mut at = STREAM_MAX;
        while at > 0 && !text.is_char_boundary(at) {
            at -= 1;
        }
        text.truncate(at);
    }
    text
}

/// Whether a word contains anything a coordinate could claim.
///
/// A cheap scan, because it runs on every word of every command. It only has to be right about
/// "there is a `{` with a digit, `-`, `*` or `:` after it" — [`substitute`] is what decides.
pub fn looks_like_a_coordinate(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(i, b)| {
        *b == b'{'
            && bytes.get(i + 1).is_some_and(|n| {
                n.is_ascii_digit() || matches!(n, b'-' | b'*' | b':' | b'.' | b'%')
            })
    })
}

/// Replace every coordinate in one piece of literal text, answering the words it becomes.
///
/// One word can become several: `{*:0}` on three lines is three arguments, the way `"$@"` is. That
/// only happens for a word that is a coordinate and **nothing else** — `{0:0}-{1:0}` has to stay one
/// word to mean anything, so there the values are joined with a space and the text between them is
/// kept.
///
/// `None` when the text holds no coordinate, so an ordinary brace group falls through to the brace
/// expansion that already handles it.
pub fn substitute(text: &str, streams: &Streams) -> Option<Vec<Word>> {
    // A word that is one coordinate and nothing else: one argument per value.
    if let Some((before, coord, after)) = split(text)
        && before.is_empty()
        && after.is_empty()
    {
        return Some(values(&coord, streams).into_iter().map(quoted).collect());
    }

    // Otherwise every coordinate is replaced where it stands, and the word stays one word.
    let mut parts = Vec::new();
    let mut rest = text;
    let mut found = false;
    while let Some((before, coord, after)) = split(rest) {
        found = true;
        if !before.is_empty() {
            parts.push(WordPart::Literal(before.to_string()));
        }
        parts.push(WordPart::SingleQuoted(values(&coord, streams).join(" ")));
        rest = after;
    }
    if !found {
        return None;
    }
    if !rest.is_empty() {
        parts.push(WordPart::Literal(rest.to_string()));
    }
    Some(vec![Word { parts }])
}

/// What a coordinate reads, or nothing when its stream was never captured.
fn values(coord: &Coord, streams: &Streams) -> Vec<String> {
    // Nothing captured: the coordinate reads empty rather than refusing to run, which is the rule
    // everywhere else in this feature.
    match coord.subject {
        coords::Subject::Output => match streams.text(coord) {
            Some(stream) => coords::select(coord, stream),
            None => Vec::new(),
        },
        coords::Subject::Command => match streams.command_words(coord) {
            Some(words) => coords::select_words(coord, &words),
            None => Vec::new(),
        },
    }
}

/// One value as one word that cannot be split or globbed.
fn quoted(value: String) -> Word {
    Word {
        parts: vec![WordPart::SingleQuoted(value)],
    }
}

/// Split a word around its first coordinate: `(before, coord, after)`.
///
/// Scans forward for a `{` that opens one, so `a{b}c{0:0}` finds the coordinate rather than giving
/// up at the brace group in front of it.
fn split(text: &str) -> Option<(&str, Coord, &str)> {
    let mut from = 0;
    while let Some(open) = text[from..].find('{') {
        let open = from + open;
        let close = open + text[open..].find('}')?;
        if let Some(coord) = coords::parse(&text[open + 1..close]) {
            return Some((&text[..open], coord, &text[close + 1..]));
        }
        from = open + 1;
    }
    None
}

/// Whether this text really holds a coordinate, parsed rather than guessed.
///
/// [`looks_like_a_coordinate`] is the cheap scan used to decide whether to bother; this is the
/// answer. `{0..2}` passes the scan and fails here, because it is brace expansion.
pub fn holds_a_coordinate(text: &str) -> bool {
    split(text).is_some()
}

/// Rewrite every coordinate a command can hold.
///
/// **Everywhere a word can appear, not only the argument list.** A coordinate in a redirection
/// target, an assignment's value or the body of a loop used to be left as text and then read as
/// nothing — `cat f | cat > {0:0}` wrote to a file called `{0:0}`, and `cat f | (echo {0:0})`
/// printed a blank line. Both failed *silently*, which is the worst way for a substitution to fail:
/// the command runs, and does the wrong thing.
///
/// **A function definition is not rewritten.** `f(){ echo {0:0}; }` defines a function whose body
/// is run later, when this stream is gone — baking today's text into it would make the definition
/// mean something different from what was written.
///
/// **Nor is a scalar assignment.** `w=x{1..3}` is text in bash — a scalar right-hand side is one of
/// the few places brace expansion deliberately does not reach — and rewriting it there made it
/// empty instead.
///
/// That is the rule the whole walk follows, and it is worth stating once: **a coordinate goes where
/// a brace expands.** Brace expansion runs on a word's source text before the lexer sees it, so by
/// the time there is a tree to walk an ordinary command word has already become its several words
/// and has no brace left to mistake. Whatever still holds a literal brace here is somewhere bash
/// refused to expand one, and a coordinate has no more business there than `{a,b}` does. The rule
/// cuts *through* assignments rather than around them: `a=(x{1,2})` expands and takes coordinates,
/// `w=x{1..3}` does neither.
pub fn rewrite_command(command: &mut Command, streams: &Streams) {
    match command {
        Command::Simple(simple) => {
            match regex_operand_of_a_conditional(&simple.words) {
                // Every word but the regex, and one word each: an operand of `[[ ]]` is one operand
                // however many spaces are in it.
                Some(skip) => {
                    for (at, word) in simple.words.iter_mut().enumerate() {
                        if at != skip {
                            rewrite_word(word, streams);
                        }
                    }
                }
                None => {
                    rewrite(&mut simple.words, streams);
                }
            }
            for assignment in &mut simple.assignments {
                // The scalar case is deliberately absent — see the note above. An array literal is
                // the other half of the same rule: `a=(x{1,2} {3..4})` *is* brace-expanded by bash,
                // so it is a word list, so a coordinate belongs there. The subscript is not — an
                // index is arithmetic, where a brace never expanded either.
                if let AssignmentValue::Array(elements) = &mut assignment.value {
                    for element in elements {
                        rewrite_word(&mut element.value, streams);
                    }
                }
            }
            rewrite_redirections(&mut simple.redirections, streams);
        }
        Command::Compound { kind, redirections } => {
            rewrite_compound(kind, streams);
            rewrite_redirections(redirections, streams);
        }
        // Deliberately untouched — see the note above.
        Command::FunctionDef { .. } => {}
    }
}

fn rewrite_redirections(redirections: &mut [Redirection], streams: &Streams) {
    for redirection in redirections {
        rewrite_word(&mut redirection.target, streams);
        if let Some(body) = &mut redirection.heredoc_content {
            rewrite_word(body, streams);
        }
    }
}

fn rewrite_compound(kind: &mut CompoundCommand, streams: &Streams) {
    match kind {
        CompoundCommand::If {
            condition,
            then_branch,
            elif_branches,
            else_branch,
        } => {
            rewrite_list(condition, streams);
            rewrite_list(then_branch, streams);
            for (condition, body) in elif_branches {
                rewrite_list(condition, streams);
                rewrite_list(body, streams);
            }
            if let Some(body) = else_branch {
                rewrite_list(body, streams);
            }
        }
        CompoundCommand::While { condition, body } | CompoundCommand::Until { condition, body } => {
            rewrite_list(condition, streams);
            rewrite_list(body, streams);
        }
        CompoundCommand::For { items, body, .. } | CompoundCommand::Select { items, body, .. } => {
            if let Some(items) = items {
                let mut words = std::mem::take(items);
                rewrite(&mut words, streams);
                *items = words;
            }
            rewrite_list(body, streams);
        }
        CompoundCommand::Case { word, items } => {
            rewrite_word(word, streams);
            for item in items.iter_mut() {
                rewrite_case_item(item, streams);
            }
        }
        CompoundCommand::ArithmeticFor { body, .. } => rewrite_list(body, streams),
        CompoundCommand::Coproc { body, .. } => rewrite_command(body, streams),
        CompoundCommand::Subshell(list) | CompoundCommand::Group(list) => {
            rewrite_list(list, streams)
        }
        // Arithmetic is a string the arithmetic parser owns, not a word list.
        CompoundCommand::Arithmetic(_) => {}
    }
}

fn rewrite_case_item(item: &mut CaseItem, streams: &Streams) {
    let mut patterns = std::mem::take(&mut item.patterns);
    rewrite(&mut patterns, streams);
    item.patterns = patterns;
    rewrite_list(&mut item.body, streams);
}

fn rewrite_list(list: &mut CommandList, streams: &Streams) {
    for item in &mut list.items {
        rewrite_pipeline(&mut item.and_or.first, streams);
        for (_, pipeline) in &mut item.and_or.rest {
            rewrite_pipeline(pipeline, streams);
        }
    }
}

/// A nested pipeline is rewritten only when it has no stream of its own.
///
/// **Where the recursion has to stop.** `cat f | (echo {0:0})` names the outer pipe, because the
/// subshell has nothing feeding it — so it is rewritten from here. But
/// `for i in 1 2; do printf "$i\n" | echo {0:0}; done` does not: that inner pipeline has its own
/// upstream, and its coordinate means *that* one. Rewriting it from out here answered with the
/// enclosing stream, which for a loop at the top of a pipeline is nothing at all — the loop printed
/// three blanks.
///
/// So a nested pipeline of two or more stages is left alone. It reaches `run_stages` in its own
/// right and asks the same question there.
fn rewrite_pipeline(pipeline: &mut oslo_base::ast::Pipeline, streams: &Streams) {
    if pipeline.commands.len() > 1 {
        return;
    }
    for command in &mut pipeline.commands {
        rewrite_command(command, streams);
    }
}

/// Rewrite one word in place, when it is a lone literal holding a coordinate.
///
/// A word that becomes *several* cannot be put back in a slot that holds one, so a redirection
/// target or an assignment value takes the values joined — which is what a single slot can mean.
fn rewrite_word(word: &mut Word, streams: &Streams) {
    let Some(text) = only_literal(word) else {
        rewrite_inside_quotes(word, streams);
        return;
    };
    let Some(mut replacements) = substitute(text, streams) else {
        return;
    };
    *word = match replacements.len() {
        1 => replacements.remove(0),
        _ => Word {
            parts: vec![WordPart::SingleQuoted(
                replacements
                    .iter()
                    .map(|word| {
                        word.parts
                            .iter()
                            .map(|part| match part {
                                WordPart::Literal(text) | WordPart::SingleQuoted(text) => {
                                    text.as_str()
                                }
                                _ => "",
                            })
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            )],
        },
    };
}

/// Rewrite every coordinate in a simple command's words, in place.
///
/// **Single quotes are literal and double quotes expand**, which is the rule every other expansion
/// in the shell already follows: `echo "$x"` is the value and `echo '$x'` is the text. A coordinate
/// had been literal in *both*, which made `echo "ran {%0:0} and got {0}"` — a coordinate in the
/// middle of a message, the obvious thing to want — impossible to write.
///
/// Inside quotes the values join with a space and the word stays one word, because that is what a
/// quoted word means. Outside, a lone coordinate still becomes one argument per value.
///
/// Answers whether anything changed, so a caller can tell a command that needed the stack from one
/// that merely looked like it might.
pub fn rewrite(words: &mut Vec<Word>, streams: &Streams) -> bool {
    let mut out = Vec::with_capacity(words.len());
    let mut changed = false;
    for mut word in words.drain(..) {
        let Some(text) = only_literal(&word) else {
            changed |= rewrite_inside_quotes(&mut word, streams);
            out.push(word);
            continue;
        };
        match substitute(text, streams) {
            Some(replacements) => {
                changed = true;
                out.extend(replacements);
            }
            None => out.push(word),
        }
    }
    *words = out;
    changed
}

/// The text of a word that is one unquoted literal, which is the only shape a coordinate can be
/// written in.
fn only_literal(word: &Word) -> Option<&str> {
    match word.parts.as_slice() {
        [WordPart::Literal(text)] => Some(text.as_str()),
        _ => None,
    }
}

/// Where the regex lives in a lowered `[[ … =~ … ]]`, if this command is one.
///
/// **A regex is not a word list, and it is the one operand that has to be found by position.**
/// `syntax::lower::cond` keeps coordinates out of it by refusing to leave it bare
/// — but it wraps every operand in a synthetic `DoubleQuoted` to stop `[[ ]]` field-splitting, and
/// once double quotes started expanding, walking into that wrapper reached the regex again and ate
/// its `{4}` quantifiers. The lowered form is `[[ left op right ]]`, so the operand after `=~` is
/// found here and left alone.
fn regex_operand_of_a_conditional(words: &[Word]) -> Option<usize> {
    if only_literal(words.first()?)? != "[[" {
        return None;
    }
    let at = words.iter().position(|w| only_literal(w) == Some("=~"))?;
    Some(at + 1)
}

mod gate;
pub use gate::command_uses_coordinates;

#[cfg(test)]
#[path = "streams/tests.rs"]
mod tests;

#[cfg(test)]
mod cap_tests {
    use super::{STREAM_MAX, cap};

    /// **The cut lands on a character, not inside one.**
    ///
    /// A captured stream is read to exactly [`STREAM_MAX`] bytes and then passed through
    /// `from_utf8_lossy`, so a character the read severed comes back as a three-byte `U+FFFD` that
    /// *straddles* the cap. `String::truncate` asserts on a boundary, so a megabyte of output
    /// ending in the wrong place aborted the shell.
    #[test]
    fn a_stream_severed_mid_character_is_cut_at_a_boundary() {
        // Exactly the shape the reader produces: filler, then a character across the boundary.
        let severed = "a".repeat(STREAM_MAX - 1) + "日";
        assert!(severed.len() > STREAM_MAX, "the replacement pushes it over");
        let capped = cap(severed);
        assert!(capped.len() <= STREAM_MAX);
        // The straddling character is dropped whole rather than half-kept.
        assert_eq!(capped.len(), STREAM_MAX - 1);
        assert!(capped.ends_with('a'));
    }

    /// Text that fits is untouched, and a cut that already lands on a boundary keeps every byte it
    /// is allowed.
    #[test]
    fn text_within_the_cap_is_left_alone() {
        assert_eq!(cap("short".to_string()), "short");
        let exact = "a".repeat(STREAM_MAX);
        assert_eq!(cap(exact.clone()).len(), STREAM_MAX);
        let over = "a".repeat(STREAM_MAX + 10);
        assert_eq!(cap(over).len(), STREAM_MAX);
    }
}
