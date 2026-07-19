use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::domain::konpu::{
    AlgebraicDeclaration, AlgebraicStructure, Diagnostic, DiagnosticRule, Law, OperationName,
    PropagationSize, Severity,
};
use crate::domain::fixtures::all_law_requirements;

use super::extract::{AnalyzedDeclaration, ImplInfo, LawTestInfo, MethodInfo, SelfKind};
use super::propagation::TypeInfo;
use super::template::{self, ResolvedConfig};

pub fn check_declaration(
    decl: &AnalyzedDeclaration,
    impls: &[ImplInfo],
    free_fns: &[MethodInfo],
    singletons: &[String],
    type_infos: &[TypeInfo],
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let declaration = AlgebraicDeclaration {
        targetStructure: decl.target_structure.clone(),
        higherKinded: decl.higher_kinded.clone(),
        operationName: OperationName,
        identityName: None,
        inverseName: None,
    };
    let matching: Vec<&ImplInfo> = impls
        .iter()
        .filter(|i| i.type_name == decl.type_name)
        .collect();
    let op_method = matching
        .iter()
        .flat_map(|i| i.methods.iter())
        .find(|m| m.name == decl.operation_name);

    let mut had_error = false;

    if decl.target_structure.rank() >= 2 {
        let id_name = decl.identity_name.as_deref();
        if id_name.is_none() || !has_op(&matching, free_fns, singletons, id_name.unwrap_or(""), &decl.type_name) {
            had_error = true;
            out.push(Diagnostic {
                severity: Severity::Error,
                declaration: declaration.clone(),
                rule: DiagnosticRule::MissingIdentity,
            });
        }
    }

    if decl.target_structure.rank() >= 3 {
        let inv_name = decl.inverse_name.as_deref();
        if inv_name.is_none() || !has_op(&matching, free_fns, singletons, inv_name.unwrap_or(""), &decl.type_name) {
            had_error = true;
            out.push(Diagnostic {
                severity: Severity::Error,
                declaration: declaration.clone(),
                rule: DiagnosticRule::MissingInverse,
            });
        }
    }

    if let Some(op) = op_method {
        let mut violated = false;
        if op.self_param == Some(SelfKind::MutRef) {
            violated = true;
        }
        match &op.return_type {
            None => violated = true,
            Some(t) => {
                let t = t.trim();
                if t == "()" || t.is_empty() {
                    violated = true;
                }
            }
        }
        if op.is_assoc_fn && op.params.len() != 2 {
            violated = true;
        }
        if violated {
            had_error = true;
            out.push(Diagnostic {
                severity: Severity::Error,
                declaration: declaration.clone(),
                rule: DiagnosticRule::ClosureViolation,
            });
        } else if !had_error && op_returns_self(decl, op) {
            // Closed binary op returning Self satisfies the *signature* necessary
            // conditions — but a float carrier or an impure body breaks
            // associativity in practice. Withhold confidence and warn instead of
            // reassuring falsely.
            if op.impure || carrier_contains_float(&decl.type_name, type_infos) {
                out.push(Diagnostic {
                    severity: Severity::Warning,
                    declaration: declaration.clone(),
                    rule: DiagnosticRule::KnownAssociativityRisk,
                });
            } else {
                out.push(Diagnostic {
                    severity: Severity::Info,
                    declaration: declaration.clone(),
                    rule: DiagnosticRule::AssociativityConfidence,
                });
            }
        }
    }

    if decl.higher_kinded.is_some() {
        let map_method = matching
            .iter()
            .flat_map(|i| i.methods.iter())
            .find(|m| m.name == "map");
        if let Some(map) = map_method {
            let map_violation = map.self_param == Some(SelfKind::MutRef)
                || map
                    .return_type
                    .as_deref()
                    .is_none_or(|t| t.trim() == "()" || t.trim().is_empty())
                || (!map.is_assoc_fn && map.params.len() != 1)
                || (map.is_assoc_fn && map.params.len() != 2);
            if map_violation {
                had_error = true;
                out.push(Diagnostic {
                    severity: Severity::Error,
                    declaration: declaration.clone(),
                    rule: DiagnosticRule::MapSignatureViolation,
                });
            }
        }
    }

    let _ = had_error;
    out
}

/// 単位元/逆元の存在:
/// - 型の impl メソッド、
/// - `ty` を返す同名の自由関数（oxidtr が receiver なし演算＝単位元を自由関数で出す形）、
/// - `name` という名のシングルトン型（oxidtr が Alloy `one sig` 単位元を
///   `struct Zero;` + `const _: Zero = Zero;` で出す形。annotation の identity が
///   nullary コンストラクタでなく識別値型を指すケース）。
fn has_op(impls: &[&ImplInfo], free_fns: &[MethodInfo], singletons: &[String], name: &str, ty: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    impls.iter().flat_map(|i| i.methods.iter()).any(|m| m.name == name)
        || free_fns
            .iter()
            .any(|f| f.name == name && ret_base_name(f).as_deref() == Some(ty))
        || singletons.iter().any(|s| s == name)
}

/// メソッドの戻り型の基底名（参照・ジェネリクス・パスを剥がす）。
/// パス修飾は Rust の `::` と TS/Swift/Kotlin の `.`（`M.Money` 等）の両方を剥がす。
fn is_float_ty(t: &str) -> bool {
    let t = t.trim().trim_start_matches('&').trim();
    // Rust f32/f64, Kotlin/Swift Double/Float. TS `number` is deliberately absent:
    // it conflates int and float, so flagging every numeric monoid would be noise.
    matches!(t, "f64" | "f32" | "Double" | "Float")
}

/// Does the carrier type resolve to (or transitively contain) a floating-point
/// field? Covers both `impl for f64` and newtypes like `struct FSum { v: f64 }`.
/// Floating-point arithmetic is not associative, so a law test is required.
fn carrier_contains_float(type_name: &str, type_infos: &[TypeInfo]) -> bool {
    fn go(name: &str, infos: &[TypeInfo], visited: &mut HashSet<String>) -> bool {
        if is_float_ty(name) {
            return true;
        }
        let base = name.trim().trim_start_matches('&').trim().to_string();
        if !visited.insert(base.clone()) {
            return false;
        }
        infos
            .iter()
            .find(|t| t.name == base)
            .is_some_and(|ti| ti.field_types.iter().any(|f| go(f, infos, visited)))
    }
    go(type_name, type_infos, &mut HashSet::new())
}

fn ret_base_name(m: &MethodInfo) -> Option<String> {
    let mut s = m.return_type.as_deref()?.trim();
    while let Some(r) = s.strip_prefix('&') {
        s = r.trim_start().strip_prefix("mut ").unwrap_or(r.trim_start()).trim_start();
    }
    let base = s.split('<').next().unwrap_or(s).trim();
    if base.is_empty() || base.contains(['[', '(', ',', ' ']) {
        return None;
    }
    let base = base.rsplit("::").next().unwrap_or(base);
    Some(base.rsplit('.').next().unwrap_or(base).to_string())
}

fn op_returns_self(decl: &AnalyzedDeclaration, op: &super::extract::MethodInfo) -> bool {
    let ret = match &op.return_type {
        Some(t) => t.trim().to_string(),
        None => return false,
    };
    let self_ret = ret == "Self" || ret == decl.type_name || ret.contains("Self");
    let self_param_ok = matches!(op.self_param, Some(SelfKind::Ref) | Some(SelfKind::Owned));
    if op.is_assoc_fn {
        if op.params.len() != 2 {
            return false;
        }
        if !self_ret {
            return false;
        }
        let all_self = op.params.iter().all(|p| {
            let p = p.trim();
            p == decl.type_name || p == "Self" || p.contains(&format!("{} ", decl.type_name))
        });
        if !all_self {
            return false;
        }
        return true;
    }
    if !self_param_ok {
        return false;
    }
    if !self_ret {
        return false;
    }
    true
}

pub fn required_laws_for(structure: &AlgebraicStructure) -> Vec<Law> {
    let mut out: Vec<Law> = all_law_requirements()
        .iter()
        .filter(|r| r.structure.rank() <= structure.rank())
        .map(|r| r.requiredLaw.clone())
        .collect();
    out.sort_by_key(law_index);
    out.dedup();
    out
}

fn law_index(l: &Law) -> usize {
    use Law::*;
    match l {
        Associativity => 0,
        LeftIdentity => 1,
        RightIdentity => 2,
        InverseLeft => 3,
        InverseRight => 4,
        FunctorIdentity => 5,
        FunctorComposition => 6,
        ApplicativeIdentity => 7,
        ApplicativeComposition => 8,
        MonadLeftIdentity => 9,
        MonadRightIdentity => 10,
        MonadAssociativity => 11,
    }
}

/// テスト出力（キャプチャ済み）から不通過テストの名前集合を返す。
/// konpu はテストを走らせず結果を参照するだけ（リンター原則）。2 形式を扱う:
/// - Rust libtest: 末尾の `failures:` ブロックが失敗テストの完全修飾名を
///   インデント付きで列挙。その clean list だけを拾う（パニック詳細ブロックは
///   直後の空行で state がリセットされ混入しない）。
/// - Swift XCTest: `Test Case '-[Module.Class method]' failed ...` 行。
///   末尾セグメント（`method`）を拾う。
///
/// どちらも末尾セグメント照合なので、別モジュール/クラスに同名テストがあると
/// 取り違えうる（ponytail: 実需が出たら完全修飾で照合）。
pub fn parse_failed_tests(test_output: &str) -> HashSet<String> {
    let mut failed = HashSet::new();
    let mut in_block = false;
    for line in test_output.lines() {
        // Swift XCTest / Kotlin(Gradle) の失敗行（行単位・state 非依存）。
        if let Some(name) = swift_failed_test(line).or_else(|| kotlin_failed_test(line)) {
            failed.insert(name);
            continue;
        }
        // Rust libtest の failures: ブロック。
        if line.trim_end() == "failures:" {
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }
        match line.strip_prefix("    ") {
            Some(name) if !name.trim().is_empty() && !name.trim().contains(' ') => {
                let name = name.trim();
                failed.insert(name.rsplit("::").next().unwrap_or(name).to_string());
            }
            _ => in_block = false,
        }
    }
    failed
}

/// Swift XCTest の失敗行から末尾のメソッド名を取る。
/// 例: `Test Case '-[MyTests.MoneyTests testAssoc]' failed (0.1 seconds).` → `testAssoc`。
fn swift_failed_test(line: &str) -> Option<String> {
    let line = line.trim();
    if !line.starts_with("Test Case") || !line.contains("failed") {
        return None;
    }
    let inner = line.split('[').nth(1)?.split(']').next()?; // `Module.Class method`
    let method = inner.split_whitespace().last()?;
    (!method.is_empty()).then(|| method.to_string())
}

/// Kotlin(Gradle) の失敗行からテスト名を取る。
/// 例: `MoneyTest > combineIsAssociative FAILED` / `... > testAssoc() FAILED` → `combineIsAssociative`。
fn kotlin_failed_test(line: &str) -> Option<String> {
    let line = line.trim();
    let body = line.strip_suffix("FAILED")?.trim_end();
    // `Class > method` の末尾（複数階層 `Class > Nested > method` にも対応）。
    let name = body.rsplit(" > ").next()?.trim();
    let name = name.trim_end_matches("()");
    (!name.is_empty() && !name.contains(' ')).then(|| name.to_string())
}

/// 1 つの (宣言, 法則) のテスト状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LawStatus {
    /// テストが存在し通過（または `--test-results` 無しで存在のみ確認）。
    Passing,
    /// テストは存在するが `--test-results` で不通過が確認された。
    Failing,
    /// この法則を検証するテストが無い。
    Missing,
}

/// `decl` の `law` を検証するテストの状態を返す。診断とレポート集計で共有。
fn classify_law(
    decl: &AnalyzedDeclaration,
    law: &Law,
    law_tests: &[LawTestInfo],
    failed_tests: &HashSet<String>,
) -> LawStatus {
    let covering: Vec<&LawTestInfo> = law_tests
        .iter()
        .filter(|t| {
            t.laws.iter().any(|l| l == law)
                && (t.enclosing_type.is_none() || t.enclosing_type.as_deref() == Some(&decl.type_name))
        })
        .collect();
    if covering.is_empty() {
        LawStatus::Missing
    } else if covering
        .iter()
        .any(|t| t.test_fn.as_deref().is_some_and(|f| failed_tests.contains(f)))
    {
        LawStatus::Failing
    } else {
        LawStatus::Passing
    }
}

pub fn check_law_tests(
    decls: &[AnalyzedDeclaration],
    law_tests: &[LawTestInfo],
    failed_tests: &HashSet<String>,
) -> Vec<(PathBuf, usize, Diagnostic)> {
    let mut out = Vec::new();
    for decl in decls {
        if decl.target_structure.rank() < 1 {
            continue;
        }
        let declaration = || AlgebraicDeclaration {
            targetStructure: decl.target_structure.clone(),
            higherKinded: decl.higher_kinded.clone(),
            operationName: OperationName,
            identityName: None,
            inverseName: None,
        };
        for law in &required_laws_for(&decl.target_structure) {
            let (severity, rule) = match classify_law(decl, law, law_tests, failed_tests) {
                LawStatus::Missing => (Severity::Warning, DiagnosticRule::MissingLawTest),
                LawStatus::Failing => (Severity::Error, DiagnosticRule::FailingLawTest),
                LawStatus::Passing => continue,
            };
            out.push((decl.path.clone(), decl.line, Diagnostic { severity, declaration: declaration(), rule }));
        }
    }
    out
}

/// 1 宣言分の充足ギャップ集計。`gap` = 1 - passing/required（roadmap 軸2）。
/// missing も failing も「未充足」に数える（テスト充足率であって真の充足度ではない）。
#[derive(Debug, Clone)]
pub struct LawCompliance {
    pub type_name: String,
    pub structure: AlgebraicStructure,
    pub required: usize,
    pub passing: usize,
    pub failing: usize,
    pub missing: usize,
}

impl LawCompliance {
    pub fn gap(&self) -> f64 {
        if self.required == 0 {
            0.0
        } else {
            1.0 - (self.passing as f64) / (self.required as f64)
        }
    }
}

/// 各宣言（rank>=1）の法則充足を集計する。`report` の充足ギャップ表示用。
pub fn law_compliance(
    decls: &[AnalyzedDeclaration],
    law_tests: &[LawTestInfo],
    failed_tests: &HashSet<String>,
) -> Vec<LawCompliance> {
    let mut out = Vec::new();
    for decl in decls {
        if decl.target_structure.rank() < 1 {
            continue;
        }
        let (mut passing, mut failing, mut missing) = (0, 0, 0);
        let required = required_laws_for(&decl.target_structure);
        for law in &required {
            match classify_law(decl, law, law_tests, failed_tests) {
                LawStatus::Passing => passing += 1,
                LawStatus::Failing => failing += 1,
                LawStatus::Missing => missing += 1,
            }
        }
        out.push(LawCompliance {
            type_name: decl.type_name.clone(),
            structure: decl.target_structure.clone(),
            required: required.len(),
            passing,
            failing,
            missing,
        });
    }
    out
}

/// 文脈伝播度の上限を超過している場合、`PropagationExceeded` (Warning) を出す。
pub fn check_propagation(
    decl: &AnalyzedDeclaration,
    config: &ResolvedConfig,
    root: &Path,
) -> Vec<Diagnostic> {
    let Some(size) = &decl.propagation else {
        return Vec::new();
    };
    let layer = template::match_layer(config, &decl.path, root);
    let threshold = template::threshold(config, layer);
    let declaration = AlgebraicDeclaration {
        targetStructure: decl.target_structure.clone(),
        higherKinded: decl.higher_kinded.clone(),
        operationName: OperationName,
        identityName: None,
        inverseName: None,
    };
    let mut out = Vec::new();
    match threshold {
        None => {}
        Some(-1) => {}
        Some(max) if max > 0 && size == &PropagationSize::Unbounded => {
            out.push(Diagnostic {
                severity: Severity::Warning,
                declaration,
                rule: DiagnosticRule::PropagationExceeded,
            });
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(ty: &str, s: AlgebraicStructure) -> AnalyzedDeclaration {
        AnalyzedDeclaration {
            target_structure: s,
            higher_kinded: None,
            type_name: ty.to_string(),
            operation_name: "combine".to_string(),
            identity_name: None,
            inverse_name: None,
            path: PathBuf::from("src/x.rs"),
            line: 1,
            propagation: None,
        }
    }

    fn law_test(ty: &str, fn_name: &str) -> LawTestInfo {
        LawTestInfo {
            laws: vec![Law::Associativity],
            enclosing_type: Some(ty.to_string()),
            test_fn: Some(fn_name.to_string()),
            path: PathBuf::from("src/x.rs"),
            line: 1,
        }
    }

    #[test]
    fn parse_failed_tests_picks_the_clean_failures_list() {
        let out = "\
running 3 tests
test tests::passing ... ok
test tests::money_assoc ... FAILED

failures:

---- tests::money_assoc stdout ----
thread 'tests::money_assoc' panicked at src/lib.rs:42:9:
assertion `left == right` failed
  left: 5
 right: 6

failures:
    tests::money_assoc

test result: FAILED. 1 passed; 1 failed; 0 ignored";
        let failed = parse_failed_tests(out);
        assert_eq!(failed.len(), 1);
        assert!(failed.contains("money_assoc"));
    }

    #[test]
    fn parse_failed_tests_swift_xctest_format() {
        let out = "\
Test Suite 'MoneyTests' started
Test Case '-[WalletTests.MoneyTests testAssoc]' passed (0.001 seconds).
Test Case '-[WalletTests.MoneyTests testLeftId]' failed (0.002 seconds).
Test Suite 'MoneyTests' failed";
        let failed = parse_failed_tests(out);
        assert_eq!(failed.len(), 1);
        assert!(failed.contains("testLeftId"));
    }

    #[test]
    fn parse_failed_tests_kotlin_gradle_format() {
        let out = "\
MoneyTest > combineIsAssociative PASSED
MoneyTest > zeroIsLeftIdentity FAILED
MoneyTest > zeroIsRightIdentity() FAILED";
        let failed = parse_failed_tests(out);
        assert_eq!(failed.len(), 2);
        assert!(failed.contains("zeroIsLeftIdentity"));
        assert!(failed.contains("zeroIsRightIdentity"));
    }

    #[test]
    fn parse_failed_tests_empty_when_all_pass() {
        let out = "test tests::a ... ok\n\ntest result: ok. 1 passed; 0 failed";
        assert!(parse_failed_tests(out).is_empty());
    }

    #[test]
    fn covered_and_passing_yields_no_diagnostic() {
        let decls = vec![decl("Money", AlgebraicStructure::Semigroup)];
        let tests = vec![law_test("Money", "money_assoc")];
        let failed = HashSet::new(); // nothing failed
        let out = check_law_tests(&decls, &tests, &failed);
        assert!(out.is_empty());
    }

    #[test]
    fn covered_but_failing_yields_failing_law_test_error() {
        let decls = vec![decl("Money", AlgebraicStructure::Semigroup)];
        let tests = vec![law_test("Money", "money_assoc")];
        let failed = HashSet::from(["money_assoc".to_string()]);
        let out = check_law_tests(&decls, &tests, &failed);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].2.rule, DiagnosticRule::FailingLawTest);
        assert_eq!(out[0].2.severity, Severity::Error);
    }

    #[test]
    fn compliance_gap_counts_pass_fail_missing() {
        // Monoid requires 3 laws (assoc, left_id, right_id). Provide an assoc
        // test that fails and a left_identity test that passes; right_id missing.
        let decls = vec![decl("Money", AlgebraicStructure::Monoid)];
        let tests = vec![
            law_test("Money", "assoc_fn"), // laws: [Associativity]
            LawTestInfo {
                laws: vec![Law::LeftIdentity],
                enclosing_type: Some("Money".to_string()),
                test_fn: Some("left_id_fn".to_string()),
                path: PathBuf::from("src/x.rs"),
                line: 1,
            },
        ];
        let failed = HashSet::from(["assoc_fn".to_string()]);
        let c = &law_compliance(&decls, &tests, &failed)[0];
        assert_eq!(c.required, 3);
        assert_eq!(c.passing, 1); // left_identity
        assert_eq!(c.failing, 1); // associativity
        assert_eq!(c.missing, 1); // right_identity
        assert!((c.gap() - (1.0 - 1.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn compliance_gap_zero_when_all_pass() {
        let decls = vec![decl("S", AlgebraicStructure::Semigroup)]; // needs assoc only
        let tests = vec![law_test("S", "s_assoc")];
        let c = &law_compliance(&decls, &tests, &HashSet::new())[0];
        assert_eq!(c.required, 1);
        assert_eq!(c.passing, 1);
        assert_eq!(c.gap(), 0.0);
    }

    #[test]
    fn uncovered_still_yields_missing_law_test_warning() {
        let decls = vec![decl("Money", AlgebraicStructure::Semigroup)];
        let out = check_law_tests(&decls, &[], &HashSet::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].2.rule, DiagnosticRule::MissingLawTest);
        assert_eq!(out[0].2.severity, Severity::Warning);
    }

    fn combine_method(ret: &str) -> MethodInfo {
        MethodInfo {
            name: "combine".to_string(),
            self_param: Some(SelfKind::Ref),
            params: vec!["other: &Self".to_string()],
            return_type: Some(ret.to_string()),
            is_assoc_fn: false,
            impure: false,
        }
    }

    fn tinfo(name: &str, fields: &[&str]) -> super::super::propagation::TypeInfo {
        super::super::propagation::TypeInfo {
            name: name.to_string(),
            kind: super::super::propagation::TypeKind::Struct,
            variant_count: 0,
            field_types: fields.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn float_carrier_downgrades_confidence_to_risk() {
        // FSum wraps f64 — associativity is NOT guaranteed (float arithmetic).
        // konpu must NOT emit reassuring AssociativityConfidence; it must warn.
        let d = decl("FSum", AlgebraicStructure::Semigroup);
        let impls = vec![ImplInfo { type_name: "FSum".to_string(), methods: vec![combine_method("FSum")] }];
        let tinfos = vec![tinfo("FSum", &["f64"])];
        let out = check_declaration(&d, &impls, &[], &[], &tinfos);
        assert!(out.iter().any(|x| x.rule == DiagnosticRule::KnownAssociativityRisk
            && x.severity == Severity::Warning));
        assert!(!out.iter().any(|x| x.rule == DiagnosticRule::AssociativityConfidence));
    }

    #[test]
    fn direct_float_carrier_downgrades() {
        // impl over f64 itself.
        let d = decl("f64", AlgebraicStructure::Semigroup);
        let impls = vec![ImplInfo { type_name: "f64".to_string(), methods: vec![combine_method("f64")] }];
        let out = check_declaration(&d, &impls, &[], &[], &[]);
        assert!(out.iter().any(|x| x.rule == DiagnosticRule::KnownAssociativityRisk));
    }

    #[test]
    fn impure_op_downgrades_confidence_to_risk() {
        // Op body touches external state (impure=true from extractor) — associativity
        // is not guaranteed even though the signature is closed.
        let d = decl("Counter", AlgebraicStructure::Semigroup);
        let mut m = combine_method("Counter");
        m.impure = true;
        let impls = vec![ImplInfo { type_name: "Counter".to_string(), methods: vec![m] }];
        let out = check_declaration(&d, &impls, &[], &[], &[]);
        assert!(out.iter().any(|x| x.rule == DiagnosticRule::KnownAssociativityRisk));
        assert!(!out.iter().any(|x| x.rule == DiagnosticRule::AssociativityConfidence));
    }

    #[test]
    fn double_float_carriers_downgrade_kotlin_swift() {
        // Kotlin/Swift Double/Float carriers are non-associative too.
        for fty in ["Double", "Float"] {
            let d = decl("KSum", AlgebraicStructure::Semigroup);
            let impls =
                vec![ImplInfo { type_name: "KSum".to_string(), methods: vec![combine_method("KSum")] }];
            let tinfos = vec![tinfo("KSum", &[fty])];
            let out = check_declaration(&d, &impls, &[], &[], &tinfos);
            assert!(
                out.iter().any(|x| x.rule == DiagnosticRule::KnownAssociativityRisk),
                "carrier field {fty} should be flagged"
            );
        }
    }

    #[test]
    fn non_float_carrier_keeps_confidence() {
        let d = decl("Log", AlgebraicStructure::Semigroup);
        let impls = vec![ImplInfo { type_name: "Log".to_string(), methods: vec![combine_method("Log")] }];
        let tinfos = vec![tinfo("Log", &["String"])];
        let out = check_declaration(&d, &impls, &[], &[], &tinfos);
        assert!(out.iter().any(|x| x.rule == DiagnosticRule::AssociativityConfidence));
        assert!(!out.iter().any(|x| x.rule == DiagnosticRule::KnownAssociativityRisk));
    }
}

