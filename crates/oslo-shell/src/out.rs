//! What `print!`, `println!`, `eprint!` and `eprintln!` reach inside this crate.
//!
//! The crate root shadows those four macros with ones that land here, so every builtin listing —
//! `declare -f`, `alias`, `trap -p`, `pwd` — writes a byte carried by `oslo_base::lossless` as
//! the byte itself rather than as its private-use stand-in.

use std::fmt::Arguments;
use std::io::Write;

/// Write `args` to standard output, each carried byte restored.
pub fn print(args: Arguments<'_>) {
    if let Err(e) = write_restored(&mut std::io::stdout().lock(), args) {
        panic!("failed printing to stdout: {e}");
    }
}

/// Write `args` to standard error, each carried byte restored.
pub fn eprint(args: Arguments<'_>) {
    if let Err(e) = write_restored(&mut std::io::stderr().lock(), args) {
        panic!("failed printing to stderr: {e}");
    }
}

fn write_restored(out: &mut impl Write, args: Arguments<'_>) -> std::io::Result<()> {
    match args.as_str() {
        Some(text) => out.write_all(&oslo_base::lossless::restore_bytes(text.as_bytes())),
        None => {
            let text = args.to_string();
            out.write_all(&oslo_base::lossless::restore_bytes(text.as_bytes()))
        }
    }
}
