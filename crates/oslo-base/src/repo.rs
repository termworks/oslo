//! Whether a directory is a git repository.

use std::path::Path;

/// Whether `dot_git` is a repository rather than a path that merely has the name.
///
/// A real one is a directory with `HEAD` in it, or a *file* naming where the directory is, which is
/// what a linked worktree and a submodule have. An empty `~/.git` left behind by something is
/// neither — `git -C ~ rev-parse` answers "not a git repository" — and a bare `exists()` made all
/// of `$HOME` one workspace.
pub fn is_repository(dot_git: &Path) -> bool {
    dot_git.is_file() || dot_git.join("HEAD").exists()
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_empty_dot_git_is_not_a_repository() {
        let dir = std::env::temp_dir().join(format!("oslo-repo-{}", std::process::id()));
        let dot = dir.join(".git");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dot).unwrap();
        assert!(!super::is_repository(&dot));
        std::fs::write(dot.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert!(super::is_repository(&dot));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
