//! Compiling and matching one shell pattern.
//!
//! Split out from pathname expansion because the same dialect answers four different questions —
//! `case`, `[[ x == p ]]`, the `${v#p}` family and the filesystem walk — and only the last of them
//! is about paths. What they share is the thing that makes a *shell* pattern different from a
//! glob: **quoting decides, per character, whether a metacharacter is one**.
//!
//! That is not a detail. `case $answer in "$expected") ok ;; esac` is an ordinary way to compare
//! two strings, and a matcher that reads the expanded `$expected` as a pattern answers "yes" to
//! `case anything in "*")`. A shell-level caller therefore compiles from runs that still carry the
//! quoting — `ShellPattern::from_chars` is the seam — rather than from a flat `String` that has
//! forgotten it.
//!
//! **It lives in this crate because a fifth caller wanted it: the prompt.** Tab on `rm /one/tw*`
//! has to know what that matches, and the interface layer cannot see the shell. One matcher
//! answering both is the only arrangement in which what completion offers and what the shell then
//! expands cannot disagree.

pub mod collate;
pub mod walk;

/// One element of a compiled pattern.
#[derive(Debug, PartialEq, Eq)]
pub enum Item {
    /// A character that must match itself — either typed literally or quoted into submission.
    Ch(char),
    /// `?`: exactly one character.
    Any,
    /// `*`: any run of characters.
    Star,
    /// `[abc]`, `[a-z]`, `[!abc]`.
    Class { negated: bool, members: Vec<Member> },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Member {
    Ch(char),
    Range(char, char),
    /// A POSIX character class: the `alpha` of `[[:alpha:]]`.
    Named(String),
}

/// Whether `ch` belongs to the POSIX character class `name`.
///
/// An unknown name matches nothing rather than aborting: a shell has no way to report a bad
/// pattern, and the word will fall back to its literal text if this was its only chance to match.
fn in_named_class(name: &str, ch: char) -> bool {
    match name {
        "alnum" => ch.is_alphanumeric(),
        "alpha" => ch.is_alphabetic(),
        "blank" => ch == ' ' || ch == '\t',
        "cntrl" => ch.is_control(),
        "digit" => ch.is_ascii_digit(),
        "graph" => !ch.is_control() && !ch.is_whitespace(),
        "lower" => ch.is_lowercase(),
        "print" => !ch.is_control(),
        "punct" => ch.is_ascii_punctuation(),
        "space" => ch.is_whitespace(),
        "upper" => ch.is_uppercase(),
        "xdigit" => ch.is_ascii_hexdigit(),
        _ => false,
    }
}

impl Item {
    pub fn matches_char(&self, ch: char) -> bool {
        self.matches_char_with(ch, false)
    }

    /// `nocase` is `nocaseglob` and `nocasematch`: a letter also matches its other case, in a
    /// bracket as well as out of one.
    pub fn matches_char_with(&self, ch: char, nocase: bool) -> bool {
        match self {
            Item::Ch(c) => *c == ch || (nocase && same_letter(*c, ch)),
            Item::Any => true,
            // `*` is handled by the outer loop; it never consumes a single character on its own.
            Item::Star => false,
            Item::Class { negated, members } => {
                let hit = members.iter().any(|m| {
                    member_has(m, ch)
                        || (nocase
                            && ch
                                .to_lowercase()
                                .chain(ch.to_uppercase())
                                .any(|other| member_has(m, other)))
                });
                hit != *negated
            }
        }
    }
}

fn member_has(member: &Member, ch: char) -> bool {
    match member {
        Member::Ch(c) => *c == ch,
        Member::Range(lo, hi) => *lo <= ch && ch <= *hi,
        Member::Named(name) => in_named_class(name, ch),
    }
}

fn same_letter(a: char, b: char) -> bool {
    a.to_lowercase().eq(b.to_lowercase())
}

/// Compile a run of `(character, is-it-a-metacharacter)` pairs, reporting whether any character
/// turned out to be a metacharacter.
///
/// The flag is what lets pathname expansion skip the directory walk for a word that only *looks*
/// like a pattern, and what makes an unterminated `[` fall back to literal text.
pub fn compile_items(chars: &[(char, bool)]) -> (Vec<Item>, bool) {
    let mut items = Vec::new();
    let mut has_metacharacter = false;
    let mut i = 0;

    while i < chars.len() {
        let (ch, globs) = chars[i];
        // **An unquoted backslash escapes the next character.** Source-level backslashes are
        // quoting and never arrive here live; one that does came out of an unquoted expansion —
        // `v='a\*'; case 'a*' in $v)` — and bash 5.2 reads it as an escape, not a character.
        if globs
            && ch == '\\'
            && let Some(&(next, _)) = chars.get(i + 1)
        {
            items.push(Item::Ch(next));
            i += 2;
            continue;
        }
        if globs {
            match ch {
                '*' => {
                    // POSIX has no globstar: `**` is just `*`, and two adjacent stars would only
                    // make the matcher backtrack for nothing.
                    if items.last() != Some(&Item::Star) {
                        items.push(Item::Star);
                    }
                    has_metacharacter = true;
                    i += 1;
                    continue;
                }
                '?' => {
                    items.push(Item::Any);
                    has_metacharacter = true;
                    i += 1;
                    continue;
                }
                '[' => {
                    if let Some((class, next)) = parse_class(chars, i) {
                        items.push(class);
                        has_metacharacter = true;
                        i = next;
                        continue;
                    }
                }
                _ => {}
            }
        }
        items.push(Item::Ch(ch));
        i += 1;
    }

    (items, has_metacharacter)
}

/// Parse `[...]` starting at `start`, or return `None` when it is never closed.
///
/// An unclosed `[` is a literal `[` — `echo a[` prints `a[` rather than failing — so the caller
/// needs to be able to back out. A `]` in the first position is a member, not the terminator.
fn parse_class(chars: &[(char, bool)], start: usize) -> Option<(Item, usize)> {
    let mut i = start + 1;
    let mut negated = false;
    // `globs` — the flag saying this character is still an active metacharacter — is what decides
    // it, exactly as it does for `]` below. A *quoted* `!` is a member and not a negation:
    // `[\!a]` is the set containing `!` and `a`, so `a` matches it. Ignoring the flag here read
    // that as "anything but `a`" and answered the opposite of bash on every one of them.
    if let Some(&(ch, globs)) = chars.get(i)
        && globs
        && (ch == '!' || ch == '^')
    {
        negated = true;
        i += 1;
    }

    let mut members = Vec::new();
    while i < chars.len() {
        let (ch, globs) = chars[i];
        // A quoted `]` is data; only an unquoted one can close the class.
        if ch == ']' && globs && !members.is_empty() {
            return Some((Item::Class { negated, members }, i + 1));
        }
        // An escaped character is a member, whatever it is: `[a\-c]` is three characters, not a
        // range, and `[\]]` holds a `]`.
        if globs
            && ch == '\\'
            && let Some(&(next, _)) = chars.get(i + 1)
        {
            members.push(Member::Ch(next));
            i += 2;
            continue;
        }
        // `[[:digit:]]` nests a named class inside the bracket, so its `]` is not the closer.
        if let Some((name, next)) = parse_named_class(chars, i) {
            members.push(Member::Named(name));
            i = next;
            continue;
        }
        // `[[.a.]]` and `[[=e=]]`: a collating symbol and an equivalence class.
        if let Some((member, next)) = parse_symbol(chars, i) {
            members.push(member);
            i = next;
            continue;
        }
        // `a-z` is a range, but the `-` in `[a-]` is an ordinary member.
        if let (Some(&('-', true)), Some(&(hi, hi_globs))) = (chars.get(i + 1), chars.get(i + 2))
            && !(hi == ']' && hi_globs)
        {
            members.push(Member::Range(ch, hi));
            i += 3;
            continue;
        }
        members.push(Member::Ch(ch));
        i += 1;
    }
    None
}

/// Parse `[:name:]` at `start`, returning the name and the index just past the closing `]`.
fn parse_named_class(chars: &[(char, bool)], start: usize) -> Option<(String, usize)> {
    if chars.get(start)?.0 != '[' || chars.get(start + 1)?.0 != ':' {
        return None;
    }
    let mut name = String::new();
    let mut i = start + 2;
    while let Some(&(ch, _)) = chars.get(i) {
        if ch == ':' && chars.get(i + 1).map(|c| c.0) == Some(']') {
            return Some((name, i + 2));
        }
        name.push(ch);
        i += 1;
    }
    None
}

/// Parse `[.x.]` or `[=x=]` at `start`: the member, and the index just past its closing `]`.
///
/// Only single characters are named, and both forms name exactly that character. An equivalence
/// class could in principle take in accented forms, but bash 5.3 in `en_US.UTF-8` answers
/// `case émile in [[=e=]]mile)` with no, so `[=e=]` is `e` and nothing else.
fn parse_symbol(chars: &[(char, bool)], start: usize) -> Option<(Member, usize)> {
    if chars.get(start)?.0 != '[' {
        return None;
    }
    let kind = chars.get(start + 1)?.0;
    if kind != '.' && kind != '=' {
        return None;
    }
    let &(named, _) = chars.get(start + 2)?;
    if chars.get(start + 3)?.0 != kind || chars.get(start + 4)?.0 != ']' {
        return None;
    }
    Some((Member::Ch(named), start + 5))
}

/// Whether the whole of `name` matches a compiled item list.
///
/// Backtracking on the most recent `*` only: shell patterns have no alternation, so one
/// resumption point is enough and the match stays linear in practice.
pub fn matches_items(items: &[Item], name: &str) -> bool {
    matches_items_with(items, name, false)
}

/// [`matches_items`], case-insensitively when `nocase`.
pub fn matches_items_with(items: &[Item], name: &str, nocase: bool) -> bool {
    let name: Vec<char> = name.chars().collect();
    let (mut i, mut j) = (0, 0);
    // The most recent `*` and how much of the name it had swallowed, for backtracking.
    let mut star: Option<(usize, usize)> = None;

    while j < name.len() {
        match items.get(i) {
            Some(Item::Star) => {
                star = Some((i, j));
                i += 1;
            }
            Some(item) if item.matches_char_with(name[j], nocase) => {
                i += 1;
                j += 1;
            }
            _ => match star {
                // Let the star eat one more character and try the rest of the pattern again.
                Some((star_index, eaten)) => {
                    i = star_index + 1;
                    j = eaten + 1;
                    star = Some((star_index, j));
                }
                None => return false,
            },
        }
    }

    while items.get(i) == Some(&Item::Star) {
        i += 1;
    }
    i == items.len()
}

/// A compiled shell pattern, ready to be matched against as many strings as the caller likes.
///
/// `${v##*/}` tries every prefix of the value and `${v//p/r}` every position in it, so compiling
/// once and matching many times is not an optimisation but the difference between a linear and a
/// quadratic operator.
#[derive(Debug)]
pub struct ShellPattern {
    items: Vec<Item>,
    /// The plain text this pattern is, when it holds no metacharacter at all.
    ///
    /// **So a search can be a search.** `${v//a/b}` is by far the commonest replacement and has no
    /// pattern in it; answering it with `str::find` rather than by testing every prefix at every
    /// position is the difference between linear and cubic — measured, before this: 250 bytes took
    /// 45 ms, 2 KB took eleven seconds, and 20 KB did not finish.
    literal: Option<String>,
    /// How many *characters* a match consumes, when every match is the same length.
    ///
    /// Everything but `*` consumes exactly one character, so a pattern without one has a fixed
    /// width and there is exactly one end worth testing at each position — rather than every end
    /// from the longest down.
    fixed_chars: Option<usize>,
}

/// The two shortcuts, read off the compiled items once.
fn shortcuts(items: &[Item]) -> (Option<String>, Option<usize>) {
    let literal = items
        .iter()
        .map(|item| match item {
            Item::Ch(ch) => Some(*ch),
            _ => None,
        })
        .collect::<Option<String>>();
    let fixed = (!items.iter().any(|item| matches!(item, Item::Star))).then_some(items.len());
    (literal, fixed)
}

impl ShellPattern {
    /// The plain text this pattern is, if it is not a pattern at all.
    pub fn literal(&self) -> Option<&str> {
        self.literal.as_deref()
    }

    /// How many characters any match consumes, when that is the same for every match.
    pub fn fixed_chars(&self) -> Option<usize> {
        self.fixed_chars
    }
}

impl ShellPattern {
    /// Compile from characters that each say whether they were quoted.
    ///
    /// The seam a shell-level caller builds on: it is what makes `case $x in "$p")` a string
    /// comparison and `case $x in $p)` a pattern match. `true` means the character is a
    /// metacharacter if it looks like one.
    pub fn from_chars(chars: &[(char, bool)]) -> Self {
        Self::of(compile_items(chars).0)
    }

    /// Build from compiled items, reading the shortcuts off them once.
    fn of(items: Vec<Item>) -> Self {
        let (literal, fixed_chars) = shortcuts(&items);
        Self {
            items,
            literal,
            fixed_chars,
        }
    }

    /// Compile text in which every metacharacter is meant as one.
    ///
    /// For callers that never had the quoting to begin with — `[[ x == p ]]` receives its right
    /// operand as an already-flattened string — and for tests.
    pub fn from_unquoted(text: &str) -> Self {
        let chars: Vec<(char, bool)> = text.chars().map(|ch| (ch, true)).collect();
        Self::of(compile_items(&chars).0)
    }

    /// Does the whole of `text` match?
    pub fn matches(&self, text: &str) -> bool {
        matches_items(&self.items, text)
    }

    /// [`Self::matches`], ignoring case when `nocase` — `nocasematch` for `case` and `[[ ]]`.
    pub fn matches_case(&self, text: &str, nocase: bool) -> bool {
        match nocase {
            true => matches_items_with(&self.items, text, true),
            false => self.matches(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ShellPattern;

    /// Characters that were quoted, then characters that were not.
    fn quoted_then(literal: &str, pattern: &str) -> ShellPattern {
        let chars: Vec<(char, bool)> = literal
            .chars()
            .map(|c| (c, false))
            .chain(pattern.chars().map(|c| (c, true)))
            .collect();
        ShellPattern::from_chars(&chars)
    }

    #[test]
    fn quoting_turns_a_metacharacter_into_a_character() {
        // The defect that motivated compiling from runs: `case abc in "*")` matched everything.
        let quoted = ShellPattern::from_chars(&[('*', false)]);
        assert!(quoted.matches("*"));
        assert!(!quoted.matches("abc"));

        let unquoted = ShellPattern::from_unquoted("*");
        assert!(unquoted.matches("abc"));
    }

    /// One word can be part quoted and part not, and each part keeps its own answer.
    #[test]
    fn quoting_is_decided_per_character() {
        let p = quoted_then("a*", "?");
        assert!(p.matches("a*z"));
        assert!(!p.matches("abz"));
    }

    /// A `]` that arrived quoted is a member of the bracket expression, not its terminator.
    /// modernish rejects a shell that gets this wrong before it will run at all.
    #[test]
    fn a_quoted_bracket_close_is_a_member() {
        // `*[` and `]*` were typed; `ab]cd` arrived quoted, so its `]` is data.
        let mut chars: Vec<(char, bool)> = "*[".chars().map(|c| (c, true)).collect();
        chars.extend("ab]cd".chars().map(|c| (c, false)));
        chars.extend("]*".chars().map(|c| (c, true)));
        let p = ShellPattern::from_chars(&chars);
        assert!(p.matches("c"));
        assert!(p.matches("x]y"));
        assert!(!p.matches("z"));
    }

    #[test]
    fn an_unterminated_bracket_is_literal_text() {
        let p = ShellPattern::from_unquoted("a[b");
        assert!(p.matches("a[b"));
        assert!(!p.matches("ab"));
    }

    #[test]
    fn classes_ranges_and_negation() {
        assert!(ShellPattern::from_unquoted("[a-c]").matches("b"));
        assert!(!ShellPattern::from_unquoted("[a-c]").matches("d"));
        assert!(ShellPattern::from_unquoted("[!a-c]").matches("d"));
        assert!(ShellPattern::from_unquoted("[[:digit:]]").matches("7"));
        assert!(!ShellPattern::from_unquoted("[[:digit:]]").matches("x"));
        // `-` last in the bracket is a member, not the start of a range.
        assert!(ShellPattern::from_unquoted("[a-]").matches("-"));
    }

    /// `/` is an ordinary character here: only pathname expansion stops a `*` at one.
    #[test]
    fn a_star_crosses_a_slash() {
        assert!(ShellPattern::from_unquoted("*/*").matches("a/b/c"));
        assert!(ShellPattern::from_unquoted("*").matches("a/b"));
    }

    /// And so is a leading dot: `case .git in *)` matches, however much `echo *` must not.
    #[test]
    fn a_leading_dot_is_not_special() {
        assert!(ShellPattern::from_unquoted("*").matches(".git"));
        assert!(ShellPattern::from_unquoted("?git").matches(".git"));
    }

    #[test]
    fn the_empty_pattern_matches_only_the_empty_string() {
        assert!(ShellPattern::from_unquoted("").matches(""));
        assert!(!ShellPattern::from_unquoted("").matches("x"));
        assert!(ShellPattern::from_unquoted("*").matches(""));
    }

    /// A *quoted* `!` or `^` is a member of the class, not a negation. `[\!a]` is the set
    /// containing `!` and `a`, so `a` matches — oslo used to answer the opposite, which is
    /// "anything but `a`", and got it backwards on every pattern written that way.
    #[test]
    fn a_quoted_negation_marker_is_a_member() {
        assert!(ShellPattern::from_unquoted("[\\!a]").matches("a"));
        assert!(ShellPattern::from_unquoted("[\\!a]").matches("!"));
        assert!(!ShellPattern::from_unquoted("[\\!a]").matches("b"));
        assert!(ShellPattern::from_unquoted("[\\^a]").matches("a"));

        // Unquoted, it still negates.
        assert!(ShellPattern::from_unquoted("[!a]").matches("b"));
        assert!(!ShellPattern::from_unquoted("[!a]").matches("a"));
        assert!(ShellPattern::from_unquoted("[^a]").matches("b"));
    }

    /// bash 5.2: a backslash that reaches the matcher unquoted — out of an unquoted expansion —
    /// escapes the next character, at the top level and inside a bracket.
    #[test]
    fn a_live_backslash_escapes_the_next_character() {
        assert!(ShellPattern::from_unquoted("a\\*").matches("a*"));
        assert!(!ShellPattern::from_unquoted("a\\*").matches("abc"));
        assert!(ShellPattern::from_unquoted("star\\*n*").matches("star*name"));
        assert!(ShellPattern::from_unquoted("[a\\-c]").matches("-"));
        assert!(
            !ShellPattern::from_unquoted("[a\\-c]").matches("b"),
            "not a range"
        );
        // A quoted backslash is only a backslash.
        let quoted = ShellPattern::from_chars(&[('\\', false), ('*', true)]);
        assert!(quoted.matches("\\x"));
    }

    #[test]
    fn collating_symbols_and_equivalence_classes() {
        assert!(ShellPattern::from_unquoted("[[.a.]]*").matches("abc"));
        assert!(!ShellPattern::from_unquoted("[[.a.]]*").matches("bcd"));
        // bash 5.3 in en_US.UTF-8: `[[=e=]]` is `e`, and not `é`.
        let e = ShellPattern::from_unquoted("[[=e=]]mile");
        assert!(e.matches("emile"));
        assert!(!e.matches("émile"));
        assert!(!e.matches("amile"));
    }

    /// **A pattern a user typed must not be able to hang the shell.**
    ///
    /// [`super::matches_items`] resumes from the most recent `*` and no other, which is what keeps
    /// the match linear. A matcher that kept *every* star as a resumption point is the classic
    /// catastrophic case: `a*a*…*b` against a run of `a`s that never reaches the `b` explores one
    /// path per way of dividing the run, and that is exponential in the number of stars.
    ///
    /// Sixty-four characters and ten stars, so a matcher that regressed to full backtracking would
    /// not finish within the life of the test run — while the real one answers in microseconds. The
    /// bound is wall clock and enormously slack on purpose: the distance being tested is between
    /// "instant" and "never", not between two timings.
    ///
    /// This reaches every shell pattern there is — `case`, `[[ == ]]`, `${x#…}` and filename
    /// globbing all compile through here — so the input is one a script can be handed.
    #[test]
    fn a_pathological_pattern_does_not_take_exponential_time() {
        let pattern = ShellPattern::from_unquoted("a*a*a*a*a*a*a*a*a*a*b");
        let subject = "a".repeat(64);

        let started = std::time::Instant::now();
        assert!(!pattern.matches(&subject), "there is no `b` to match");
        let took = started.elapsed();

        assert!(
            took < std::time::Duration::from_secs(2),
            "the match took {took:?}: the star backtracking is no longer bounded to one \
             resumption point"
        );
    }
}
