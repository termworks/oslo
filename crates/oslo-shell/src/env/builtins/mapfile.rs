//! `mapfile` / `readarray` — read lines of input into an indexed array.
//!
//! One builtin under two names, as in bash. It exists because the loop it replaces is wrong in a
//! way that is hard to see: `while read -r line; do a+=("$line"); done < f` drops a final line
//! with no newline, mangles backslashes without `-r`, and re-splits on `IFS` if anyone forgets to
//! clear it. Reading the descriptor here does none of that.
//!
//! Bytes are read one at a time, for the same reason [`crate::env::builtins::io`] does it: a
//! buffered read would swallow input past the last delimiter, and a later command sharing the
//! descriptor (`{ mapfile -n 1 a; cat; } < f`) would find the file already drained.

use crate::env::origin_now;
use crate::env::scope::{Environment, ShellArray, is_valid_identifier};
use nix::errno::Errno;
use oslo_base::error::Result;
use std::os::fd::RawFd;

const USAGE: &str =
    "mapfile: usage: mapfile [-d delim] [-n count] [-O origin] [-s count] [-t] [-u fd] [array]";

/// The array `mapfile` fills when the caller names none. bash's default, and scripts rely on it.
const DEFAULT_ARRAY: &str = "MAPFILE";

struct Options {
    /// `-d`: the byte that ends a record. `-d ''` selects NUL, as in bash.
    delim: u8,
    /// `-n`: stop after this many records. 0 means "all of them".
    count: usize,
    /// `-O`: the index the first record is stored at.
    origin: i64,
    /// `-s`: discard this many records before storing any.
    skip: usize,
    /// `-t`: strip the delimiter from each record.
    strip: bool,
    fd: RawFd,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            delim: b'\n',
            count: 0,
            origin: 0,
            skip: 0,
            strip: false,
            fd: 0,
        }
    }
}

struct OptionError {
    message: String,
    status: i32,
}

fn usage(message: String) -> OptionError {
    OptionError { message, status: 2 }
}

fn takes_argument(flag: char) -> bool {
    matches!(flag, 'd' | 'n' | 'O' | 's' | 'u' | 'C' | 'c')
}

/// `mapfile [-d delim] [-n count] [-O origin] [-s count] [-t] [-u fd] [array]`.
///
/// `readarray` is the same builtin; the two names are registered together.
pub fn builtin_mapfile(env: &mut Environment, args: &[String]) -> Result<i32> {
    let (opts, operands) = match parse_options(args) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("{}mapfile: {}", origin_now(), e.message);
            eprintln!("{}", USAGE);
            return Ok(e.status);
        }
    };

    // bash ignores operands after the first, so `mapfile a b` fills `a` and leaves `b` alone.
    let name = operands.first().map_or(DEFAULT_ARRAY, String::as_str);
    if !is_valid_identifier(name) {
        crate::env::complain(
            args,
            name,
            &format!(
                "mapfile: `{}': not a valid identifier",
                oslo_base::shown::shown(name)
            ),
            "not a name",
            Some(
                "a name starts with a letter or underscore and continues with letters, digits or underscores",
            ),
        );
        return Ok(1);
    }

    let records = match read_records(&opts) {
        Ok(records) => records,
        Err(errno) => {
            eprintln!("{}mapfile: read error: {}", origin_now(), errno);
            return Ok(1);
        }
    };

    let mut array = ShellArray::default();
    for (offset, record) in records.into_iter().enumerate() {
        array.set(opts.origin.saturating_add(offset as i64), record);
    }
    // A read-only name is refused by the store, which reports it; the failure is `mapfile`'s.
    Ok(i32::from(!env.set_array(name, array)))
}

fn parse_options(args: &[String]) -> std::result::Result<(Options, Vec<String>), OptionError> {
    let mut opts = Options::default();
    let mut idx = 1;

    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--" {
            idx += 1;
            break;
        }
        if arg.len() < 2 || !arg.starts_with('-') {
            break;
        }
        let mut rest = &arg[1..];
        while let Some(flag) = rest.chars().next() {
            rest = &rest[flag.len_utf8()..];
            if !takes_argument(flag) {
                match flag {
                    't' => opts.strip = true,
                    _ => return Err(usage(format!("-{flag}: invalid option"))),
                }
                continue;
            }
            let value = if rest.is_empty() {
                idx += 1;
                args.get(idx)
                    .cloned()
                    .ok_or_else(|| usage(format!("-{flag}: option requires an argument")))?
            } else {
                std::mem::take(&mut rest).to_string()
            };
            apply_valued_flag(&mut opts, flag, &value)?;
            break;
        }
        idx += 1;
    }

    Ok((opts, args[idx.min(args.len())..].to_vec()))
}

fn apply_valued_flag(
    opts: &mut Options,
    flag: char,
    value: &str,
) -> std::result::Result<(), OptionError> {
    fn number<T: std::str::FromStr>(
        flag: char,
        value: &str,
    ) -> std::result::Result<T, OptionError> {
        value.parse::<T>().map_err(|_| OptionError {
            message: format!("{value}: invalid {flag} argument"),
            status: 1,
        })
    }

    match flag {
        // bash's `-d ''` is the NUL delimiter, which is what pairs with `find -print0`.
        'd' => opts.delim = value.as_bytes().first().copied().unwrap_or(0),
        'n' => opts.count = number('n', value)?,
        'O' => opts.origin = number('O', value)?,
        's' => opts.skip = number('s', value)?,
        'u' => opts.fd = number('u', value)?,
        // The callback options run shell code between records. Refused rather than ignored:
        // accepting `-C` and never calling the callback would silently drop the progress
        // reporting or the incremental processing it was there to do.
        'C' | 'c' => {
            return Err(OptionError {
                message: format!("-{flag}: the callback options are not implemented"),
                status: 2,
            });
        }
        _ => return Err(usage(format!("-{flag}: invalid option"))),
    }
    Ok(())
}

/// Read the descriptor into records, applying `-s`, `-n` and `-t`.
///
/// A final record with no delimiter is kept — that is the whole difference between this and the
/// `while read` loop it replaces.
fn read_records(opts: &Options) -> std::result::Result<Vec<String>, Errno> {
    let mut records = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let mut seen = 0usize;

    loop {
        let mut byte = [0u8; 1];
        match nix::unistd::read(opts.fd, &mut byte) {
            Ok(0) => break,
            Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(e) => return Err(e),
        }
        current.push(byte[0]);
        if byte[0] != opts.delim {
            continue;
        }
        seen += 1;
        if push_record(&mut records, &mut current, opts, seen) {
            return Ok(records);
        }
    }

    if !current.is_empty() {
        seen += 1;
        push_record(&mut records, &mut current, opts, seen);
    }
    Ok(records)
}

/// Store one finished record; `true` once `-n` is satisfied.
fn push_record(
    records: &mut Vec<String>,
    current: &mut Vec<u8>,
    opts: &Options,
    seen: usize,
) -> bool {
    let mut bytes = std::mem::take(current);
    if seen > opts.skip {
        if opts.strip && bytes.last() == Some(&opts.delim) {
            bytes.pop();
        }
        records.push(String::from_utf8_lossy(&bytes).into_owned());
    }
    opts.count != 0 && records.len() >= opts.count
}

#[cfg(test)]
mod tests {
    use super::{Options, builtin_mapfile, read_records};
    use crate::env::Environment;
    use std::io::{Seek, Write};
    use std::os::fd::AsRawFd;

    /// A temporary file holding `text`, rewound, so a test can read it as a descriptor.
    fn fed(text: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(text.as_bytes()).unwrap();
        file.as_file_mut().rewind().unwrap();
        file
    }

    fn records(text: &str, opts: Options) -> Vec<String> {
        let file = fed(text);
        let opts = Options {
            fd: file.as_file().as_raw_fd(),
            ..opts
        };
        read_records(&opts).unwrap()
    }

    /// The delimiter is part of the record unless `-t` says otherwise — bash stores `a\n`.
    #[test]
    fn records_keep_their_delimiter_until_t_strips_it() {
        assert_eq!(
            records("a\nb\n", Options::default()),
            vec!["a\n".to_string(), "b\n".to_string()]
        );
        assert_eq!(
            records(
                "a\nb\n",
                Options {
                    strip: true,
                    ..Default::default()
                }
            ),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    /// The reason this builtin exists: a last line with no newline is data, not a mistake.
    #[test]
    fn a_final_record_without_a_delimiter_is_kept() {
        assert_eq!(
            records(
                "a\nb",
                Options {
                    strip: true,
                    ..Default::default()
                }
            ),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn skip_and_count_select_a_window() {
        let opts = || Options {
            strip: true,
            ..Default::default()
        };
        assert_eq!(
            records(
                "a\nb\nc\nd\n",
                Options {
                    skip: 1,
                    count: 2,
                    ..opts()
                }
            ),
            vec!["b".to_string(), "c".to_string()]
        );
    }

    /// `-d ''` is the NUL delimiter, which is how `find -print0` output is read safely.
    #[test]
    fn a_chosen_delimiter_replaces_the_newline() {
        assert_eq!(
            records(
                "a\nb\0c\0",
                Options {
                    delim: 0,
                    strip: true,
                    ..Default::default()
                }
            ),
            vec!["a\nb".to_string(), "c".to_string()]
        );
    }

    /// `-O` decides where the first element lands, leaving lower indices untouched.
    #[test]
    fn the_origin_offsets_the_indices() {
        let file = fed("a\nb\n");
        let mut env = Environment::new();
        let args: Vec<String> = ["mapfile", "-t", "-O", "3", "-u"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut args = args;
        args.push(file.as_file().as_raw_fd().to_string());
        args.push("dest".to_string());
        assert_eq!(builtin_mapfile(&mut env, &args).unwrap(), 0);

        let array = env.get_array("dest").expect("the array was created");
        assert_eq!(array.get(3), Some("a"));
        assert_eq!(array.get(4), Some("b"));
        assert_eq!(array.get(0), None);
    }

    #[test]
    fn a_bad_name_and_a_bad_option_are_both_refused() {
        let mut env = Environment::new();
        let bad_name: Vec<String> = ["mapfile", "1bad"].iter().map(|s| s.to_string()).collect();
        assert_eq!(builtin_mapfile(&mut env, &bad_name).unwrap(), 1);
        let bad_flag: Vec<String> = ["mapfile", "-Z"].iter().map(|s| s.to_string()).collect();
        assert_eq!(builtin_mapfile(&mut env, &bad_flag).unwrap(), 2);
    }
}
