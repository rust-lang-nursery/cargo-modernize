//! `cargo-modernize-lsp`
//!
//! Tower-based Language Server that runs `cargo-modernize` over Rust source
//! and reports old-school vs new-school idioms as LSP diagnostics.

pub mod diagnostics;
pub mod server;

pub use server::ModernizeServer;
