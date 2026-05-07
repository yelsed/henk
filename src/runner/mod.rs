//! Shell-execution helper. Thin wrapper around `tokio::process::Command`
//! so detection / install logic has one place to look for `which`-style
//! and `--version`-style probes.
//!
//! Currently a concrete struct rather than a trait. We can extract a
//! `Runner` trait once we need to mock external commands in tests.

mod real;

pub use real::{CommandOutput, SystemRunner};
