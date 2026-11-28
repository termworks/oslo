//! The definitions a script makes for itself: `alias name=value` lines, and the
//! `shopt -s expand_aliases` that turns substitution on.

use super::Scanner;
use super::scan::{is_plain_name, split_words, unquote};

impl Scanner<'_> {
    /// Record the `alias name=value` definitions a line makes, for use from the next line on.
    ///
    /// Only the literal form is recognised. `eval "alias $x=$y"` is beyond a text scan, and
    /// guessing at it would be worse than not trying: the environment's own table still answers
    /// for anything defined before this unit was parsed.
    pub(super) fn record_definitions(&mut self, line: &str) {
        let words = split_words(line);
        // `shopt -s expand_aliases` takes effect from the next line, as it does in bash.
        for window in words.windows(3) {
            if window[0] == "shopt" && window[2] == "expand_aliases" {
                match window[1].as_str() {
                    "-s" => self.expanding = true,
                    "-u" => self.expanding = false,
                    _ => {}
                }
            }
        }
        let mut rest = words.as_slice();
        // Definitions can follow `;` or `&&`, so look for `alias` anywhere a command may start.
        while let Some(position) = rest.iter().position(|w| w == "alias") {
            rest = &rest[position + 1..];
            for word in rest {
                if word == ";" || word == "&&" || word == "||" || word == "|" || word == "&" {
                    break;
                }
                if word.starts_with('-') {
                    continue;
                }
                let Some((name, value)) = word.split_once('=') else {
                    continue;
                };
                if is_plain_name(name) {
                    self.defined
                        .insert(name.to_string(), (self.line, unquote(value)));
                }
            }
        }
    }
}
