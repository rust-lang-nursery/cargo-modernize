//! `cargo-modernize` library.
//!
//! Exposes the scanning API and types used by the LSP and other consumers.

pub mod config;
pub mod ecosystem;
pub mod legacy;
pub mod project;
pub mod rewrite;
pub mod rules;
pub mod scan;

pub use rules::Level;
