//! Programmable completion as bash's builtins see it: `complete`, `compopt` and the specs they
//! store, which `compgen` shares the option grammar and the generator with.
//!
//! The specs are a table of their own. oslo's line editor completes through `oslo-ui`'s providers
//! and does not consult them; these builtins store, print and remove specs exactly as bash does,
//! so a script or a `bash-completion` file that registers them runs without error.

pub(super) mod generate;

use crate::env::origin_now;
use crate::env::scope::Environment;
use oslo_base::error::Result;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// Every action, in bash's bit order, with the letter that selects it where there is one.
pub(super) const ACTIONS: &[(&str, Option<char>)] = &[
    ("alias", Some('a')),
    ("arrayvar", None),
    ("binding", None),
    ("builtin", Some('b')),
    ("command", Some('c')),
    ("directory", Some('d')),
    ("disabled", None),
    ("enabled", None),
    ("export", Some('e')),
    ("file", Some('f')),
    ("function", None),
    ("group", Some('g')),
    ("helptopic", None),
    ("hostname", None),
    ("job", Some('j')),
    ("keyword", Some('k')),
    ("running", None),
    ("service", Some('s')),
    ("setopt", None),
    ("shopt", None),
    ("signal", None),
    ("stopped", None),
    ("user", Some('u')),
    ("variable", Some('v')),
];

/// `-o` option names, in the order `complete -p` prints them.
pub(super) const OPTIONS: &[&str] = &[
    "bashdefault",
    "default",
    "dirnames",
    "filenames",
    "fullquote",
    "noquote",
    "nosort",
    "nospace",
    "plusdirs",
];

/// The table keys of `-D`, `-E` and `-I`, which name no command.
const SPECIAL_KEYS: &[(char, &str)] = &[
    ('D', "_DefaultCmD_"),
    ('E', "_EmptycmD_"),
    ('I', "_InitialWorD_"),
];

/// bash's bucket count for this table, which decides the order `complete -p` lists it in.
const BUCKETS: u32 = 512;

/// One completion specification: what to generate, and how to shape it.
#[derive(Clone, Default)]
pub(super) struct Spec {
    pub actions: u32,
    pub options: u32,
    pub glob: Option<String>,
    pub words: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub filter: Option<String>,
    pub command: Option<String>,
    pub function: Option<String>,
}

impl Spec {
    pub fn has(&self, action: &str) -> bool {
        bit(ACTIONS.iter().map(|(name, _)| *name), action).is_some_and(|b| self.actions & b != 0)
    }

    pub fn option(&self, name: &str) -> bool {
        bit(OPTIONS.iter().copied(), name).is_some_and(|b| self.options & b != 0)
    }
}

fn bit<'a>(mut names: impl Iterator<Item = &'a str>, name: &str) -> Option<u32> {
    names.position(|n| n == name).map(|at| 1 << at)
}

/// The command line of `complete`, `compgen` or `compopt`, parsed.
#[derive(Default)]
pub(super) struct Parsed {
    pub spec: Spec,
    /// `+o` options, for `compopt`.
    pub cleared: u32,
    pub print: bool,
    pub remove: bool,
    /// The `-D`, `-E` and `-I` keys asked for.
    pub keys: Vec<&'static str>,
    /// `compgen -V name`.
    pub var: Option<String>,
    pub operands: Vec<String>,
}

/// Parse `args` for builtin `cmd`, accepting the option letters in `letters`.
///
/// `Err` is the status of a usage error, already reported, with `usage` under a bad option.
pub(super) fn parse(
    cmd: &str,
    args: &[String],
    letters: &str,
    usage: &str,
) -> std::result::Result<Parsed, i32> {
    let mut parsed = Parsed::default();
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        let plus = arg.starts_with('+') && letters.contains('+');
        if arg.len() < 2 || !(arg.starts_with('-') || plus) {
            break;
        }
        let mut at = 1;
        while at < arg.len() {
            let c = arg[at..].chars().next().unwrap_or_default();
            at += c.len_utf8();
            if !letters.contains(c) || c == '+' {
                let body = format!("{cmd}: -{c}: invalid option");
                crate::env::complain_option(args, c, &body, usage);
                return Err(2);
            }
            if "oAGWPSXFCV".contains(c) {
                let value = match at < arg.len() {
                    true => arg[at..].to_string(),
                    false => {
                        i += 1;
                        match args.get(i) {
                            Some(value) => value.clone(),
                            None => {
                                let body = format!("{cmd}: -{c}: option requires an argument");
                                crate::env::complain_option(args, c, &body, usage);
                                return Err(2);
                            }
                        }
                    }
                };
                take_value((cmd, args), &mut parsed, c, plus, value)?;
                break;
            }
            take_flag(&mut parsed, c);
        }
        i += 1;
    }
    parsed.operands = args[i.min(args.len())..].to_vec();
    Ok(parsed)
}

fn take_flag(parsed: &mut Parsed, c: char) {
    match c {
        'p' => parsed.print = true,
        'r' => parsed.remove = true,
        'D' | 'E' | 'I' => {
            if let Some((_, key)) = SPECIAL_KEYS.iter().find(|(letter, _)| *letter == c) {
                parsed.keys.push(key);
            }
        }
        letter => {
            if let Some(at) = ACTIONS.iter().position(|(_, l)| *l == Some(letter)) {
                parsed.spec.actions |= 1 << at;
            }
        }
    }
}

fn take_value(
    (cmd, args): (&str, &[String]),
    parsed: &mut Parsed,
    c: char,
    plus: bool,
    value: String,
) -> std::result::Result<(), i32> {
    let spec = &mut parsed.spec;
    match c {
        'o' => {
            let Some(b) = bit(OPTIONS.iter().copied(), &value) else {
                report(
                    args,
                    &value,
                    &format!("{cmd}: {value}: invalid option name"),
                );
                return Err(2);
            };
            match plus {
                true => parsed.cleared |= b,
                false => spec.options |= b,
            }
        }
        'A' => {
            let Some(b) = bit(ACTIONS.iter().map(|(name, _)| *name), &value) else {
                report(
                    args,
                    &value,
                    &format!("{cmd}: {value}: invalid action name"),
                );
                return Err(2);
            };
            spec.actions |= b;
        }
        'G' => spec.glob = Some(value),
        'W' => spec.words = Some(value),
        'P' => spec.prefix = Some(value),
        'S' => spec.suffix = Some(value),
        'X' => spec.filter = Some(value),
        'F' => spec.function = Some(value),
        'C' => spec.command = Some(value),
        _ => parsed.var = Some(value),
    }
    Ok(())
}

/// Report `message` about `word`, pointing at it where `args` holds it.
fn report(args: &[String], word: &str, message: &str) {
    crate::env::complain(args, word, message, "here", None);
}

thread_local! {
    static SPECS: RefCell<HashMap<String, (Spec, u64)>> = RefCell::new(HashMap::new());
    static SEQ: Cell<u64> = const { Cell::new(0) };
}

/// The specs as `(key, spec)`, in the order bash walks its table.
fn in_bash_order() -> Vec<(String, Spec)> {
    SPECS.with(|specs| {
        let specs = specs.borrow();
        let mut all: Vec<(&String, &(Spec, u64))> = specs.iter().collect();
        all.sort_by_key(|(key, (_, seq))| {
            (
                super::hash::fnv(key) & (BUCKETS - 1),
                std::cmp::Reverse(*seq),
            )
        });
        all.into_iter()
            .map(|(key, (spec, _))| (key.clone(), spec.clone()))
            .collect()
    })
}

fn store(key: &str, spec: Spec) {
    let seq = SEQ.with(|seq| {
        seq.set(seq.get() + 1);
        seq.get()
    });
    SPECS.with(|specs| specs.borrow_mut().insert(key.to_string(), (spec, seq)));
}

fn lookup(key: &str) -> Option<Spec> {
    SPECS.with(|specs| specs.borrow().get(key).map(|(spec, _)| spec.clone()))
}

/// `'text'`, with embedded quotes as `'\''`.
pub(super) fn single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The key as `complete -p` writes it at the end of the line.
fn shown_key(key: &str) -> String {
    match SPECIAL_KEYS.iter().find(|(_, k)| *k == key) {
        Some((letter, _)) => format!("-{letter}"),
        None => key.to_string(),
    }
}

/// One spec, as the `complete` command that would recreate it.
fn print_spec(key: &str, spec: &Spec) {
    let mut line = String::from("complete ");
    for (at, name) in OPTIONS.iter().enumerate() {
        if spec.options & (1 << at) != 0 {
            line.push_str(&format!("-o {name} "));
        }
    }
    for (at, (_, letter)) in ACTIONS.iter().enumerate() {
        if let Some(letter) = letter
            && spec.actions & (1 << at) != 0
        {
            line.push_str(&format!("-{letter} "));
        }
    }
    for (at, (name, letter)) in ACTIONS.iter().enumerate() {
        if letter.is_none() && spec.actions & (1 << at) != 0 {
            line.push_str(&format!("-A {name} "));
        }
    }
    let quoted = [
        ("-G", &spec.glob),
        ("-W", &spec.words),
        ("-P", &spec.prefix),
        ("-S", &spec.suffix),
        ("-X", &spec.filter),
        ("-C", &spec.command),
    ];
    for (flag, value) in quoted {
        if let Some(value) = value {
            line.push_str(&format!("{flag} {} ", single_quote(value)));
        }
    }
    if let Some(function) = &spec.function {
        line.push_str(&format!("-F {function} "));
    }
    println!("{line}{}", shown_key(key));
}

/// `complete [-abcdefgjksuv] [-pr] [-DEI] [-o option] [-A action] [-G glob] [-W words] …`.
pub fn builtin_complete(_env: &mut Environment, args: &[String]) -> Result<i32> {
    let usage = "complete: usage: complete [-abcdefgjksuv] [-pr] [-DEI] [-o option] [-A action] \
                 [-G globpat] [-W wordlist] [-F function] [-C command] [-X filterpat] \
                 [-P prefix] [-S suffix] [name ...]";
    let parsed = match parse("complete", args, "abcdefgjksuvprDEIoAGWPSXFC", usage) {
        Ok(parsed) => parsed,
        Err(status) => return Ok(status),
    };
    let mut keys: Vec<String> = parsed.keys.iter().map(|k| k.to_string()).collect();
    keys.extend(parsed.operands.iter().cloned());
    let defines = args.len() > 1 && !parsed.print && !parsed.remove;

    if parsed.remove && !parsed.print {
        if keys.is_empty() {
            SPECS.with(|specs| specs.borrow_mut().clear());
            return Ok(0);
        }
        let mut status = 0;
        for key in &keys {
            if SPECS.with(|specs| specs.borrow_mut().remove(key)).is_none() {
                let body = format!("complete: {}: no completion specification", shown_key(key));
                report(args, key, &body);
                status = 1;
            }
        }
        return Ok(status);
    }
    if !defines || parsed.print {
        if keys.is_empty() {
            for (key, spec) in in_bash_order() {
                print_spec(&key, &spec);
            }
            return Ok(0);
        }
        let mut status = 0;
        for key in &keys {
            match lookup(key) {
                Some(spec) => print_spec(key, &spec),
                None => {
                    let body = format!("complete: {}: no completion specification", shown_key(key));
                    report(args, key, &body);
                    status = 1;
                }
            }
        }
        return Ok(status);
    }
    for key in &keys {
        store(key, parsed.spec.clone());
    }
    Ok(0)
}

/// `compopt [-o|+o option] [-DEI] [name …]`.
pub fn builtin_compopt(_env: &mut Environment, args: &[String]) -> Result<i32> {
    let usage = "compopt: usage: compopt [-o|+o option] [-DEI] [name ...]";
    let parsed = match parse("compopt", args, "o+DEI", usage) {
        Ok(parsed) => parsed,
        Err(status) => return Ok(status),
    };
    let mut keys: Vec<String> = parsed.keys.iter().map(|k| k.to_string()).collect();
    keys.extend(parsed.operands.iter().cloned());
    if keys.is_empty() {
        eprintln!(
            "{}compopt: not currently executing completion function",
            origin_now()
        );
        return Ok(1);
    }
    let mut status = 0;
    for key in &keys {
        let Some(mut spec) = lookup(key) else {
            let body = format!("compopt: {}: no completion specification", shown_key(key));
            report(args, key, &body);
            status = 1;
            continue;
        };
        if parsed.spec.options == 0 && parsed.cleared == 0 {
            let mut line = String::from("compopt ");
            for (at, name) in OPTIONS.iter().enumerate() {
                let sign = if spec.options & (1 << at) != 0 {
                    '-'
                } else {
                    '+'
                };
                line.push_str(&format!("{sign}o {name} "));
            }
            println!("{line}{}", shown_key(key));
            continue;
        }
        spec.options = (spec.options | parsed.spec.options) & !parsed.cleared;
        SPECS.with(|specs| {
            if let Some(entry) = specs.borrow_mut().get_mut(key) {
                entry.0 = spec;
            }
        });
    }
    Ok(status)
}
