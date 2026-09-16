use pretty_assertions::assert_eq;

use super::Diagnostic;
use super::DiagnosticSeverity;

fn sample() -> Diagnostic {
    Diagnostic {
        uri: "file:///repo/main.py".to_string(),
        line: 0,
        character: 4,
        severity: DiagnosticSeverity::Error,
        message: "undefined name".to_string(),
        code: Some("F821".to_string()),
        source: Some("pyright".to_string()),
    }
}

#[test]
fn context_line_is_one_based_and_bounded() {
    assert_eq!(
        sample().to_context_line(),
        "file:///repo/main.py:1:5 [error] undefined name (F821)"
    );
}

#[test]
fn long_messages_are_truncated() {
    let mut diagnostic = sample();
    diagnostic.message = "x".repeat(500);
    let line = diagnostic.to_context_line();
    assert!(line.chars().count() < 500);
    assert!(line.ends_with('…') || line.contains('…'));
}

#[test]
fn severity_mapping_matches_lsp() {
    assert_eq!(DiagnosticSeverity::from_lsp(1), DiagnosticSeverity::Error);
    assert_eq!(DiagnosticSeverity::from_lsp(2), DiagnosticSeverity::Warning);
    assert_eq!(
        DiagnosticSeverity::from_lsp(3),
        DiagnosticSeverity::Information
    );
    assert_eq!(DiagnosticSeverity::from_lsp(99), DiagnosticSeverity::Hint);
}
