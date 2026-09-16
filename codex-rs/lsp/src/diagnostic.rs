/// One LSP diagnostic, trimmed to what an agent needs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Diagnostic {
    /// File URI or path the diagnostic belongs to.
    pub uri: String,
    /// 0-based start line.
    pub line: u32,
    /// 0-based start character.
    pub character: u32,
    pub severity: DiagnosticSeverity,
    pub message: String,
    /// Optional diagnostic code (e.g. `"PyF0001"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Optional source (e.g. `"pyright"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Diagnostic {
    /// Bounded rendering for model context (each diagnostic is one line).
    pub fn to_context_line(&self) -> String {
        let code = self.code.as_deref().unwrap_or("-");
        format!(
            "{}:{}:{} [{}] {} ({code})",
            self.uri,
            self.line + 1,
            self.character + 1,
            self.severity.as_str(),
            truncate(&self.message, 300),
        )
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}

/// LSP `DiagnosticSeverity` (1=Error .. 4=Hint).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl DiagnosticSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Information => "info",
            Self::Hint => "hint",
        }
    }

    pub fn from_lsp(value: u32) -> Self {
        match value {
            1 => Self::Error,
            2 => Self::Warning,
            3 => Self::Information,
            _ => Self::Hint,
        }
    }
}

#[cfg(test)]
#[path = "diagnostic_tests.rs"]
mod tests;
