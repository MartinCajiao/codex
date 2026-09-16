use std::collections::HashMap;
use std::time::Duration;

/// Opt-in LSP configuration (diagnostics only for stage 1).
///
/// Example `config.toml`:
/// ```toml
/// [lsp]
/// enabled = true
///
/// [lsp.servers.pyright]
/// command = "pyright-langserver"
/// args = ["--stdio"]
/// languages = ["python"]
/// ```
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LspConfig {
    /// Master switch. Default off: zero behavior change unless opted in.
    #[serde(default)]
    pub enabled: bool,
    /// Named language servers, keyed by an arbitrary label.
    #[serde(default)]
    pub servers: HashMap<String, LspServerConfig>,
    /// How long to wait for `publishDiagnostics` after `didOpen`.
    #[serde(default, with = "option_duration_ms")]
    pub diagnostics_timeout_ms: Option<Duration>,
}

impl LspConfig {
    /// Servers applicable to `language_id` (e.g. `"python"`, `"rust"`).
    pub fn servers_for_language(&self, language_id: &str) -> Vec<(&str, &LspServerConfig)> {
        self.servers
            .iter()
            .map(|(name, server)| (name.as_str(), server))
            .filter(|(_, server)| server.languages.iter().any(|l| l == language_id))
            .collect()
    }
}

/// One stdio language server.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LspServerConfig {
    /// Binary to spawn (must speak LSP over stdio).
    pub command: String,
    /// Extra argv. Defaults to empty.
    #[serde(default)]
    pub args: Vec<String>,
    /// Language ids this server handles (e.g. `["python"]`).
    #[serde(default)]
    pub languages: Vec<String>,
    /// Per-server startup timeout override.
    #[serde(default, with = "option_duration_ms")]
    pub startup_timeout_ms: Option<Duration>,
}

impl LspServerConfig {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            languages: Vec::new(),
            startup_timeout_ms: None,
        }
    }

    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.languages.push(language.into());
        self
    }
}

mod option_duration_ms {
    use std::time::Duration;

    use serde::Deserialize;
    use serde::Deserializer;
    use serde::Serializer;

    pub fn serialize<S: Serializer>(
        value: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(duration) => {
                let millis = duration.as_millis().min(u128::from(u64::MAX)) as u64;
                serializer.serialize_some(&millis)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        let millis = Option::<u64>::deserialize(deserializer)?;
        Ok(millis.map(Duration::from_millis))
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
