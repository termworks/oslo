//! Filesystem-triggered command services.

pub mod event;
mod pattern;
mod process;
mod runner;
mod set;

pub use pattern::{PathPattern, PatternSet, WatchRoot};
pub use runner::run;

use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Coalesce,
    Restart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchSpec {
    pub name: String,
    pub root: PathBuf,
    pub patterns: Vec<String>,
    pub argv: Vec<String>,
    pub initial: bool,
    pub debounce: Duration,
    pub policy: Policy,
    pub grace: Duration,
}

impl WatchSpec {
    pub fn validate(&self) -> std::io::Result<()> {
        if self.patterns.is_empty() {
            return Err(std::io::Error::other("watch needs at least one path"));
        }
        if self.argv.is_empty() {
            return Err(std::io::Error::other("watch needs a command"));
        }
        if !self.root.is_absolute() {
            return Err(std::io::Error::other("watch root must be absolute"));
        }
        Ok(())
    }
}
