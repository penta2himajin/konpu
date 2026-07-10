use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::domain::konpu::{
    AlgebraicDeclaration, AlgebraicStructure, Diagnostic, DiagnosticRule, Law, OperationName,
    PropagationSize, Severity,
};
use crate::domain::fixtures::all_law_requirements;

use super::extract::{AnalyzedDeclaration, ImplInfo, LawTestInfo, MethodInfo, SelfKind};
use super::template::{self, ResolvedConfig};

pub fn check_declaration(
    decl: &AnalyzedDeclaration,
    impls: &[ImplInfo],
    free_fns: &[MethodInfo],
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
        if id_name.is_none() || !has_op(&matching, free_fns, id_name.unwrap_or(""), &decl.type_name) {
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
        if inv_name.is_none() || !has_op(&matching, free_fns, inv_name.unwrap_or(""), &decl.type_name) {
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
            out.push(Diagnostic {
                severity: Severity::Info,
                declaration: declaration.clone(),
                rule: DiagnosticRule::AssociativityConfidence,
            });
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

/// 単位元/逆元の存在: 型の impl メソッド、または `ty` を返す同名の自由関数。
/// 後者は oxidtr が receiver なし演算（単位元）を自由関数として出す形に対応する
/// （infer 側と同じ帰属規則）。
fn has_op(impls: &[&ImplInfo], free_fns: &[MethodInfo], name: &str, ty: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    impls.iter().flat_map(|i| i.methods.iter()).any(|m| m.name == name)
        || free_fns
            .iter()
            .any(|f| f.name == name && ret_base_name(f).as_deref() == Some(ty))
}

/// メソッドの戻り型の基底名（参照・ジェネリクス・パスを剥がす）。
fn ret_base_name(m: &MethodInfo) -> Option<String> {
    let mut s = m.return_type.as_deref()?.trim();
    while let Some(r) = s.strip_prefix('&') {
        s = r.trim_start().strip_prefix("mut ").unwrap_or(r.trim_start()).trim_start();
    }
    let base = s.split('<').next().unwrap_or(s).trim();
    if base.is_empty() || base.contains(['[', '(', ',', ' ']) {
        return None;
    }
    Some(base.rsplit("::").next().unwrap_or(base).to_string())
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

/// `cargo test` の出力（キャプチャ済み）から不通過テストの名前集合を返す。
/// konpu はテストを走らせず結果を参照するだけ（リンター原則）。libtest は
/// 失敗時に末尾へ `failures:` ブロックを出し、失敗テストの完全修飾名を
/// インデント付きで列挙する。その clean list（`    module::name` 形式）だけを
/// 拾い、末尾セグメント（`name`）に正規化する。パニック詳細ブロックは直後に
/// 空行が来て state がリセットされるため混入しない。
///
/// ponytail: 末尾セグメント照合なので、別モジュールに同名テストがあると
/// 取り違えうる。実害が出たら module パス込みで照合する。
pub fn parse_failed_tests(cargo_test_output: &str) -> HashSet<String> {
    let mut failed = HashSet::new();
    let mut in_block = false;
    for line in cargo_test_output.lines() {
        if line.trim_end() == "failures:" {
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }
        match line.strip_prefix("    ") {
            // 単一トークン（空白なし）＝失敗テストの完全修飾名。
            Some(name) if !name.trim().is_empty() && !name.trim().contains(' ') => {
                let name = name.trim();
                failed.insert(name.rsplit("::").next().unwrap_or(name).to_string());
            }
            // 空行や非インデント行（`test result:` 等）でブロック終了。
            _ => in_block = false,
        }
    }
    failed
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
}

