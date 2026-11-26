//! The full-screen macro manager: everything you have kept, and what to do with it.
//!
//! `oslo macros show` on a terminal. The history finder's shape, its keys and its look, because a
//! second full-screen list on the same machine that behaved differently would be a second thing to
//! learn rather than the same thing pointed at other data.
//!
//! ```text
//!    3d   alias   gs        git status --short                    #git #system
//!    3d   abbrev  gco       git checkout                          #git
//!   12d   script  deploy    #!/usr/bin/env python3                #work
//!
//!   ⬝⬝⬝⬝⬝⬝  >>  de▏                          [stored] @ [#work] || 1/4
//! ```
//!
//! # What the keys mean here, and why they are these keys
//!
//! | | |
//! |---|---|
//! | ← → | the **tag**. Where the finder's scopes are, because both are "narrow this list" |
//! | Tab | the **source**. Where the finder's profile is, because both are "a different set" |
//! | Enter | the **editor** — for every kind, including an alias |
//! | Delete | forget it, behind the confirmation the finder already asks for |
//! | Space | off for this session |
//! | Space ×3 | off everywhere |
//!
//! Enter is the one that differs from the finder, and it has to: the finder puts a line on the
//! prompt because a past command is something to run again, while a macro is something you keep and
//! what you want from it is to change it.
//!
//! # It asks the caller to do things rather than doing them
//!
//! This module draws and dispatches. Forgetting a macro, turning one off and opening `$EDITOR` all
//! belong to whoever owns the database — see [`Backing`] — which is what keeps `oslo-ui` free of
//! both the store and the editor, and what lets the screen be tested by handing it a fake.

mod render;
mod run;

pub use run::{Outcome, open};

/// One macro, flattened to what a row needs.
///
/// **Flattened, because a function is many lines and a list of many-line entries is not a list.**
/// The whole body is what Enter shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// `alias`, `abbrev`, `func` or `script` — a word rather than an enum, because this crate does
    /// not know what a macro is and does not need to.
    pub kind: String,
    pub name: String,
    /// The first line that is not blank.
    pub first: String,
    pub tags: Vec<String>,
    /// Unix seconds. `0` for one that never recorded when it was made.
    pub created: i64,
    /// On everywhere. `false` is the third space press.
    pub active: bool,
    /// Off in this shell only. `false` is one space press.
    pub session_off: bool,
    /// From the database, as against from the configuration.
    pub stored: bool,
    /// Another stored kind has the same name: a script file beside an alias or a function.
    pub clash: bool,
}

impl Item {
    /// The pieces the query is matched against, best score winning.
    ///
    /// **Field by field, not one joined string.** Joining them and matching once looks simpler and
    /// is wrong: a fuzzy match has a maximum gap between letters, so `git` against
    /// `alias gs echo gs git` has to jump ten characters and fails — the tag it was obviously
    /// asking for is right there and the row disappears. Matching each field on its own means
    /// typing `script` narrows to scripts and typing a tag narrows to that tag, with no special
    /// syntax for either.
    pub fn fields(&self) -> Vec<&str> {
        let mut fields = vec![self.name.as_str(), self.kind.as_str(), self.first.as_str()];
        fields.extend(self.tags.iter().map(String::as_str));
        fields
    }

    /// A name for the row that is unique across kinds, since `deploy` may be two things.
    pub fn key(&self) -> String {
        format!("{}/{}", self.kind, self.name)
    }

    /// Whether it applies to the shell you are standing in.
    pub fn on(&self) -> bool {
        self.active && !self.session_off
    }
}

/// Which set of macros the screen is showing. Tab moves between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The database — everything `oslo macros add` has put there.
    Stored,
    /// What your configuration defined: `alias` in a bash file, `oslo.alias` in Lua.
    ///
    /// **Aliases and abbreviations only.** A function is a file on disk and a script is a name in a
    /// Lua table; there is nothing to enumerate, so this source can never show either.
    Elsewhere,
}

impl Source {
    pub fn other(self) -> Source {
        match self {
            Source::Stored => Source::Elsewhere,
            Source::Elsewhere => Source::Stored,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Source::Stored => "[stored]",
            Source::Elsewhere => "[elsewhere]",
        }
    }

    pub fn holds(self, item: &Item) -> bool {
        matches!(
            (self, item.stored),
            (Source::Stored, true) | (Source::Elsewhere, false)
        )
    }
}

/// What the screen is asking the caller to do to a macro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// Delete it. Asked for behind a confirmation.
    Forget,
    /// Off, or back on, in this shell alone.
    Session(bool),
    /// Off, or back on, everywhere and in every shell.
    Everywhere(bool),
}

/// The database, from the screen's point of view.
///
/// Implemented by the caller — `oslo macros` — so that this crate needs neither the store nor an
/// editor, and so that the key loop can be driven in a test by something that records what it was
/// asked for and answers with a list.
pub trait Backing {
    /// Do it. A failure is the caller's to report; the screen carries on either way, because a row
    /// that refuses to change is still a row and the alternative is a screen that exits on an error
    /// you cannot read because the screen is what was covering it.
    fn act(&mut self, item: &Item, act: Act);
}
