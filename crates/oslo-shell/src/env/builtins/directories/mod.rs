//! Working directory: `cd`, `pwd`, and the directory stack (`pushd`, `popd`, `dirs`).
//!
//! Split along the seam between *moving* and *remembering*:
//!
//! * `chdir` — the single change-directory helper every one of these builtins goes through, and
//!   with it the logical/physical distinction, `CDPATH`, `$PWD` and `$OLDPWD`;
//! * `cd` — the `cd`/`pwd` option matrix in front of that helper;
//! * `jump` — where `cd` looks once the filesystem has said no, which is the only part of any of
//!   this that needs a database and the only part a script never reaches;
//! * `ring` — the directories this session has been in, which is what `cd -N` counts back through;
//! * `stack` — the directory-stack model plus `pushd`/`popd`;
//! * `dirs` — how the stack is printed.
//!
//! The reason the helper is shared rather than duplicated: `$OLDPWD` has to be written by
//! *whichever* builtin moved the shell, or a later `cd -` returns to a directory the user left
//! two commands ago.

mod cd;
mod chdir;
mod dirs;
mod jump;
pub mod ring;
mod stack;

pub use cd::{builtin_cd, builtin_pwd};
pub use dirs::builtin_dirs;
pub use stack::{builtin_popd, builtin_pushd, stack};
