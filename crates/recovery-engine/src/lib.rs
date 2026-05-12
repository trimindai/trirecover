//! `tr-recovery-engine` — scan strategies, async job manager, recovery writer,
//! and session orchestration.
//!
//! This is the brain of TriRecover. It ties together partition detection,
//! filesystem parsing, signature carving, and file recovery into a unified
//! async pipeline with pause/resume/cancel and streaming progress updates.
//!
//! See `docs/architecture.md` §4.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::too_many_lines
)]

pub mod job;
pub mod pipeline;
pub mod recovery;
pub mod strategies;

pub use job::{JobHandle, JobManager};
pub use pipeline::ScanPipeline;
pub use recovery::RecoveryWriter;
