//! The transformations that read a whole list rather than each value: `A`, `K`, `k` and `a` on
//! `${a[@]}` and `$@`.
//!
//! ```text
//!   a=(x 'y z')
//!   "${a[@]@A}"   declare  -a  a=([0]="x" [1]="y z")     three words
//!   "${a[@]@K}"   0 "x" 1 "y z"                           one word
//!   "${a[@]@k}"   0  x  1  y z                            one word per key and per value
//!   "${@@A}"      set  --  'x'  'y z'
//! ```

use super::transform::{attributes, quote_element, quote_reusable};
use crate::env::Environment;
use crate::expand::locale;

/// What a list transformation produced: its words, and whether they are always one field.
pub(super) struct Listed {
    pub words: Vec<String>,
    pub single: bool,
}

/// `${name[@]@op}` over an array's `indices` (an associative array's keys) and `values`, or
/// `None` when `op` acts on each value. A nameref is read through to the array it names.
pub(super) fn array(
    env: &Environment,
    name: &str,
    op: char,
    indices: &[String],
    values: &[String],
) -> Option<Listed> {
    let utf8 = locale::is_utf8(env);
    let target = env.ref_target_name(name).into_owned();
    let var = env.variable(&target);
    let shape = var.map(|v| (v.indexed().is_some(), v.assoc().is_some()));
    let pairs = || indices.iter().zip(values);
    let none = || Listed {
        words: Vec::new(),
        single: false,
    };
    Some(match (op, shape) {
        ('A', Some((false, false)) | None) => Listed {
            words: values
                .iter()
                .map(|value| match attributes(env, &target) {
                    flags if flags.is_empty() => {
                        format!("{target}={}", quote_reusable(value, utf8))
                    }
                    flags => format!("declare -{flags} {target}={}", quote_reusable(value, utf8)),
                })
                .collect(),
            single: false,
        },
        ('A', Some(_)) => {
            let var = var?;
            let rest = match crate::env::builtins::declare::assignment(var, utf8) {
                Some(body) => format!("{target}={body}"),
                None => target.clone(),
            };
            Listed {
                words: vec![
                    "declare".to_string(),
                    format!("-{}", attributes(env, &target)),
                    rest,
                ],
                single: false,
            }
        }
        ('K', _) if values.is_empty() => none(),
        // A scalar is not an array, so it has no keys to show: its value, quoted.
        ('K', Some((false, false)) | None) => Listed {
            words: values.iter().map(|v| quote_reusable(v, utf8)).collect(),
            single: false,
        },
        // An associative array quotes its keys and ends every pair with a space, as bash does.
        ('K', Some((_, true))) => Listed {
            words: vec![
                pairs()
                    .map(|(key, value)| {
                        let key = crate::env::builtins::declare::quote_key(key, utf8);
                        format!("{key} {} ", quote_element(value, utf8))
                    })
                    .collect(),
            ],
            single: true,
        },
        ('K', _) => Listed {
            words: vec![
                pairs()
                    .map(|(index, value)| format!("{index} {}", quote_element(value, utf8)))
                    .collect::<Vec<_>>()
                    .join(" "),
            ],
            single: true,
        },
        ('k', _) => Listed {
            words: pairs()
                .flat_map(|(index, value)| [index.clone(), value.clone()])
                .collect(),
            single: false,
        },
        // A declared array with no value still answers its attributes, once.
        ('a', _) if values.is_empty() && var.is_some_and(|v| !v.is_set()) => Listed {
            words: vec![attributes(env, &target)],
            single: false,
        },
        ('a', _) => Listed {
            words: values.iter().map(|_| attributes(env, &target)).collect(),
            single: false,
        },
        _ => return None,
    })
}

/// `${@@op}` over the positional parameters, or `None` when `op` acts on each value.
pub(super) fn positional(env: &Environment, op: char, values: &[String]) -> Option<Listed> {
    let utf8 = locale::is_utf8(env);
    let quoted = || values.iter().map(|value| quote_reusable(value, utf8));
    Some(match op {
        'A' => Listed {
            words: ["set".to_string(), "--".to_string()]
                .into_iter()
                .chain(quoted())
                .collect(),
            single: false,
        },
        'K' | 'k' => Listed {
            words: quoted().collect(),
            single: false,
        },
        'a' => Listed {
            words: values.iter().map(|_| String::new()).collect(),
            single: false,
        },
        _ => return None,
    })
}
