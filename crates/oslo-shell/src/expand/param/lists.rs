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

/// `${name[@]@op}` over an array's `indices` and `values`, or `None` when `op` acts on each value.
pub(super) fn array(
    env: &Environment,
    name: &str,
    op: char,
    indices: &[String],
    values: &[String],
) -> Option<Listed> {
    let utf8 = locale::is_utf8(env);
    let pairs = || indices.iter().zip(values);
    Some(match op {
        'A' if env.get_array(name).is_none() => Listed {
            words: values
                .iter()
                .map(|value| format!("{name}={}", quote_reusable(value, utf8)))
                .collect(),
            single: false,
        },
        'A' => {
            let body: Vec<String> = pairs()
                .map(|(index, value)| format!("[{index}]={}", quote_element(value, utf8)))
                .collect();
            Listed {
                words: vec![
                    "declare".to_string(),
                    format!("-{}", attributes(env, name)),
                    format!("{name}=({})", body.join(" ")),
                ],
                single: false,
            }
        }
        'K' => Listed {
            words: vec![
                pairs()
                    .map(|(index, value)| format!("{index} {}", quote_element(value, utf8)))
                    .collect::<Vec<_>>()
                    .join(" "),
            ],
            single: true,
        },
        'k' => Listed {
            words: pairs()
                .flat_map(|(index, value)| [index.clone(), value.clone()])
                .collect(),
            single: false,
        },
        'a' => Listed {
            words: values.iter().map(|_| attributes(env, name)).collect(),
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
