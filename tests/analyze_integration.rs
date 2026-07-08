use konpu::analyze::analyze_path;
use konpu::domain::konpu::{DiagnosticRule, Severity};

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/analyze/fixtures")
        .join(name)
}

fn count(
    diags: &[konpu::analyze::AnalyzedDiagnostic],
    s: Severity,
    r: DiagnosticRule,
) -> usize {
    diags
        .iter()
        .filter(|d| d.diag.severity == s && d.diag.rule == r)
        .count()
}

#[test]
fn monoid_valid_no_errors() {
    let diags = analyze_path(&fixture("monoid_valid.rs"));
    assert_eq!(count(&diags, Severity::Error, DiagnosticRule::MissingIdentity), 0);
    assert_eq!(count(&diags, Severity::Error, DiagnosticRule::MissingInverse), 0);
    assert_eq!(count(&diags, Severity::Error, DiagnosticRule::ClosureViolation), 0);
}

#[test]
fn monoid_missing_identity() {
    let diags = analyze_path(&fixture("monoid_missing_identity.rs"));
    assert_eq!(count(&diags, Severity::Error, DiagnosticRule::MissingIdentity), 1);
}

#[test]
fn group_missing_inverse() {
    let diags = analyze_path(&fixture("group_missing_inverse.rs"));
    assert_eq!(count(&diags, Severity::Error, DiagnosticRule::MissingInverse), 1);
}

#[test]
fn closure_violation() {
    let diags = analyze_path(&fixture("closure_violation.rs"));
    assert_eq!(count(&diags, Severity::Error, DiagnosticRule::ClosureViolation), 1);
    assert_eq!(count(&diags, Severity::Info, DiagnosticRule::AssociativityConfidence), 0);
}

#[test]
fn confidence_info() {
    let diags = analyze_path(&fixture("confidence_info.rs"));
    assert_eq!(count(&diags, Severity::Info, DiagnosticRule::AssociativityConfidence), 1);
    assert_eq!(count(&diags, Severity::Error, DiagnosticRule::ClosureViolation), 0);
}