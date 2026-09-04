//! Path-component matching and watch-root selection.

use oslo_base::glob::ShellPattern;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchRoot {
    pub path: PathBuf,
    pub recursive: bool,
}

#[derive(Debug)]
enum Part {
    Recursive,
    Name {
        matcher: ShellPattern,
        matches_hidden: bool,
    },
}

#[derive(Debug)]
pub struct PathPattern {
    parts: Vec<Part>,
    watch: WatchRoot,
}

impl PathPattern {
    pub fn compile(root: &Path, text: &str) -> std::io::Result<Self> {
        let path = if Path::new(text).is_absolute() {
            PathBuf::from(text)
        } else {
            root.join(text)
        };
        let path = lexical(&path);
        let words = components(&path)?;
        let first_pattern = words.iter().position(|part| has_pattern(part));
        let literal = first_pattern.is_none();
        let directory = literal && path.is_dir();
        let mut match_words = words.clone();
        if directory {
            match_words.push("*".to_string());
        }
        let parts = match_words
            .iter()
            .map(|part| {
                if part == "**" {
                    Part::Recursive
                } else {
                    Part::Name {
                        matcher: ShellPattern::from_unquoted(part),
                        matches_hidden: part.starts_with('.'),
                    }
                }
            })
            .collect();
        let prefix = first_pattern.unwrap_or_else(|| {
            if directory {
                words.len()
            } else {
                words.len().saturating_sub(1)
            }
        });
        let mut watch_path = PathBuf::from("/");
        for part in words.iter().take(prefix) {
            watch_path.push(part);
        }
        Ok(Self {
            parts,
            watch: WatchRoot {
                path: watch_path,
                recursive: match_words.iter().any(|part| part == "**"),
            },
        })
    }

    pub fn matches(&self, path: &Path) -> bool {
        let Ok(names) = components(&lexical(path)) else {
            return false;
        };
        matches_parts(&self.parts, &names, 0, 0)
    }

    pub fn watch_root(&self) -> &WatchRoot {
        &self.watch
    }
}

#[derive(Debug)]
pub struct PatternSet {
    patterns: Vec<PathPattern>,
}

impl PatternSet {
    pub fn compile(root: &Path, patterns: &[String]) -> std::io::Result<Self> {
        patterns
            .iter()
            .map(|pattern| PathPattern::compile(root, pattern))
            .collect::<std::io::Result<Vec<_>>>()
            .map(|patterns| Self { patterns })
    }

    pub fn matches(&self, path: &Path) -> bool {
        self.patterns.iter().any(|pattern| pattern.matches(path))
    }

    pub fn roots(&self) -> impl Iterator<Item = &WatchRoot> {
        self.patterns.iter().map(PathPattern::watch_root)
    }
}

fn matches_parts(parts: &[Part], names: &[String], pattern: usize, name: usize) -> bool {
    match parts.get(pattern) {
        None => name == names.len(),
        Some(Part::Recursive) => {
            matches_parts(parts, names, pattern + 1, name)
                || names.get(name).is_some_and(|part| {
                    !part.starts_with('.') && matches_parts(parts, names, pattern, name + 1)
                })
        }
        Some(Part::Name {
            matcher,
            matches_hidden,
        }) => names.get(name).is_some_and(|part| {
            (!part.starts_with('.') || *matches_hidden)
                && matcher.matches(part)
                && matches_parts(parts, names, pattern + 1, name + 1)
        }),
    }
}

fn has_pattern(part: &str) -> bool {
    part.contains(['*', '?', '['])
}

fn components(path: &Path) -> std::io::Result<Vec<String>> {
    path.components()
        .filter_map(|component| match component {
            Component::RootDir | Component::CurDir => None,
            Component::Normal(name) => Some(
                name.to_str()
                    .map(str::to_string)
                    .ok_or_else(|| std::io::Error::other("watch path is not valid UTF-8")),
            ),
            Component::ParentDir | Component::Prefix(_) => Some(Err(std::io::Error::other(
                "watch path could not be normalized",
            ))),
        })
        .collect()
}

fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(text: &str) -> PathPattern {
        PathPattern::compile(Path::new("/work"), text).expect("pattern")
    }

    #[test]
    fn component_patterns_do_not_cross_separators() {
        let p = pattern("src/*.rs");
        assert!(p.matches(Path::new("/work/src/lib.rs")));
        assert!(!p.matches(Path::new("/work/src/deep/lib.rs")));
    }

    #[test]
    fn recursive_matches_zero_or_many_directories() {
        let p = pattern("src/**/*.rs");
        assert!(p.matches(Path::new("/work/src/lib.rs")));
        assert!(p.matches(Path::new("/work/src/a/b/lib.rs")));
    }

    #[test]
    fn wildcard_does_not_match_hidden_components() {
        let p = pattern("src/**/.*.rs");
        assert!(p.matches(Path::new("/work/src/.root.rs")));
        assert!(p.matches(Path::new("/work/src/a/.nested.rs")));
        assert!(!p.matches(Path::new("/work/src/.hidden/a.rs")));
    }

    #[test]
    fn relative_parent_is_normalized_once() {
        let p = pattern("../shared/?.toml");
        assert!(p.matches(Path::new("/shared/a.toml")));
    }
}
