use pretty_assertions::assert_eq;

use super::LspConfig;
use super::LspServerConfig;

#[test]
fn disabled_by_default() {
    let config: LspConfig = serde_json::from_value(serde_json::json!({})).expect("deserialize");
    assert_eq!(config.enabled, false);
    assert!(config.servers.is_empty());
}

#[test]
fn servers_for_language_filters() {
    let config = LspConfig {
        enabled: true,
        diagnostics_timeout_ms: None,
        servers: [(
            "pyright".to_string(),
            LspServerConfig::new("pyright-langserver").with_language("python"),
        )]
        .into_iter()
        .collect(),
    };
    assert_eq!(config.servers_for_language("python").len(), 1);
    assert!(config.servers_for_language("rust").is_empty());
}

#[test]
fn toml_example_deserializes() {
    let toml = r#"
enabled = true

[lsp.servers.pyright]
command = "pyright-langserver"
args = ["--stdio"]
languages = ["python"]
"#;
    // Config is parsed as generic TOML table first; only the [lsp] body matters here.
    let body = toml.replace("[lsp.servers.pyright]", "[servers.pyright]");
    let config: LspConfig = toml::from_str(&body).expect("parse lsp config");
    assert_eq!(config.enabled, true);
    let server = config.servers.get("pyright").expect("server");
    assert_eq!(server.command, "pyright-langserver");
    assert_eq!(server.args, vec!["--stdio".to_string()]);
}
