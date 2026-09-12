//! The two lists that make the differential suite a ratchet.
//!
//! `EXPECTED_FAIL` names every corpus case oslo currently gets wrong, with the PLAN.md finding
//! that explains it. Two rules keep the list from rotting:
//!
//! * a case that is not listed and does not match bash fails the suite — no new divergence
//!   lands unnoticed;
//! * a case that is listed and *does* match bash also fails the suite — so closing a finding
//!   means deleting its line here, and a round is done when its entries are gone.
//!
//! `UNFILED` is not a plan ID: it marks a divergence the audit did not enumerate. Those are
//! genuine bugs with no scheduled owner yet.
//!
//! `BRUSH` marks a divergence that lives in the parser oslo depends on rather than in oslo. The
//! distinction matters when reading the list: those rows cannot be closed by changing oslo, only
//! by an upstream fix, a workaround ahead of the parser, or vendoring — and each is a decision
//! rather than a bug to schedule.
//!
//! `KNOWN_DIVERGENT` is the escape hatch for cases where bash is not a valid oracle at all.
//! Entries there are skipped, never compared, and each one needs a reason. Keep the two lists
//! separate: "we are wrong" and "the comparison is meaningless" are different claims.

/// Corpus file, PLAN.md finding ID, and what oslo does instead.
///
/// One row per line, deliberately: closing a finding is a one-line deletion, and rustfmt would
/// otherwise wrap each row across four lines and hide that.
#[rustfmt::skip]
pub const EXPECTED_FAIL: &[(&str, &str, &str)] = &[

    // --- Round 1: hangs, crashes, and data executed as code ---
    // Empty: every Round 1 finding now matches bash.

    // --- Round 2: quoting, fields, and parameter expansion ---
    // Empty: the expansion operators match bash, and so does the status a fatal expansion error
    // leaves a non-interactive shell with (127, decided by `ShellError::fatal_exit_status`).

    // --- Round 3: arithmetic ---
    // Empty: the operator ladder matches bash, and a fatal arithmetic error exits 127 with it.

    // --- Round 4: exit status, descriptors, and subshell state ---
    // Empty: exit codes survive subshells, pipeline stages and background jobs; `$?` is written
    // after every pipeline; a signalled child reports 128 + signo.

    // --- Round 5: builtins conformance ---
    // Empty: `readonly` now refuses the assignment *and* reports it. `builtin_readonly.sh` needed
    // both halves of R11 to close — R5.10's status propagation, and the differential harness
    // finally running oslo in the same `--posix` mode it was giving the oracle.

    // --- Round 6: shell options and traps ---
    // Empty: every option in the `set -o` table that has behaviour now acts on it, and traps are
    // both stored and run — the EXIT handler on every exit path, signals at command boundaries.

    // --- Round 7: job control ---
    // Empty for `wait` (R7.5): a pid operand reports the child's own status, a signalled child
    // reports 128 + signo, an unknown pid is 127, and `-n` and `%n` operands work. What remains
    // in Round 7 needs a pty and is covered by the job-control integration tests instead.

    // --- Round 8: missing language features ---
    // **`for ((;;))` used to be here and is now closed.** The vendored parser's tokenizer took the
    // longest match and fused the two section separators into the single `;;` that terminates a
    // `case` item, so its `arithmetic_for_clause` rule never saw the two `;` it asked for. Fixing
    // it in that grammar meant carrying a patch across 10,181 vendored lines. `rune` reads the
    // head of a `for ((…))` as text and splits it on `;` itself, so the unspaced form parses like
    // any other — which is the kind of thing owning the parser was for.
    ("syntax_unsupported_coproc.sh", "R8.5", "coproc is refused by name and deliberately not implemented — it needs job control; bash runs the body and exits 0"),
    ("syntax_unsupported_select.sh", "R8.6", "select is refused by name and deliberately not implemented — it needs a prompt, PS3 and REPLY; bash runs the loop and reads EOF"),

    // --- the tokenizer ---
    // Empty. The comment-after-an-odd-number-of-blanks bug that lived here belonged to the
    // vendored parser and went with it: a comment inside `$( … )` was recognised only when an
    // *even* number of blanks preceded its `#`, so a quote in the comment's text was never closed.
    // `rune` reads a comment wherever a word could start and nowhere else, which is the rule, and
    // `comment_in_command_substitution.sh` matches bash.

    // --- divergences the audit did not enumerate ---
    // `robust_special_builtin_failure.sh` closed with R11's C4. It needed three things at once —
    // a `--posix` flag, a differential harness that gives oslo the same mode it gives the oracle,
    // and a builtin that reports a *utility* error rather than a non-zero status, so that
    // `export "=1"` is fatal where `shift 5` is not.
    //
    // Three divergences were found by running every `#!/bin/sh` script on a
    // Debian system under both oslo and dash, and all three are fixed: a comment inside a `$( … )`
    // inside a heredoc body, `printf`'s missing `%*` width, and the errexit exemption being lost
    // when a short-circuited AND-OR list was the last command of a compound.
    ("loop_break_in_condition.sh", "UNFILED", "a function named parse is dispatched to the structured parse tool when it receives several arguments"),
];

/// Corpus file and why bash cannot arbitrate it. Empty is the healthy state.
pub const KNOWN_DIVERGENT: &[(&str, &str)] = &[];
