//! Per-project linking: detection, override-file generation, file-provider
//! entries, .henk.toml read/write.

pub mod compose;
pub mod detect;
pub mod env_file;
pub mod file_provider;
pub mod link;
pub mod manifest;
pub mod override_file;
pub mod preflight;
pub mod unlink;
