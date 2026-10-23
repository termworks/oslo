//! Protocol-neutral terminal command lifecycle.

use super::capability::{Capabilities, SemanticProtocol};
use super::{osc133, vscode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Primary,
    Continuation,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandVisibility {
    Publish,
    Omit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticEvent<'a> {
    InteractionStart,
    PromptStart(PromptKind),
    InputStart,
    CommandStart {
        command_line: &'a str,
        visibility: CommandVisibility,
    },
    CommandEnd {
        status: i32,
    },
    InteractionAbort,
    CwdChanged(&'a str),
    TitleChanged(&'a str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionError {
    InvalidTransition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Interaction,
    Prompt(PromptKind),
    Input,
    Output,
}

/// The marks a block on screen is wearing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockMarks {
    /// Belongs at the block's first row.
    pub prompt: String,
    /// Belongs where the prompt ends and the input begins.
    pub input: String,
}

pub struct SemanticSession {
    aid: String,
    capabilities: Capabilities,
    phase: Phase,
    /// Which prompt the block being edited is, kept past the move to `Input`.
    block: PromptKind,
}

impl SemanticSession {
    pub fn new(aid: impl Into<String>, capabilities: Capabilities) -> Self {
        Self {
            aid: aid.into(),
            capabilities,
            phase: Phase::Idle,
            block: PromptKind::Primary,
        }
    }

    pub fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    /// The marks for the block being edited, without advancing the lifecycle.
    ///
    /// Empty once the command is running: the rows drawn then are output's, not a block's.
    pub fn block_marks(&self) -> BlockMarks {
        match self.phase {
            Phase::Prompt(_) | Phase::Input => BlockMarks {
                prompt: self.prompt_start(self.block),
                input: self.input_start(self.block),
            },
            _ => BlockMarks::default(),
        }
    }

    pub fn emit(&mut self, event: SemanticEvent<'_>) -> String {
        self.try_emit(event).unwrap_or_default()
    }

    pub fn try_emit(&mut self, event: SemanticEvent<'_>) -> Result<String, TransitionError> {
        let output = match event {
            SemanticEvent::InteractionStart if self.phase == Phase::Idle => {
                self.phase = Phase::Interaction;
                String::new()
            }
            SemanticEvent::PromptStart(PromptKind::Primary) if self.phase == Phase::Interaction => {
                self.phase = Phase::Prompt(PromptKind::Primary);
                self.block = PromptKind::Primary;
                self.prompt_start(PromptKind::Primary)
            }
            SemanticEvent::PromptStart(PromptKind::Continuation) if self.phase == Phase::Input => {
                self.phase = Phase::Prompt(PromptKind::Continuation);
                self.block = PromptKind::Continuation;
                self.prompt_start(PromptKind::Continuation)
            }
            SemanticEvent::PromptStart(PromptKind::Right) => String::new(),
            SemanticEvent::InputStart if matches!(self.phase, Phase::Prompt(_)) => {
                let prompt = match self.phase {
                    Phase::Prompt(kind) => kind,
                    _ => unreachable!(),
                };
                self.phase = Phase::Input;
                self.input_start(prompt)
            }
            SemanticEvent::CommandStart {
                command_line,
                visibility,
            } if self.phase == Phase::Input => {
                self.phase = Phase::Output;
                self.command_start(command_line, visibility)
            }
            SemanticEvent::CommandEnd { status } if self.phase == Phase::Output => {
                self.phase = Phase::Idle;
                self.command_end(Some(status))
            }
            SemanticEvent::InteractionAbort
                if matches!(
                    self.phase,
                    Phase::Interaction | Phase::Prompt(_) | Phase::Input
                ) =>
            {
                self.phase = Phase::Idle;
                self.command_end(None)
            }
            SemanticEvent::CwdChanged(path) => self.cwd_changed(path),
            SemanticEvent::TitleChanged(_) => String::new(),
            _ => return Err(TransitionError::InvalidTransition),
        };
        Ok(output)
    }

    fn prompt_start(&self, kind: PromptKind) -> String {
        match self.capabilities.semantic {
            SemanticProtocol::None => String::new(),
            SemanticProtocol::Osc133
                if kind == PromptKind::Continuation && !self.capabilities.semantic_secondary =>
            {
                String::new()
            }
            SemanticProtocol::Osc133 => {
                osc133::prompt_start(&self.aid, kind, self.capabilities.semantic_clicks)
            }
            SemanticProtocol::Vscode633 if kind == PromptKind::Primary => vscode::prompt_start(),
            SemanticProtocol::Vscode633 => String::new(),
        }
    }

    fn input_start(&self, prompt: PromptKind) -> String {
        match self.capabilities.semantic {
            SemanticProtocol::None => String::new(),
            SemanticProtocol::Osc133
                if prompt == PromptKind::Continuation && !self.capabilities.semantic_secondary =>
            {
                String::new()
            }
            SemanticProtocol::Osc133 => osc133::input_start(&self.aid),
            SemanticProtocol::Vscode633 if prompt == PromptKind::Primary => vscode::prompt_end(),
            SemanticProtocol::Vscode633 => String::new(),
        }
    }

    fn command_start(&self, command: &str, visibility: CommandVisibility) -> String {
        let published = (visibility == CommandVisibility::Publish).then_some(command);
        match self.capabilities.semantic {
            SemanticProtocol::None => String::new(),
            SemanticProtocol::Osc133 => osc133::command_start(
                &self.aid,
                published.filter(|_| self.capabilities.osc133_cmdline_url),
            ),
            SemanticProtocol::Vscode633 => {
                let metadata = published
                    .map(|line| vscode::command_line(line, None))
                    .unwrap_or_default();
                format!("{metadata}{}", vscode::command_start())
            }
        }
    }

    fn command_end(&self, status: Option<i32>) -> String {
        let status = status.map(normalize_status);
        match self.capabilities.semantic {
            SemanticProtocol::None => String::new(),
            SemanticProtocol::Osc133 => osc133::command_end(&self.aid, status),
            SemanticProtocol::Vscode633 => vscode::command_end(status),
        }
    }

    fn cwd_changed(&self, path: &str) -> String {
        match self.capabilities.semantic {
            SemanticProtocol::None => String::new(),
            SemanticProtocol::Vscode633 => vscode::property("Cwd", path).unwrap_or_default(),
            SemanticProtocol::Osc133 => String::new(),
        }
    }
}

fn normalize_status(status: i32) -> i32 {
    status.rem_euclid(256)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generic(metadata: bool) -> SemanticSession {
        let mut capabilities = Capabilities::portable();
        capabilities.osc133_cmdline_url = metadata;
        SemanticSession::new("42-start", capabilities)
    }

    #[test]
    fn generic_interactions_are_balanced() {
        let mut session = generic(true);
        assert_eq!(session.emit(SemanticEvent::InteractionStart), "");
        assert!(
            session
                .emit(SemanticEvent::PromptStart(PromptKind::Primary))
                .contains(";A;")
        );
        assert!(session.emit(SemanticEvent::InputStart).contains(";B;"));
        assert!(
            session
                .emit(SemanticEvent::CommandStart {
                    command_line: "echo hi",
                    visibility: CommandVisibility::Publish,
                })
                .contains(";C;cmdline_url=echo%20hi;")
        );
        assert!(
            session
                .emit(SemanticEvent::CommandEnd { status: 3 })
                .contains(";D;3;")
        );
    }

    #[test]
    fn command_statuses_are_shell_status_bytes() {
        for (status, expected) in [(-1, 255), (256, 0), (513, 1)] {
            let mut session = generic(false);
            session.emit(SemanticEvent::InteractionStart);
            session.emit(SemanticEvent::PromptStart(PromptKind::Primary));
            session.emit(SemanticEvent::InputStart);
            session.emit(SemanticEvent::CommandStart {
                command_line: "true",
                visibility: CommandVisibility::Publish,
            });
            let end = session.emit(SemanticEvent::CommandEnd { status });
            assert!(end.contains(&format!(";D;{expected};")), "{end:?}");
        }
    }

    #[test]
    fn duplicates_and_out_of_order_events_are_silent() {
        let mut session = generic(false);
        assert_eq!(
            session.try_emit(SemanticEvent::InputStart),
            Err(TransitionError::InvalidTransition)
        );
        assert_eq!(session.emit(SemanticEvent::InputStart), "");
        session.emit(SemanticEvent::InteractionStart);
        let first = session.emit(SemanticEvent::PromptStart(PromptKind::Primary));
        assert!(!first.is_empty());
        assert_eq!(
            session.try_emit(SemanticEvent::PromptStart(PromptKind::Primary)),
            Err(TransitionError::InvalidTransition)
        );
        assert_eq!(
            session.emit(SemanticEvent::PromptStart(PromptKind::Primary)),
            ""
        );
        session.emit(SemanticEvent::InputStart);
        assert_eq!(session.emit(SemanticEvent::InputStart), "");
    }

    #[test]
    fn continuations_stay_in_one_interaction() {
        let capabilities =
            Capabilities::portable().with_verified(super::super::capability::Verified {
                semantic_secondary: true,
                ..super::super::capability::Verified::default()
            });
        let mut session = SemanticSession::new("42-start", capabilities);
        session.emit(SemanticEvent::InteractionStart);
        session.emit(SemanticEvent::PromptStart(PromptKind::Primary));
        session.emit(SemanticEvent::InputStart);
        let continuation = session.emit(SemanticEvent::PromptStart(PromptKind::Continuation));
        assert!(continuation.contains(";A;k=s;"));
        assert!(session.emit(SemanticEvent::InputStart).contains(";B;"));
    }

    #[test]
    fn private_commands_and_aborts_publish_no_command_text() {
        let mut session = generic(true);
        session.emit(SemanticEvent::InteractionStart);
        session.emit(SemanticEvent::PromptStart(PromptKind::Primary));
        session.emit(SemanticEvent::InputStart);
        let start = session.emit(SemanticEvent::CommandStart {
            command_line: "secret",
            visibility: CommandVisibility::Omit,
        });
        assert!(!start.contains("cmdline_url"), "{start:?}");
        session.emit(SemanticEvent::CommandEnd { status: 0 });

        session.emit(SemanticEvent::InteractionStart);
        session.emit(SemanticEvent::PromptStart(PromptKind::Primary));
        session.emit(SemanticEvent::InputStart);
        let abort = session.emit(SemanticEvent::InteractionAbort);
        assert!(abort.contains(";D;aid="), "{abort:?}");
        assert!(!abort.contains(";C;"), "{abort:?}");
    }

    #[test]
    fn vscode_uses_one_rich_lifecycle() {
        let capabilities = Capabilities::from_environment(Some("vscode"));
        let mut session = SemanticSession::new("unused", capabilities);
        session.emit(SemanticEvent::InteractionStart);
        assert_eq!(
            session.emit(SemanticEvent::PromptStart(PromptKind::Primary)),
            vscode::prompt_start()
        );
        assert_eq!(
            session.emit(SemanticEvent::InputStart),
            vscode::prompt_end()
        );
        assert_eq!(
            session.emit(SemanticEvent::CommandStart {
                command_line: "echo hi",
                visibility: CommandVisibility::Publish,
            }),
            format!(
                "{}{}",
                vscode::command_line("echo hi", None),
                vscode::command_start()
            )
        );
        assert_eq!(
            session.emit(SemanticEvent::CommandEnd { status: 0 }),
            vscode::command_end(Some(0))
        );
    }

    #[test]
    fn null_backend_tracks_transitions_without_output() {
        let mut session = SemanticSession::new("unused", Capabilities::disabled());
        let output = [
            session.emit(SemanticEvent::InteractionStart),
            session.emit(SemanticEvent::PromptStart(PromptKind::Primary)),
            session.emit(SemanticEvent::InputStart),
            session.emit(SemanticEvent::CommandStart {
                command_line: "echo hidden",
                visibility: CommandVisibility::Publish,
            }),
            session.emit(SemanticEvent::CommandEnd { status: 0 }),
        ];
        assert!(output.iter().all(String::is_empty));
        assert_eq!(session.emit(SemanticEvent::InputStart), "");
        assert_eq!(session.emit(SemanticEvent::InteractionStart), "");
    }
}
