//! Minimal opt-in LSP integration for Codex (issue #8745).
//!
//! Stage 1: diagnostics-only stdio client + config types.
//! No auto-install, no symbols/rename yet. The agent loop wiring
//! lands separately so this crate stays small and reviewable.

mod client;
mod config;
mod diagnostic;
mod protocol;

pub use client::LspClient;
pub use config::LspConfig;
pub use config::LspServerConfig;
pub use diagnostic::Diagnostic;
pub use diagnostic::DiagnosticSeverity;
