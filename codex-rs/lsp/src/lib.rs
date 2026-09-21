//! Minimal opt-in LSP integration for Codex (issue #8745).
//!
//! Stage 1: a persistent diagnostics-only stdio client + config types.
//! No auto-install, no symbols/rename, and no agent-loop wiring yet, so this
//! crate stays independently testable and reviewable.

mod client;
mod config;
mod diagnostic;
mod protocol;

pub use client::LspClient;
pub use config::LspConfig;
pub use config::LspServerConfig;
pub use diagnostic::Diagnostic;
pub use diagnostic::DiagnosticSeverity;
