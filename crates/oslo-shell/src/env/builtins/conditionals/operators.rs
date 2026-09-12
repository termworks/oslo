//! The primaries shared by `test`/`[` and `[[ ]]`.
//!
//! There is exactly one implementation of every operator here, and both conditional builtins call
//! it. The bug this replaces was two half-tables that disagreed: `test` knew seven unary operators
//! and answered *false* to the rest, while `[[` knew fourteen — so `[ -s file ]` and
//! `[[ -s file ]]` gave different answers for the same file.
//!
//! Two rules the old code broke, and why they matter:
//!
//! * An operator that does not exist is a **syntax error**, never `false`. A typo'd predicate that
//!   quietly reports "no" is indistinguishable from a real negative answer, so a script guarding
//!   on `[ -q "$f" ]` would take the wrong branch forever without a diagnostic.
//! * Permission predicates ask the kernel. `-r` implemented as `stat()` reports `/etc/shadow` as
//!   readable by an ordinary user, because `stat` only needs search permission on the *directory*.

use crate::env::scope::Environment;
use crate::expand::glob::ShellPattern;
use crate::syntax::lower::cond::{QUOTED_CLOSE, QUOTED_OPEN};
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};

/// A conditional-expression diagnostic. Carrying it as an error rather than returning a truth
/// value is the whole point: every path that produces one exits 2, and none of them can be
/// mistaken for an answer.
pub(super) struct TestError(String);

impl TestError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        TestError(message.into())
    }

    pub(super) fn message(&self) -> &str {
        &self.0
    }
}

pub(super) type TestResult<T> = std::result::Result<T, TestError>;

/// Which conditional builtin is being evaluated.
///
/// The only operator whose *meaning* depends on this is `==`: inside `[[ ]]` an unquoted
/// right-hand side is a glob pattern (`[[ abc == a* ]]` is true), whereas POSIX `test` compares
/// literally (`[ abc == a* ]` is false).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Mode {
    Posix,
    Extended,
}

/// Every word bash's `test_unop` accepts. Membership decides *parsing*, not truth: `[ -f -a ]` is
/// the unary `-f` applied to the string `-a`, because `-a` sits in the operand slot.
pub(super) fn is_unary_op(op: &str) -> bool {
    matches!(
        op,
        "-a" | "-b"
            | "-c"
            | "-d"
            | "-e"
            | "-f"
            | "-g"
            | "-h"
            | "-k"
            | "-n"
            | "-o"
            | "-p"
            | "-r"
            | "-s"
            | "-t"
            | "-u"
            | "-v"
            | "-w"
            | "-x"
            | "-z"
            | "-G"
            | "-L"
            | "-N"
            | "-O"
            | "-R"
            | "-S"
    )
}

/// Every word bash's `test_binop` accepts.
pub(super) fn is_binary_op(op: &str) -> bool {
    matches!(
        op,
        "=" | "=="
            | "!="
            | "<"
            | ">"
            | "-eq"
            | "-ne"
            | "-lt"
            | "-le"
            | "-gt"
            | "-ge"
            | "-nt"
            | "-ot"
            | "-ef"
    )
}

pub(super) fn eval_unary(env: &Environment, op: &str, target: &str) -> TestResult<bool> {
    // The bytes the name really has, not its UTF-8 stand-in: `[ -e "$f" ]` over a glob match.
    let real = oslo_base::lossless::to_os(target);
    let path = std::path::Path::new(&real);

    // Resolved lazily: the string predicates must not stat anything, or `[ -n "$x" ]` would pay a
    // syscall per loop iteration.
    let file_type = || fs::metadata(path).ok().map(|m| m.file_type());
    let mode_bits = || fs::metadata(path).map(|m| m.mode()).unwrap_or(0);

    Ok(match op {
        "-e" | "-a" => path.exists(),
        "-f" => path.is_file(),
        "-d" => path.is_dir(),
        // `-h`/`-L` must not follow the link, so this is the one predicate using lstat.
        "-h" | "-L" => fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()),
        "-s" => fs::metadata(path).is_ok_and(|m| m.len() > 0),
        "-r" => access(path, nix::unistd::AccessFlags::R_OK),
        "-w" => access(path, nix::unistd::AccessFlags::W_OK),
        "-x" => access(path, nix::unistd::AccessFlags::X_OK),
        "-p" => file_type().is_some_and(|t| t.is_fifo()),
        "-S" => file_type().is_some_and(|t| t.is_socket()),
        "-b" => file_type().is_some_and(|t| t.is_block_device()),
        "-c" => file_type().is_some_and(|t| t.is_char_device()),
        "-u" => mode_bits() & 0o4000 != 0,
        "-g" => mode_bits() & 0o2000 != 0,
        "-k" => mode_bits() & 0o1000 != 0,
        "-O" => fs::metadata(path).is_ok_and(|m| m.uid() == nix::unistd::geteuid().as_raw()),
        "-G" => fs::metadata(path).is_ok_and(|m| m.gid() == nix::unistd::getegid().as_raw()),
        // "modified since last read": bash's test.c asks `atime <= mtime`, so a file whose two
        // stamps are identical — every freshly written file that nothing has read — counts as
        // modified. A strict `>` reported those as already-read.
        "-N" => fs::metadata(path)
            .is_ok_and(|m| (m.mtime(), m.mtime_nsec()) >= (m.atime(), m.atime_nsec())),
        // A non-numeric fd is not an error, just never a terminal.
        "-t" => target
            .trim()
            .parse::<i32>()
            .is_ok_and(|fd| nix::unistd::isatty(fd).unwrap_or(false)),
        "-z" => target.is_empty(),
        "-n" => !target.is_empty(),
        "-v" => env.get_param(target).is_some(),
        // `-o NAME` reads the same table `set -o` writes, so `[[ -o errexit ]]` cannot disagree
        // with `set -o` about whether errexit is on. An unknown name is false, not an error —
        // that is what bash answers, and it is what makes `[[ -o pipefail ]] || ...` portable to
        // shells lacking the option.
        "-o" => crate::env::options::ShellOption::from_name(target).is_some_and(|o| env.option(o)),
        // `-R` needs namerefs, which this shell does not have; false is what bash answers for a
        // variable that is set but not a nameref, and no variable here is ever one.
        "-R" => false,
        other => {
            return Err(TestError::new(format!(
                "{}: unary operator expected",
                other
            )));
        }
    })
}

/// Real access check, rather than inferring readability from a successful `stat`.
fn access(path: &std::path::Path, mode: nix::unistd::AccessFlags) -> bool {
    nix::unistd::access(path, mode).is_ok()
}

pub(super) fn eval_binary(mode: Mode, left: &str, op: &str, right: &str) -> TestResult<bool> {
    Ok(match op {
        "=" => left == right,
        "==" => pattern_or_literal(mode, left, right),
        "!=" => !pattern_or_literal(mode, left, right),
        // Byte-order comparison, as bash's `test` does; no locale collation.
        "<" => left < right,
        ">" => left > right,
        "-eq" => to_int(left)? == to_int(right)?,
        "-ne" => to_int(left)? != to_int(right)?,
        "-lt" => to_int(left)? < to_int(right)?,
        "-le" => to_int(left)? <= to_int(right)?,
        "-gt" => to_int(left)? > to_int(right)?,
        "-ge" => to_int(left)? >= to_int(right)?,
        "-nt" => newer_than(left, right),
        "-ot" => newer_than(right, left),
        "-ef" => same_file(left, right),
        other => {
            return Err(TestError::new(format!(
                "{}: binary operator expected",
                other
            )));
        }
    })
}

/// `[[ left == right ]]`, where `right` is a pattern, against `[ left = right ]`, where it is not.
///
/// A fully quoted operand never gets here as a pattern — the adapter emits the POSIX operator for
/// `[[ $x == "$y" ]]`. A *partly* quoted one arrives with its quoted runs between the adapter's
/// marks, and each character keeps whether it was quoted.
fn pattern_or_literal(mode: Mode, left: &str, right: &str) -> bool {
    match mode {
        Mode::Posix => left == right,
        Mode::Extended => {
            marked_pattern(right).matches_case(left, oslo_base::glob::walk::nocasematch())
        }
    }
}

/// Compile a right operand, reading the adapter's marks as quoting.
fn marked_pattern(right: &str) -> ShellPattern {
    let mut chars = Vec::with_capacity(right.len());
    let mut quoted = false;
    for ch in right.chars() {
        match ch {
            QUOTED_OPEN => quoted = true,
            QUOTED_CLOSE => quoted = false,
            _ => chars.push((ch, !quoted)),
        }
    }
    ShellPattern::from_chars(&chars)
}

/// Operand of an arithmetic comparison.
///
/// `unwrap_or(0)` here was a silent-wrong-answer generator: `[ "$count" -gt 0 ]` with `count`
/// holding `many` compared `0 > 0` and reported false, with status 0, as if the question had been
/// answered. bash exits 2 with `integer expected`, and so does this.
fn to_int(s: &str) -> TestResult<i64> {
    // Surrounding blanks are allowed (`[ " 3 " -eq 3 ]` is true in bash); a leading `+` is too.
    // Anything else — including `0x10` — is an error, since `test` is decimal-only.
    s.trim()
        .parse::<i64>()
        .map_err(|_| TestError::new(format!("{}: integer expected", s)))
}

/// True when `a` exists and is newer than `b`, or when `b` does not exist.
fn newer_than(a: &str, b: &str) -> bool {
    let ma = fs::metadata(oslo_base::lossless::to_os(a))
        .ok()
        .and_then(|m| m.modified().ok());
    let mb = fs::metadata(oslo_base::lossless::to_os(b))
        .ok()
        .and_then(|m| m.modified().ok());
    match (ma, mb) {
        (Some(a), Some(b)) => a > b,
        (Some(_), None) => true,
        _ => false,
    }
}

/// True when both paths refer to the same device and inode.
fn same_file(a: &str, b: &str) -> bool {
    match (
        fs::metadata(oslo_base::lossless::to_os(a)),
        fs::metadata(oslo_base::lossless::to_os(b)),
    ) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}
