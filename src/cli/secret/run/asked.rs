//! What `oslo secret run` was asked for, before the `--`.
//!
//! ```text
//! VAR=NAME          one secret, as a variable
//! VAR=              the secret named after VAR, lowercased and hyphenated
//! --file VAR=NAME   one secret, as a sealed in-memory file at VAR=/dev/fd/N
//! --env-file FILE   the variables FILE lists
//! --all             every secret of the store
//! --prefix P        before each name --all makes
//! ```

/// One `VAR=NAME` from the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub variable: String,
    pub name: String,
    /// `--file`: handed over as a file rather than a value.
    pub file: bool,
}

/// Everything before the `--`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Asked {
    pub pairs: Vec<Pair>,
    pub env_file: Option<String>,
    pub all: bool,
    pub prefix: Option<String>,
}

impl Asked {
    /// Whether nothing was asked for at all.
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty() && self.env_file.is_none() && !self.all
    }
}

/// Read the words before the `--`.
pub fn parse(words: &[String]) -> Result<Asked, String> {
    let mut asked = Asked::default();
    let mut words = words.iter();
    while let Some(word) = words.next() {
        let (flag, inline) = match word.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (word.as_str(), None),
        };
        let mut value = |what: &str| match inline.clone() {
            Some(value) => Ok(value),
            None => words
                .next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs {what}")),
        };
        match flag {
            "--all" if inline.is_none() => asked.all = true,
            "--prefix" => asked.prefix = Some(value("a prefix")?),
            "--env-file" => {
                if asked.env_file.is_some() {
                    return Err("--env-file is given once".to_string());
                }
                asked.env_file = Some(value("a file")?);
            }
            "--file" => asked.pairs.push(pair(&value("VAR=NAME")?, true)?),
            other if other.starts_with('-') => {
                let shown = oslo::secrets::recipient::shown(word);
                return Err(format!("{shown}: not an option of run"));
            }
            _ => asked.pairs.push(pair(word, false)?),
        }
    }
    if asked.prefix.is_some() && !asked.all {
        return Err("--prefix only means something with --all".to_string());
    }
    Ok(asked)
}

/// `VAR=NAME`, with an empty `NAME` meaning the one `VAR` reads as.
fn pair(word: &str, file: bool) -> Result<Pair, String> {
    let shown = oslo::secrets::recipient::shown(word);
    let Some((variable, name)) = word.split_once('=') else {
        return Err(format!("{shown:?} is not VAR=NAME"));
    };
    if variable.is_empty() {
        return Err(format!("{shown:?} has no variable to set"));
    }
    let name = match name.is_empty() {
        true => variable.to_lowercase().replace('_', "-"),
        false => name.to_string(),
    };
    Ok(Pair {
        variable: variable.to_string(),
        name,
        file,
    })
}

/// The variable `--all` makes of a secret's name: upper-cased, `-` as `_`, after the prefix.
pub fn variable_for(prefix: Option<&str>, name: &str) -> Result<String, String> {
    let variable = format!(
        "{}{}",
        prefix.unwrap_or_default(),
        name.to_uppercase().replace('-', "_")
    );
    match is_identifier(&variable) {
        true => Ok(variable),
        false => Err(format!(
            "{name}: makes no variable name under --all; name it with VAR={name}"
        )),
    }
}

/// `[A-Za-z_][A-Za-z0-9_]*`.
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn every_flag_and_pair_is_read() {
        let asked = parse(&words(
            "GH= --file KUBE=kubeconfig --env-file=.env --all --prefix APP_ T=tok",
        ))
        .expect("parses");
        assert_eq!(asked.env_file.as_deref(), Some(".env"));
        assert!(asked.all);
        assert_eq!(asked.prefix.as_deref(), Some("APP_"));
        let pairs: Vec<(&str, &str, bool)> = asked
            .pairs
            .iter()
            .map(|p| (p.variable.as_str(), p.name.as_str(), p.file))
            .collect();
        assert_eq!(
            pairs,
            [
                ("GH", "gh", false),
                ("KUBE", "kubeconfig", true),
                ("T", "tok", false)
            ]
        );
    }

    #[test]
    fn a_wrong_word_is_refused() {
        for (text, says) in [
            ("--prefix X_", "only means something with --all"),
            ("--env-file", "needs a file"),
            ("--env-file a --env-file b", "given once"),
            ("--file", "needs VAR=NAME"),
            ("--files X=y", "not an option"),
            ("NOEQUALS", "not VAR=NAME"),
            ("=name", "no variable"),
        ] {
            let refused = parse(&words(text)).expect_err(text);
            assert!(refused.contains(says), "{text}: {refused}");
        }
    }

    #[test]
    fn all_makes_upper_case_names() {
        assert_eq!(variable_for(None, "gh-token").as_deref(), Ok("GH_TOKEN"));
        assert_eq!(
            variable_for(Some("APP_"), "db-url").as_deref(),
            Ok("APP_DB_URL")
        );
        assert!(variable_for(None, "has.dot").is_err());
        assert!(variable_for(None, "1st").is_err());
    }
}
