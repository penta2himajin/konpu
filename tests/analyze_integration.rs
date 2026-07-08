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

#[test]
fn scaffold_generates_required_law_tests() {
    use konpu::analyze::scaffold;
    let config = template::ResolvedConfig::empty();
    let files = scaffold::scaffold_path(&fixture("monoid_valid.rs"), &config);
    assert_eq!(files.len(), 1);
    let f = &files[0];
    // Monoid accumulates Semigroup{Associativity} + Monoid{LeftIdentity,RightIdentity} = 3 tests
    assert_eq!(f.decl_count, 1);
    assert_eq!(f.test_count, 3);
    let body = &f.contents;
    assert!(body.contains("test_ValidMonoid_associativity"));
    assert!(body.contains("test_ValidMonoid_left_identity"));
    assert!(body.contains("test_ValidMonoid_right_identity"));
    assert!(f.path.ends_with("monoid_valid_law_tests.rs"));
}

#[test]
fn scaffold_skips_files_with_no_annotations() {
    use konpu::analyze::scaffold;
    let config = template::ResolvedConfig::empty();
    // Empty fixture (no annotations) — we use the empty crate src for an
    // annotate-free path: the `opencode.json` is non-Rust so collect_rust_files
    // returns nothing; use the project's own root source dir which has no
    // `#[konpu::*]` annotations in `src/main.rs` etc.
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("main.rs");
    let files = scaffold::scaffold_path(&p, &config);
    assert!(files.is_empty());
}

#[test]
fn baseline_roundtrip_filters_known_violations() {
    use konpu::analyze::baseline;
    use konpu::analyze::analyze_with_config;
    use konpu::analyze::template;
    let cfg = template::ResolvedConfig::empty();
    let diags = analyze_with_config(&fixture("monoid_missing_identity.rs"), &cfg);
    assert!(!diags.is_empty());
    let entries = baseline::entries_from(&diags);
    let tmp = std::env::temp_dir().join("konpu_baseline_test.json");
    baseline::save(&tmp, &entries).unwrap();
    let bl = baseline::load(&tmp);
    let filtered = baseline::filter_new(diags, &bl);
    assert!(filtered.is_empty(), "expected no new violations after baseline, got {filtered:?}");
    let _ = std::fs::remove_file(tmp);
}

#[test]
fn baseline_filter_keeps_new_violations() {
    use konpu::analyze::baseline;
    use konpu::analyze::analyze_with_config;
    use konpu::analyze::template;
    let cfg = template::ResolvedConfig::empty();
    let diags_a = analyze_with_config(&fixture("monoid_missing_identity.rs"), &cfg);
    // Build baseline from a different fixture
    let diags_b = analyze_with_config(&fixture("monoid_valid.rs"), &cfg);
    let entries = baseline::entries_from(&diags_b);
    let bl: std::collections::HashSet<_> = entries.into_iter().collect();
    let filtered = baseline::filter_new(diags_a, &bl);
    assert!(!filtered.is_empty(), "expected new violations to remain");
}

#[test]
fn ignores_extracted_with_reason() {
    use konpu::analyze::analyze_full;
    use konpu::analyze::template;
    use konpu::domain::konpu::IgnoreReason;
    let cfg = template::ResolvedConfig::empty();
    let result = analyze_full(&fixture("with_ignore.rs"), &cfg);
    assert_eq!(result.ignores.len(), 1);
    let ig = &result.ignores[0];
    assert_eq!(ig.reason, IgnoreReason::Intentional);
    assert_eq!(ig.note.as_deref(), Some("skipped for now"));
}

#[test]
fn layer_expectation_mismatch_detected_for_ddd() {
    use konpu::analyze::analyze_full;
    use konpu::analyze::template;
    use konpu::domain::konpu::{AlgebraicStructure, HigherKindedStructure};
    let cfg = template::parse("preset = \"ddd\"\n");
    // Build a user layer that points at our fixture file. The DDD preset's
    // `domain` layer expects Monoid|Group; we declare magma, so it should
    // produce an expectation_mismatch.
    let user = template::parse(
        "preset = \"ddd\"\n[layers.domain]\npath = \"src/analyze/fixtures/layer_expectation_mismatch.rs\"\nexpect = [\"monoid\", \"group\"]\n",
    );
    let _ = cfg;
    let result = analyze_full(
        &fixture("layer_expectation_mismatch.rs"),
        &user,
    );
    assert_eq!(result.expectation_mismatches.len(), 1);
    let m = &result.expectation_mismatches[0];
    assert_eq!(m.layer_name, "domain");
    assert_eq!(m.type_name, "WeakDomain");
    assert!(m.reason.contains("Magma"));
    // the DDD domain layer does not require higher-kinded, so no higher mismatch
    assert!(!result.expectation_mismatches.iter().any(|m| m.reason.contains("higher")));
    let _ = (AlgebraicStructure::Magma, HigherKindedStructure::Functor);
}

#[test]
fn layer_expectation_mismatch_for_higher_kinded() {
    use konpu::analyze::analyze_full;
    use konpu::analyze::template;
    // infra layer expects `functor` higher; fixture declares `applicative` higher.
    let cfg = template::parse(
        "preset = \"ddd\"\n[layers.infra]\npath = \"src/analyze/fixtures/higher_mismatch.rs\"\nexpect = [\"functor\"]\n",
    );
    let result = analyze_full(&fixture("higher_mismatch.rs"), &cfg);
    let higher_mismatch = result
        .expectation_mismatches
        .iter()
        .find(|m| m.reason.contains("higher"));
    assert!(higher_mismatch.is_some(), "expected a higher-kinded mismatch, got: {:?}", result.expectation_mismatches);
}