use konpu::analyze::{analyze_path, analyze_with_config, template};
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

#[test]
fn monoid_missing_identity_suppresses_info() {
    let diags = analyze_path(&fixture("monoid_missing_identity.rs"));
    assert_eq!(count(&diags, Severity::Error, DiagnosticRule::MissingIdentity), 1);
    assert_eq!(count(&diags, Severity::Info, DiagnosticRule::AssociativityConfidence), 0);
}

#[test]
fn functor_sig_violation() {
    let diags = analyze_path(&fixture("functor_sig_violation.rs"));
    assert_eq!(count(&diags, Severity::Error, DiagnosticRule::MapSignatureViolation), 1);
}

#[test]
fn law_test_present_no_missing_warning() {
    let diags = analyze_path(&fixture("law_test_present.rs"));
    assert_eq!(count(&diags, Severity::Warning, DiagnosticRule::MissingLawTest), 0);
}

#[test]
fn law_test_missing_warns() {
    let diags = analyze_path(&fixture("law_test_missing.rs"));
    assert_eq!(count(&diags, Severity::Warning, DiagnosticRule::MissingLawTest), 1);
}

#[test]
fn monoid_partial_law_tests_warns() {
    let diags = analyze_path(&fixture("monoid_partial_law_tests.rs"));
    // Monoid rank 2 includes Semigroup{Associativity} + Monoid{LeftIdentity, RightIdentity} = 3 laws.
    // Only left_identity is present → 2 missing.
    assert_eq!(count(&diags, Severity::Warning, DiagnosticRule::MissingLawTest), 2);
}

#[test]
fn propagation_exceeded_when_unbounded_under_threshold() {
    let config = template::parse(
        "[defaults]\nmax_propagation = 4\n",
    );
    let diags = analyze_with_config(&fixture("propagation_exceeded.rs"), &config);
    assert_eq!(count(&diags, Severity::Warning, DiagnosticRule::PropagationExceeded), 1);
}

#[test]
fn propagation_ok_when_no_threshold() {
    let config = template::ResolvedConfig::empty();
    let diags = analyze_with_config(&fixture("propagation_exceeded.rs"), &config);
    assert_eq!(count(&diags, Severity::Warning, DiagnosticRule::PropagationExceeded), 0);
}

#[test]
fn propagation_unlimited_threshold_allows_unbounded() {
    let config = template::parse(
        "[defaults]\nmax_propagation = -1\n",
    );
    let diags = analyze_with_config(&fixture("propagation_exceeded.rs"), &config);
    assert_eq!(count(&diags, Severity::Warning, DiagnosticRule::PropagationExceeded), 0);
}