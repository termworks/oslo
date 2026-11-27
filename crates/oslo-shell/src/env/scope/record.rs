//! What the command that just ran left behind: `$LINENO`, `PIPESTATUS`, and the status a command
//! substitution exited with.
//!
//! Separate from the variable table these end up in because they are the shell writing down its
//! own state rather than a script assigning to a name — none of them go through the readonly
//! check, and two of them are published only when the number actually moved.

use super::*;

impl Environment {
    /// Publish `line` as `$LINENO`, unless it is already what the variable says.
    ///
    /// The write itself is not free — a `String` for the number, the readonly check, the
    /// representability scan, an array probe and two more `String`s — and it happened before every
    /// command in every list. Interactively that is the same line number every time, because each
    /// typed command is line one; in a loop it is the same handful over and over.
    ///
    /// `published_line` is zeroed by any *other* route to the variable — an assignment or an
    /// `unset` — so the next command republishes rather than trusting a stale record.
    pub fn note_line(&mut self, line: u32) {
        // The tree's line, plus however much of the file was consumed before the chunk it came
        // from. See [`Self::set_line_offset`].
        let line = line.saturating_add(self.line_offset);
        self.current_line = line;
        if line == self.published_line {
            return;
        }
        self.set_var("LINENO", &line.to_string(), false);
        self.published_line = line;
    }

    /// How many lines of the file come before the chunk about to run. Answers the previous value.
    ///
    /// **For the one caller that runs a file in pieces.** A script that does not parse is run a
    /// command at a time, and each of those pieces is parsed on its own — so every line number in
    /// its tree counts from 1 rather than from where the piece began. One syntax error anywhere in
    /// a script therefore made *every* diagnostic before it say `line 1`, which is worse than
    /// saying nothing: the reader trusts a line number.
    ///
    /// The previous value comes back so a caller can put it back. That matters for `source`, which
    /// runs a whole file inside a chunk of another one: without a reset the sourced file's lines
    /// would be shifted by the outer file's offset, and without a restore the outer file's next
    /// chunk would lose it.
    pub fn set_line_offset(&mut self, lines: u32) -> u32 {
        std::mem::replace(&mut self.line_offset, lines)
    }

    /// The line of the command now running, counted from the top of its file.
    pub fn current_line(&self) -> u32 {
        self.current_line
    }

    /// Make `line`, already counted from the top of its file, the current one again.
    pub fn resume_line(&mut self, line: u32) {
        self.current_line = line;
        if line != self.published_line {
            self.set_var("LINENO", &line.to_string(), false);
            self.published_line = line;
        }
    }

    /// Number the lines of text about to be parsed and run from the line of the command now
    /// running, as bash numbers `eval` and `$(…)` bodies. Answers the previous offset.
    pub fn number_lines_from_here(&mut self) -> u32 {
        let here = self.current_line.saturating_sub(1);
        self.set_line_offset(here)
    }

    /// How many lines come before the chunk now running — see [`Self::set_line_offset`].
    pub fn line_offset(&self) -> u32 {
        self.line_offset
    }

    /// Where a diagnostic about the command now running should say it came from.
    ///
    /// ```text
    /// script.sh: line 4: nosuchcommand: command not found     in a script
    /// oslo: nosuchcommand: command not found                  at a prompt, or under -c
    /// ```
    ///
    /// **Because a script that fails should say where.** Every diagnostic oslo printed began
    /// `oslo: `, so a failure in a two-hundred-line script named the shell and the command and
    /// nothing about *which line* — the one thing the person fixing it needs. Every other shell
    /// answers with the file and the line, and `$LINENO` was already being published at each
    /// command boundary, so both halves of the answer were there and unused.
    ///
    /// **Only for a file**, which is the whole of the condition. A prompt has no line worth
    /// naming — every typed command is line one — and `-c` has no file, so both keep the plain
    /// `oslo: ` that a reader already associates with the shell talking about itself. That is also
    /// what bash does. A sourced file names *itself* rather than its includer — see
    /// [`Environment::enter_source_file`].
    pub fn origin(&self) -> String {
        // `script_frames` is pushed only for a file — `main` for a script, `source` for a sourced
        // one — and nothing is pushed for `-c` or for standard input. See `enter_script_frame`.
        if self.script_frames.is_empty() {
            return "oslo: ".to_string();
        }
        // Zero means nothing has published a line yet: a command the shell built for itself, a
        // `$(…)` body re-lexed into a list. Naming the file alone is still better than naming
        // neither, and inventing a line would be worse than both.
        if self.published_line == 0 {
            return format!("{}: ", self.current_file());
        }
        format!("{}: line {}: ", self.current_file(), self.published_line)
    }

    /// What a *report* should call the text it is pointing into.
    ///
    /// [`Self::origin`]'s file, and `oslo` where there is no file — a `-c` string and standard
    /// input are programs with no path, and naming them `$0` would put the shell's own binary at
    /// the head of a report about something you typed.
    pub fn diagnostic_source(&self) -> &str {
        match self.script_frames.is_empty() {
            true => "oslo",
            false => self.current_file(),
        }
    }

    /// Record every stage of a pipeline's exit status, left to right. A one-command pipeline
    /// records a single status, as bash's `PIPESTATUS` does.
    ///
    /// Publishing `PIPESTATUS` here rather than synthesising it during expansion keeps one copy
    /// of the numbers: `${PIPESTATUS[0]}` and `set -o pipefail` cannot disagree about what the
    /// last pipeline did. Written directly into the array table, bypassing the readonly check —
    /// this is the shell recording its own state, not a user assignment.
    pub fn set_pipeline_status(&mut self, statuses: Vec<i32>) {
        // The array is rebuilt only when the numbers moved. Publishing it unconditionally meant a
        // `Vec`, a `"PIPESTATUS"` `String`, a `BTreeMap` and a `String` per stage after every
        // command — and a run of successful commands reports the same `[0]` every time.
        if self.pipeline_status == statuses && self.get_array("PIPESTATUS").is_some() {
            return;
        }
        self.publish_array(
            "PIPESTATUS",
            ShellArray::from_values(statuses.iter().map(i32::to_string)),
        );
        self.pipeline_status = statuses;
    }

    /// The stage statuses of the most recent pipeline. Never empty.
    pub fn pipeline_status(&self) -> &[i32] {
        &self.pipeline_status
    }

    /// Record the text of the command about to run, for `$BASH_COMMAND`.
    pub fn set_bash_command(&mut self, text: String) {
        self.bash_command = text;
    }

    /// Record what a command substitution exited with. Called by `exec::substitution`.
    pub fn note_substitution_status(&mut self, status: i32) {
        self.substitution_status = Some(status);
    }

    /// The status of the last command substitution, without consuming it.
    ///
    /// Distinct from [`Environment::take_substitution_status`] because two callers want the same
    /// number for different reasons and must not steal it from each other: an assignment-only
    /// command *reports* it and is done with it, while word expansion only needs to know it in
    /// order to update `$?` mid-word.
    pub fn peek_substitution_status(&self) -> Option<i32> {
        self.substitution_status
    }

    /// Take the status of the last command substitution, clearing it.
    ///
    /// What an assignment-only command reports (POSIX: `x=$(exit 5)` leaves `$?` at 5). Consumed
    /// rather than read, so a later assignment with no substitution reports 0, not a stale number.
    pub fn take_substitution_status(&mut self) -> Option<i32> {
        self.substitution_status.take()
    }
}
