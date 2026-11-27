//! `oslo <Tab>` offers the tools this build actually has.
//!
//! ```console
//! $ oslo <Tab>
//!   macros   the aliases, abbreviations, functions and scripts you keep
//!   config   inspect and edit the Lua configuration
//!   secret   values kept encrypted, handed out when something asks
//! ```
//!
//! # From the one list, so it cannot drift
//!
//! [`super::tools::TOOLS`] is already the single source the dispatcher and `--help` both read — a
//! tool cannot be reachable and undocumented, or listed and unreachable. Completing from the same
//! list makes that three readers of one fact instead of two readers and a copy. A tool added in a
//! commit is completable in that commit, with the description already written for its help line.
//!
//! **And it is right per build without saying so.** The entries are `#[cfg]`-gated one by one, so a
//! binary without `secrets` has no `secret` in the list and therefore offers none — where a spec
//! file would have to be generated per feature combination, or be wrong for most of them.

use oslo_ui::spec::{CommandSpec, OptionSpec, SubcommandSpec};

/// Register the source. Called once, as the interactive shell starts.
pub(crate) fn register() {
    oslo_ui::spec::custom::add_source(
        "tools",
        std::rc::Rc::new(|command: &str| match command {
            "oslo" => Some(spec()),
            _ => None,
        }),
    );
}

fn spec() -> CommandSpec {
    CommandSpec {
        name: "oslo".to_string(),
        description: "a POSIX shell that also speaks Lua".to_string(),
        subcommands: super::tools::TOOLS
            .iter()
            .map(|tool| SubcommandSpec {
                name: tool.name.to_string(),
                description: tool.about.to_string(),
                // Every tool answers it, and it is the one flag somebody types when they do not
                // yet know what the others are.
                options: vec![OptionSpec::new(
                    vec!["-h".to_string(), "--help".to_string()],
                    "what this tool does, and its options".to_string(),
                )],
                ..SubcommandSpec::default()
            })
            .collect(),
        options: vec![
            OptionSpec::new(
                vec!["-c".to_string()],
                "run one command and exit".to_string(),
            ),
            OptionSpec::new(
                vec!["-l".to_string(), "--login".to_string()],
                "act as a login shell".to_string(),
            ),
            OptionSpec::new(
                vec!["-i".to_string()],
                "interactive, even with no terminal".to_string(),
            ),
            OptionSpec::new(
                vec!["--no-rc".to_string()],
                "skip the configuration".to_string(),
            ),
            OptionSpec::new(
                vec!["--func".to_string()],
                "run a Lua file pure".to_string(),
            ),
            OptionSpec::new(
                vec!["--jit".to_string()],
                "compile hot Lua to machine code".to_string(),
            ),
            OptionSpec::new(
                vec!["-h".to_string(), "--help".to_string()],
                "the tour, and every tool".to_string(),
            ),
            OptionSpec::new(
                vec!["-V".to_string(), "--version".to_string()],
                "what this build calls itself".to_string(),
            ),
        ],
        ..CommandSpec::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every tool is offered, and only the ones this build has.** The `#[cfg]`s are on the list
    /// itself, so this is the same assertion in both directions.
    #[test]
    fn the_tools_are_the_subcommands() {
        let spec = spec();
        assert_eq!(spec.subcommands.len(), super::super::tools::TOOLS.len());
        for tool in super::super::tools::TOOLS {
            let found = spec
                .subcommands
                .iter()
                .find(|sub| sub.name == tool.name)
                .unwrap_or_else(|| panic!("{} is not offered", tool.name));
            assert_eq!(found.description, tool.about);
        }
    }

    /// A tool that is gated out of this build is gated out of the menu, without a second list
    /// saying so.
    #[test]
    fn a_tool_this_build_lacks_is_not_offered() {
        let spec = spec();
        let offered = |name: &str| spec.subcommands.iter().any(|s| s.name == name);
        assert_eq!(offered("secret"), cfg!(feature = "secrets"));
        assert_eq!(offered("plugin"), cfg!(feature = "plugin"));
        assert_eq!(offered("make"), cfg!(feature = "make"));
        // …and the ungated ones are always there.
        assert!(offered("macros") && offered("config") && offered("history"));
    }
}
