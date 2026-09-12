//! A full-screen history finder.
//!
//! Up opens it. It fills the screen with everything the shell has ever run, newest first, with a
//! search bar along the bottom; typing filters fuzzily and Enter puts the chosen line back on the
//! prompt, unrun, for you to edit or accept.
//!
//! # How it differs from the completion dropdown
//!
//! They look alike on purpose and answer different questions. The dropdown is *suggestions*: what
//! could this half-typed word become — a file, a flag, a command on `$PATH`? It appears under the
//! prompt, borrows a few rows, and is about the word the cursor is in.
//!
//! This is *history*, and only history. Nothing is suggested; every row is something you have
//! actually run. So it drops the dropdown's kind badge — every row is the same kind — and spends
//! that column on the thing worth knowing about a past command: **when you last ran it**, beside
//! how often and where.
//!
//! The finder opens over the current git worktree's history, or global history outside one (see
//! `oslo.finder.scope`). **Left and Right narrow and widen the scope** — global,
//! host, session, directory, workspace — and **Tab moves to the next profile**, which is a
//! different pair of stores and so a different history entirely. The bar's right end says which:
//! `default @ [global] || 3/57`. They are the arrows because the scopes are a line from widest to narrowest, and because
//! there is no cursor to move in a search box that only ever appends.
//!
//! # Why it takes the whole screen
//!
//! Because history is long. The dropdown shows eight rows because a completion list that pushed
//! the prompt up the screen would be worse than the thing it is helping with. A history search is
//! the opposite: you came here to look through a lot of it, and the alternate screen means the
//! prompt is exactly where you left it when you leave.
//!
//! # Where the data comes from
//!
//! [`oslo_base::track::history`], not the history file. The file keeps lines in the order they were
//! typed; the tracker keeps counts, timestamps and directories per command, which is what the
//! three ranking signals need. See [`rank`] for the order, and why it is that order.

pub mod rank;
pub mod render;
mod run;

/// Which part of history the finder is searching.
///
/// The order is widest to narrowest, so Right feels like closing in on what you want and Left like
/// opening back out — rather than jumping between unrelated views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Everything the store knows.
    Global,
    /// This machine only.
    ///
    /// **Identical to [`Scope::Global`] today**, and deliberately still its own scope: the store
    /// is local, so every row in it was run here. It becomes a real filter the moment history is
    /// shared between machines, and having the name already means that change is a filter rather
    /// than a new concept for anyone to learn.
    Host,
    /// Only what this shell has run since it started.
    Session,
    /// Only what has been run in the current directory.
    Directory,
    /// Anywhere inside the current git worktree.
    Workspace,
}

impl Scope {
    /// One scope narrower, wrapping — Right.
    pub fn next(self) -> Scope {
        match self {
            Scope::Global => Scope::Host,
            Scope::Host => Scope::Session,
            Scope::Session => Scope::Directory,
            Scope::Directory => Scope::Workspace,
            Scope::Workspace => Scope::Global,
        }
    }

    /// One scope wider, wrapping — Left.
    pub fn previous(self) -> Scope {
        match self {
            Scope::Global => Scope::Workspace,
            Scope::Host => Scope::Global,
            Scope::Session => Scope::Host,
            Scope::Directory => Scope::Session,
            Scope::Workspace => Scope::Directory,
        }
    }

    /// What the search bar shows.
    pub fn label(self) -> &'static str {
        match self {
            Scope::Global => "[global]",
            Scope::Host => "[host]",
            Scope::Session => "[session]",
            Scope::Directory => "[directory]",
            Scope::Workspace => "[workspace]",
        }
    }
}

pub use run::{Outcome, open};
