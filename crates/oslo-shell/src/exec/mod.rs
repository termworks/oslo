//! Command execution.
//!
//! Split by the shape of what is being run: [`pipeline`] drives lists and pipelines,
//! [`simple`] runs one command, [`compound`] handles control flow, [`substitution`] captures
//! `$(...)`, [`redirect`] applies redirections, and [`job`] holds job-control state.

pub mod argv;
pub mod compound;
mod coproc;
pub mod funsub;
pub mod job;
pub mod pipeline;
pub mod procsub;
pub mod redirect;
mod select;
pub mod simple;
pub mod stored;
/// The stack of streams a coordinate reads from. See [`streams`].
pub mod streams;
pub mod substitution;

pub use job::JobManager;
pub use pipeline::{eval_and_or_list, eval_command, eval_command_list, eval_pipeline};
pub use substitution::eval_command_substitution;
