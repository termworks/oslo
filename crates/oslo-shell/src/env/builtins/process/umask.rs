//! `umask`, including the symbolic modes POSIX requires and the `-S`/`-p` output forms.
//!
//! What was here before accepted an octal string, ran it through `Mode::from_bits`, and dropped
//! the result on the floor when either step failed — so `umask 999` and `umask u=rwx,g=,o=` were
//! both silent no-ops that reported success. The parse now returns a value or a diagnostic, and
//! the only way to reach `umask(2)` is through a value.

use crate::env::origin_now;
use crate::env::scope::Environment;
use nix::sys::stat::{Mode, mode_t, umask};
use oslo_base::error::Result;
use std::fmt;

/// The permission triplets symbolic modes talk about. Set-user-ID, set-group-ID and the sticky
/// bit can be written in a symbolic mode but have no meaning in a file-creation mask, and Linux
/// does not keep them there.
const PERM_BITS: u32 = 0o777;

/// Widest mask an octal operand may specify. The kernel keeps only the permission bits, but the
/// four-digit form is what shells accept and print.
const MAX_OCTAL: u32 = 0o7777;

/// Why a mode operand was refused. Held as a value so the parse stays pure and testable: the
/// mask must not move before we know the whole operand is good.
#[derive(Debug, PartialEq, Eq)]
pub enum ModeError {
    OutOfRange(String),
    Operator(String),
    Character(char),
}

impl fmt::Display for ModeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModeError::OutOfRange(spec) => write!(f, "{spec}: octal number out of range"),
            ModeError::Operator(tok) => write!(f, "`{tok}': invalid symbolic mode operator"),
            ModeError::Character(c) => write!(f, "`{c}': invalid symbolic mode character"),
        }
    }
}

pub fn builtin_umask(_env: &mut Environment, args: &[String]) -> Result<i32> {
    let mut symbolic = false;
    let mut as_command = false;
    let mut idx = 1;

    while idx < args.len() {
        let arg = args[idx].as_str();
        if arg == "--" {
            idx += 1;
            break;
        }
        // A mode never starts with `-`, so anything that does is an option or a mistake.
        if !arg.starts_with('-') || arg.len() < 2 {
            break;
        }
        for c in arg[1..].chars() {
            match c {
                'S' => symbolic = true,
                'p' => as_command = true,
                _ => {
                    crate::env::complain_option(
                        args,
                        c,
                        &format!("umask: -{c}: invalid option"),
                        "umask: usage: umask [-p] [-S] [mode]",
                    );
                    return Ok(2);
                }
            }
        }
        idx += 1;
    }

    let current = current_mask();
    let Some(spec) = args.get(idx) else {
        print_mask(current, symbolic, as_command);
        return Ok(0);
    };

    match parse_mode(spec, current) {
        Ok(new_mask) => {
            umask(Mode::from_bits_truncate((new_mask & MAX_OCTAL) as mode_t));
            // `-S` with a mode sets it and then shows the result symbolically, as bash does.
            if symbolic {
                print_mask(new_mask & MAX_OCTAL, true, false);
            }
            Ok(0)
        }
        Err(e) => {
            eprintln!("{}umask: {e}", origin_now());
            Ok(1)
        }
    }
}

/// Read the mask without changing it. There is no getter: `umask(2)` always sets, so the value
/// has to be put straight back.
fn current_mask() -> u32 {
    let current = umask(Mode::empty());
    umask(current);
    current.bits() as u32
}

fn print_mask(mask: u32, symbolic: bool, as_command: bool) {
    let body = if symbolic {
        symbolic_form(mask)
    } else {
        format!("{mask:04o}")
    };
    if as_command {
        // `-p` prints something that can be pasted back to restore this mask.
        let flag = if symbolic { "-S " } else { "" };
        println!("umask {flag}{body}");
    } else {
        println!("{body}");
    }
}

/// `-S` prints the permissions the mask *allows*, not the mask itself.
fn symbolic_form(mask: u32) -> String {
    let allowed = !mask & PERM_BITS;
    ["u", "g", "o"]
        .iter()
        .zip([6, 3, 0])
        .map(|(who, shift)| {
            let bits = (allowed >> shift) & 0o7;
            let mut perms = String::new();
            for (bit, c) in [(0o4, 'r'), (0o2, 'w'), (0o1, 'x')] {
                if bits & bit != 0 {
                    perms.push(c);
                }
            }
            format!("{who}={perms}")
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Turn a mode operand into the new mask, relative to `current` for the symbolic forms.
pub fn parse_mode(spec: &str, current: u32) -> std::result::Result<u32, ModeError> {
    if spec.starts_with(|c: char| c.is_ascii_digit()) {
        let valid = spec.chars().all(|c| ('0'..='7').contains(&c));
        let value = u32::from_str_radix(spec, 8)
            .ok()
            .filter(|v| *v <= MAX_OCTAL);
        return match (valid, value) {
            (true, Some(v)) => Ok(v),
            _ => Err(ModeError::OutOfRange(spec.to_string())),
        };
    }
    apply_symbolic(spec, current)
}

/// Symbolic modes describe the permissions to *keep*, so they are applied to the complement of
/// the mask and complemented back at the end — `umask u=rwx,g=,o=` means "owner keeps
/// everything, nobody else gets anything", i.e. mask 0077.
fn apply_symbolic(spec: &str, current: u32) -> std::result::Result<u32, ModeError> {
    let mut allowed = !current & PERM_BITS;
    for clause in spec.split(',') {
        allowed = apply_clause(clause, allowed)?;
    }
    Ok(!allowed & PERM_BITS)
}

fn apply_clause(clause: &str, mut allowed: u32) -> std::result::Result<u32, ModeError> {
    let mut chars = clause.chars().peekable();

    let mut who = 0u32;
    while let Some(bits) = chars.peek().and_then(|c| who_bits(*c)) {
        who |= bits;
        chars.next();
    }
    if who == 0 {
        who = PERM_BITS; // an omitted `who` means all three triplets
    }

    let mut applied = false;
    loop {
        let Some(op) = chars.next() else {
            // `u=rwx` ends here; a clause that never carried an operator is a syntax error.
            if applied {
                return Ok(allowed);
            }
            return Err(ModeError::Operator(clause.to_string()));
        };
        if !matches!(op, '+' | '-' | '=') {
            return Err(ModeError::Operator(op.to_string()));
        }

        let mut perms = 0u32;
        while let Some(&c) = chars.peek() {
            match c {
                'r' => perms |= 0o444,
                'w' => perms |= 0o222,
                // `X` is chmod's conditional execute; on a creation mask there is no file to be
                // conditional about, so it is plain execute. `s` and `t` are accepted and have
                // no effect: the bits they name are not part of the mask.
                'x' | 'X' => perms |= 0o111,
                's' | 't' => {}
                'u' | 'g' | 'o' => {
                    // A copy: `g=u` gives the group whatever the owner currently keeps.
                    let shift = match c {
                        'u' => 6,
                        'g' => 3,
                        _ => 0,
                    };
                    let triplet = (allowed >> shift) & 0o7;
                    perms |= triplet * 0o111;
                }
                '+' | '-' | '=' => break,
                _ => return Err(ModeError::Character(c)),
            }
            chars.next();
        }

        let effective = perms & who;
        allowed = match op {
            '=' => (allowed & !who) | effective,
            '+' => allowed | effective,
            _ => allowed & !effective,
        };
        applied = true;
    }
}

fn who_bits(c: char) -> Option<u32> {
    match c {
        'u' => Some(0o700),
        'g' => Some(0o070),
        'o' => Some(0o007),
        'a' => Some(PERM_BITS),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parse is pure so it can be tested without touching the process-wide mask, which the
    /// rest of the test binary's threads share.
    #[test]
    fn octal_modes_are_range_checked() {
        assert_eq!(parse_mode("022", 0), Ok(0o022));
        assert_eq!(parse_mode("0022", 0), Ok(0o022));
        assert_eq!(parse_mode("7777", 0), Ok(0o7777));
        // Every one of these used to be a silent no-op that reported success.
        assert_eq!(
            parse_mode("999", 0),
            Err(ModeError::OutOfRange("999".into()))
        );
        assert_eq!(parse_mode("8", 0), Err(ModeError::OutOfRange("8".into())));
        assert_eq!(
            parse_mode("77777", 0),
            Err(ModeError::OutOfRange("77777".into()))
        );
    }

    #[test]
    fn symbolic_modes_apply_to_the_permissions_the_mask_keeps() {
        assert_eq!(parse_mode("u=rwx,g=,o=", 0o022), Ok(0o077));
        assert_eq!(parse_mode("a-w", 0o000), Ok(0o222));
        assert_eq!(parse_mode("u+w,go-rx", 0o222), Ok(0o077));
        assert_eq!(parse_mode("=rwx", 0o022), Ok(0o000));
        assert_eq!(parse_mode("a=", 0o022), Ok(0o777));
        // A copy reads the *current* permissions: from 0777 the owner keeps nothing, so neither
        // does the group.
        assert_eq!(parse_mode("g=u", 0o777), Ok(0o777));
        assert_eq!(parse_mode("g=u", 0o077), Ok(0o007));
        // Only the owner's triplet moves.
        assert_eq!(parse_mode("u=r", 0o022), Ok(0o322));
        // Set-user-ID and sticky are accepted and ignored; they are not mask bits.
        assert_eq!(parse_mode("u=rws", 0o022), Ok(0o122));
    }

    #[test]
    fn bad_symbolic_modes_are_diagnosed() {
        assert_eq!(parse_mode("abc", 0), Err(ModeError::Operator("b".into())));
        assert_eq!(parse_mode("u=q", 0), Err(ModeError::Character('q')));
        assert!(parse_mode("u=rwx,,g=r", 0).is_err());
        assert!(parse_mode("", 0).is_err());
        assert!(parse_mode("u", 0).is_err());
    }

    #[test]
    fn symbolic_output_shows_what_is_allowed() {
        assert_eq!(symbolic_form(0o022), "u=rwx,g=rx,o=rx");
        assert_eq!(symbolic_form(0o077), "u=rwx,g=,o=");
        assert_eq!(symbolic_form(0o777), "u=,g=,o=");
        assert_eq!(symbolic_form(0o000), "u=rwx,g=rwx,o=rwx");
    }
}
