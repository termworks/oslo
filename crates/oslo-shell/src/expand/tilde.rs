//! Tilde expansion.

use crate::env::Environment;

/// Expand a `~`, `~user`, `~+` or `~-` prefix.
///
/// Every failure path returns the text unchanged, which is what POSIX asks for and also the only
/// safe answer: `~nosuchuser` as a literal is a path that will fail loudly, while a guessed home
/// directory is a path that will succeed on the wrong files.
/// The rule itself is [`oslo_base::tilde`], because the prompt needs the same answer and cannot see
/// this crate — completion, the ghost and the highlighter each knew only `~` and `~/…`, so a form
/// this function expands perfectly read at the prompt as a mistake.
pub fn expand_tilde(env: &Environment, user: &str) -> String {
    if let Some(entry) = stack_entry(env, user) {
        return entry;
    }
    oslo_base::tilde::expand(user, &|name: &str| env.get_var(name).map(str::to_string))
}

/// `~N`, `~+N` and `~-N`: an entry of the directory stack, counted as `dirs +N` and `dirs -N`.
///
/// `None` for any other prefix, and for an index past the end, which stays literal.
fn stack_entry(env: &Environment, user: &str) -> Option<String> {
    let (from_bottom, digits) = match user.as_bytes().first()? {
        b'+' => (false, &user[1..]),
        b'-' => (true, &user[1..]),
        _ => (false, user),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: usize = digits.parse().ok()?;
    let entries = crate::env::builtins::directory_stack(env);
    let index = if from_bottom {
        entries.len().checked_sub(n + 1)?
    } else {
        n
    };
    entries.get(index).cloned()
}

#[cfg(test)]
mod tests {
    use super::expand_tilde;
    use crate::env::Environment;

    #[test]
    fn bare_tilde_is_home() {
        let mut env = Environment::new();
        env.set_var("HOME", "/home/tester", false);
        assert_eq!(expand_tilde(&env, ""), "/home/tester");
    }

    #[test]
    fn unknown_user_is_left_alone() {
        let env = Environment::new();
        assert_eq!(
            expand_tilde(&env, "oslo-no-such-user"),
            "~oslo-no-such-user"
        );
    }

    /// `~root` is the one entry every Unix password database is guaranteed to have.
    #[test]
    fn known_user_resolves_through_passwd() {
        let env = Environment::new();
        let expected = nix::unistd::User::from_name("root")
            .unwrap()
            .expect("every system has root")
            .dir
            .to_string_lossy()
            .into_owned();
        assert_eq!(expand_tilde(&env, "root"), expected);
    }

    #[test]
    fn plus_is_pwd_and_minus_is_oldpwd() {
        let mut env = Environment::new();
        env.set_var("PWD", "/here", false);
        env.set_var("OLDPWD", "/there", false);
        assert_eq!(expand_tilde(&env, "+"), "/here");
        assert_eq!(expand_tilde(&env, "-"), "/there");
    }

    /// Without `OLDPWD` there is no previous directory to name, so the text stands.
    #[test]
    fn minus_without_oldpwd_stays_literal() {
        let mut env = Environment::new();
        env.unset_var("OLDPWD");
        assert_eq!(expand_tilde(&env, "-"), "~-");
    }
}
