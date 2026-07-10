//! コールグラフを使った preserve 検査（`call-graph` feature 時のみ）。
//!
//! 不変条件（本セッションで合意）: `from`→`to` を跨ぐ構造化型 `T` は、`T` の構造が
//! 許す代数サーフェス `{operation, identity, inverse}` を経由してのみ生成・併合される。
//! 検出器 B（集約保存）: to 層で「複数の `T` を 1 個の `T` に併合する」形の関数は、
//! `T` の `operation`（combine）に到達必須。未到達なら手書きマージの疑いとして報告。
//!
//! 精度: 実効深刻度は law_test の有無で調整（法則が検証済みの型ほど強い）。
//! best-effort な箇所（シグネチャ型の末尾セグメント照合、join、到達可能性の見逃し）は
//! それぞれ `// ponytail:` で改善経路を明記する。

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use super::call_graph::{
    is_aggregation_shape, param_is_type, CallGraph, Facts, FnSig, FuncId, Precision,
};
use super::extract::{AnalyzedDeclaration, LawTestInfo};
use super::template::{PreserveSeverity, ResolvedConfig};
use crate::domain::konpu::Severity;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreserveKind {
    /// 検出器 B: 集約が operation に到達しない。
    Aggregate,
    /// 検出器 C: 手書きマージ構築（Stage 4）。
    Construct,
}

#[derive(Debug, Clone)]
pub struct PreserveFinding {
    pub boundary: String,
    pub type_name: String,
    pub function: String,
    pub path: PathBuf,
    pub line: usize,
    pub kind: PreserveKind,
    pub severity: Severity,
    pub reason: String,
}

/// preserve 検査本体。decls/law_tests は tree-sitter 由来、facts はコールグラフ、
/// fn_sigs は to 層関数のシグネチャ。
pub fn check_preserve(
    decls: &[AnalyzedDeclaration],
    law_tests: &[LawTestInfo],
    config: &ResolvedConfig,
    facts: &Facts,
    fn_sigs: &[FnSig],
    root: &Path,
) -> Vec<PreserveFinding> {
    let graph = CallGraph::build(facts, Precision::Rta);
    let mut out = Vec::new();

    for b in &config.boundaries {
        if b.preserve.is_empty() || b.preserve_severity == PreserveSeverity::Off {
            continue;
        }
        if !b.preserve_checks.aggregate && !b.preserve_checks.construct {
            continue;
        }
        let from_decls: Vec<&AnalyzedDeclaration> = decls
            .iter()
            .filter(|d| {
                glob_match_str(&b.from_pattern, &strip_root(&d.path, root))
                    && b.preserve.contains(&d.target_structure)
            })
            .collect();
        if from_decls.is_empty() {
            continue;
        }
        for d in &from_decls {
            let ty = &d.type_name;
            let op = &d.operation_name;
            let targets: HashSet<FuncId> = resolve_method(facts, ty, op).into_iter().collect();
            if targets.is_empty() {
                // operation を FuncId に解決できない（外部/未索引）→ 判定不能でスキップ。
                // ponytail: SCIP に operation 定義が無いケース。改善経路: 抽出範囲の拡張。
                continue;
            }
            let has_law = law_tests
                .iter()
                .any(|lt| lt.enclosing_type.as_deref() == Some(ty.as_str()));
            let Some(severity) = effective_severity(b.preserve_severity, has_law) else {
                continue;
            };
            // to 層かつ operation に未到達か、を判定する共通クロージャ。
            let to_layer_bypasses = |sig: &FnSig| -> bool {
                let Some(fid) = resolve_funcid(sig, facts) else {
                    return false;
                };
                glob_match_str(&b.to_pattern, &facts.funcs[fid].path.to_string_lossy())
                    && !reaches(&graph, fid, &targets)
            };
            // 検出器 B（集約保存）。C と重複しないよう対象関数を記録する。
            let mut flagged: HashSet<(PathBuf, usize)> = HashSet::new();
            if b.preserve_checks.aggregate {
                for sig in fn_sigs {
                    if !is_aggregation_shape(sig, ty) || !to_layer_bypasses(sig) {
                        continue;
                    }
                    flagged.insert((sig.path.clone(), sig.line));
                    out.push(PreserveFinding {
                        boundary: b.name.clone(),
                        type_name: ty.clone(),
                        function: sig.name.clone(),
                        path: sig.path.clone(),
                        line: sig.line,
                        kind: PreserveKind::Aggregate,
                        severity: severity.clone(),
                        reason: format!(
                            "aggregates `{ty}` but never reaches its `{op}` operation across boundary `{}` (possible hand-rolled merge)",
                            b.name
                        ),
                    });
                }
            }
            // 検出器 C（手書きマージ構築）。B が拾った関数は除く。
            if b.preserve_checks.construct {
                for sig in fn_sigs {
                    if flagged.contains(&(sig.path.clone(), sig.line)) {
                        continue;
                    }
                    let Some(cline) = hand_rolled_merge(sig, ty) else {
                        continue;
                    };
                    if !to_layer_bypasses(sig) {
                        continue;
                    }
                    out.push(PreserveFinding {
                        boundary: b.name.clone(),
                        type_name: ty.clone(),
                        function: sig.name.clone(),
                        path: sig.path.clone(),
                        line: cline,
                        kind: PreserveKind::Construct,
                        severity: severity.clone(),
                        reason: format!(
                            "constructs `{ty}` by hand-merging >=2 `{ty}` values without reaching its `{op}` operation across boundary `{}`",
                            b.name
                        ),
                    });
                }
            }
        }
    }
    out
}

/// 検出器 C: `sig` が「≥2 個の `ty` 型値を生構築で併合する」手書きマージを含むか。
/// 含めばその構築の行を返す。to 層で `ty` を扱うがシグネチャにマージが現れない
/// （例 `fn h(a: T, b: T) -> Response`）ケースを、構築サイトのデータフローで拾う。
// ponytail: T 型変数は「T 型の引数 + self」のみ追跡する近似。ループ変数や
// `let` で束ねた中間 T（アキュムレータ等）は追わない — その形は検出器 B が拾う。
// 改善経路: 関数内データフロー。
fn hand_rolled_merge(sig: &FnSig, ty: &str) -> Option<usize> {
    let self_ty = sig.self_type.as_deref();
    let mut t_vars: HashSet<&str> = HashSet::new();
    if self_ty == Some(ty) {
        t_vars.insert("self");
    }
    for (name, pty) in &sig.params_named {
        if param_is_type(pty, ty, self_ty) {
            t_vars.insert(name.as_str());
        }
    }
    for c in &sig.constructions {
        if c.type_name != ty {
            continue;
        }
        let distinct_t = c.refs.iter().filter(|r| t_vars.contains(r.as_str())).count();
        if distinct_t >= 2 {
            return Some(c.line);
        }
    }
    None
}

/// 設定深刻度 + law_test の有無から実効深刻度を決める。law_test 無しは一段降格。
fn effective_severity(configured: PreserveSeverity, has_law: bool) -> Option<Severity> {
    match (configured, has_law) {
        (PreserveSeverity::Off, _) => None,
        (PreserveSeverity::Error, true) => Some(Severity::Error),
        (PreserveSeverity::Error, false) => Some(Severity::Warning),
        (PreserveSeverity::Warn, true) => Some(Severity::Warning),
        (PreserveSeverity::Warn, false) => Some(Severity::Info),
    }
}

/// 型 `ty` のメソッド `method` を表す FuncId 群（型修飾で厳密に照合）。
/// SCIP シンボル名の `impl#[ty]...method` / `impl#[ty][Trait]method` を見る。
fn resolve_method(facts: &Facts, ty: &str, method: &str) -> Vec<FuncId> {
    facts
        .funcs
        .iter()
        .enumerate()
        .filter(|(_, f)| method_name(&f.name) == method && for_type_of(&f.name) == Some(ty))
        .map(|(i, _)| i)
        .collect()
}

/// SCIP 名の末尾メソッド名（`impl#[T][Tr]combine` → `combine`, `total` → `total`）。
fn method_name(scip_name: &str) -> &str {
    let after_bracket = scip_name.rsplit(']').next().unwrap_or(scip_name);
    after_bracket.rsplit('/').next().unwrap_or(after_bracket)
}

/// SCIP 名の impl 対象型（`impl#[Money][Tr]combine` → `Money`）。無ければ None。
fn for_type_of(scip_name: &str) -> Option<&str> {
    let idx = scip_name.find("impl#[")?;
    scip_name[idx + "impl#[".len()..].split(']').next()
}

/// FnSig（tree-sitter）を Facts の FuncId に結合する。パス末尾一致 + メソッド名一致、
/// 行の近さでタイブレーク。
// ponytail: パスは末尾一致、行は近接一致の best-effort join。属性/マクロで行がずれた
// 同名メソッドが同ファイルに複数あると取り違えうる。改善経路: SCIP の定義行との厳密対応。
fn resolve_funcid(sig: &FnSig, facts: &Facts) -> Option<FuncId> {
    let sig_path = sig.path.to_string_lossy();
    facts
        .funcs
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            method_name(&f.name) == sig.name && path_ends_with(&sig_path, &f.path.to_string_lossy())
        })
        .min_by_key(|(_, f)| f.line.abs_diff(sig.line))
        .map(|(i, _)| i)
}

fn path_ends_with(a: &str, b: &str) -> bool {
    a == b || a.ends_with(b) || b.ends_with(a)
}

/// start から targets のいずれかへ到達できるか（RTA グラフ上の BFS）。
// ponytail: マクロ生成呼び出しや fn ポインタ経由の呼び出しはグラフに現れず、到達を
// 見逃す＝偽陽性方向。改善経路: データフロー / より精密な事実抽出。
fn reaches(graph: &CallGraph, start: FuncId, targets: &HashSet<FuncId>) -> bool {
    if targets.contains(&start) {
        return true;
    }
    let mut seen = vec![false; graph.edges.len()];
    let mut queue = VecDeque::new();
    if start < seen.len() {
        seen[start] = true;
        queue.push_back(start);
    }
    while let Some(n) = queue.pop_front() {
        for &m in &graph.edges[n] {
            if targets.contains(&m) {
                return true;
            }
            if !seen[m] {
                seen[m] = true;
                queue.push_back(m);
            }
        }
    }
    false
}

/// 解析ルート起点の相対パス文字列。SCIP の相対パスと同じ空間に揃える。
fn strip_root(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .trim_start_matches("./")
        .to_string()
}

fn glob_match_str(pattern: &str, path: &str) -> bool {
    super::template::glob_match(pattern, path.trim_start_matches("./"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::call_graph::FnSig;
    use crate::analyze::template::{PreserveChecks, ResolvedBoundary, ResolvedConfig};
    use crate::domain::konpu::{AlgebraicStructure, Law};
    use konpu_cg::CallSite;

    fn decl(ty: &str, path: &str, op: &str) -> AnalyzedDeclaration {
        AnalyzedDeclaration {
            target_structure: AlgebraicStructure::Monoid,
            higher_kinded: None,
            type_name: ty.to_string(),
            operation_name: op.to_string(),
            identity_name: Some("zero".to_string()),
            inverse_name: None,
            path: PathBuf::from(path),
            line: 1,
            propagation: None,
        }
    }

    fn sig(name: &str, path: &str, line: usize, self_ty: Option<&str>, params: &[&str], ret: &str) -> FnSig {
        FnSig {
            path: PathBuf::from(path),
            line,
            name: name.to_string(),
            self_type: self_ty.map(str::to_string),
            params: params.iter().map(|s| s.to_string()).collect(),
            params_named: Vec::new(),
            ret: Some(ret.to_string()),
            constructions: Vec::new(),
        }
    }

    fn config(sev: PreserveSeverity) -> ResolvedConfig {
        ResolvedConfig {
            defaults_max: None,
            layers: Vec::new(),
            boundaries: vec![ResolvedBoundary {
                name: "d2i".to_string(),
                from_pattern: "src/domain/**".to_string(),
                to_pattern: "src/infra/**".to_string(),
                preserve: vec![AlgebraicStructure::Monoid],
                preserve_severity: sev,
                preserve_checks: PreserveChecks { aggregate: true, construct: true },
            }],
            exclude: Vec::new(),
            callgraph_hub_threshold: None,
            infer: false,
        }
    }

    // to-layer: good_merge reaches Money::combine; hand_merge does not.
    fn facts() -> Facts {
        let mut f = Facts::default();
        let combine = f.add_func("impl#[Money]combine", "src/domain/money.rs", 5);
        let good = f.add_func("good_merge", "src/infra/repo.rs", 10);
        let hand = f.add_func("hand_merge", "src/infra/repo.rs", 20);
        f.calls.push(CallSite { caller: good, target: konpu_cg::CallTargetKind::Static(combine) });
        // hand_merge calls nothing relevant
        let _ = hand;
        f
    }

    fn law_for(ty: &str) -> Vec<LawTestInfo> {
        vec![LawTestInfo {
            laws: vec![Law::Associativity],
            enclosing_type: Some(ty.to_string()),
            test_fn: None,
            path: PathBuf::from("src/domain/money.rs"),
            line: 30,
        }]
    }

    #[test]
    fn flags_aggregation_that_bypasses_operation() {
        let facts = facts();
        let decls = vec![decl("Money", "src/domain/money.rs", "combine")];
        let sigs = vec![
            sig("good_merge", "src/infra/repo.rs", 10, None, &["&[Money]"], "Money"),
            sig("hand_merge", "src/infra/repo.rs", 20, None, &["Money", "Money"], "Money"),
        ];
        let out = check_preserve(&decls, &law_for("Money"), &config(PreserveSeverity::Warn), &facts, &sigs, std::path::Path::new(""));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].function, "hand_merge");
        assert_eq!(out[0].kind, PreserveKind::Aggregate);
        // has law test + configured warn => Warning
        assert_eq!(out[0].severity, Severity::Warning);
    }

    #[test]
    fn no_law_test_downgrades_severity() {
        let facts = facts();
        let decls = vec![decl("Money", "src/domain/money.rs", "combine")];
        let sigs = vec![sig("hand_merge", "src/infra/repo.rs", 20, None, &["Money", "Money"], "Money")];
        // no law tests => warn downgrades to Info
        let out = check_preserve(&decls, &[], &config(PreserveSeverity::Warn), &facts, &sigs, std::path::Path::new(""));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity, Severity::Info);
    }

    #[test]
    fn severity_off_suppresses() {
        let facts = facts();
        let decls = vec![decl("Money", "src/domain/money.rs", "combine")];
        let sigs = vec![sig("hand_merge", "src/infra/repo.rs", 20, None, &["Money", "Money"], "Money")];
        let out = check_preserve(&decls, &law_for("Money"), &config(PreserveSeverity::Off), &facts, &sigs, std::path::Path::new(""));
        assert!(out.is_empty());
    }

    #[test]
    fn detector_c_flags_hidden_hand_rolled_merge() {
        use crate::analyze::call_graph::MergeConstruction;
        // `convert(a: Money, b: Money) -> Response` — signature hides the merge
        // (returns Response), so B misses it; C catches the Money{..} that
        // combines a and b without reaching combine.
        let mut f = Facts::default();
        f.add_func("impl#[Money]combine", "src/domain/money.rs", 5);
        f.add_func("convert", "src/infra/repo.rs", 40); // calls nothing
        let decls = vec![decl("Money", "src/domain/money.rs", "combine")];
        let mut s = sig("convert", "src/infra/repo.rs", 40, None, &["Money", "Money"], "Response");
        s.params_named = vec![("a".into(), "Money".into()), ("b".into(), "Money".into())];
        s.constructions = vec![MergeConstruction {
            type_name: "Money".into(),
            line: 42,
            refs: vec!["a".into(), "b".into()],
        }];
        let out = check_preserve(&decls, &law_for("Money"), &config(PreserveSeverity::Warn), &f, &[s], Path::new(""));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, PreserveKind::Construct);
        assert_eq!(out[0].line, 42);
    }

    #[test]
    fn detector_c_ignores_single_source_construction() {
        use crate::analyze::call_graph::MergeConstruction;
        // constructs Money from ONE Money (a transform) + a primitive — not a merge.
        let mut f = Facts::default();
        f.add_func("impl#[Money]combine", "src/domain/money.rs", 5);
        f.add_func("scale", "src/infra/repo.rs", 40);
        let decls = vec![decl("Money", "src/domain/money.rs", "combine")];
        let mut s = sig("scale", "src/infra/repo.rs", 40, None, &["Money", "u64"], "Money");
        s.params_named = vec![("a".into(), "Money".into()), ("k".into(), "u64".into())];
        s.constructions = vec![MergeConstruction {
            type_name: "Money".into(),
            line: 42,
            refs: vec!["a".into(), "k".into()], // only one Money source
        }];
        let out = check_preserve(&decls, &law_for("Money"), &config(PreserveSeverity::Warn), &f, &[s], Path::new(""));
        // aggregation shape? a:Money,k:u64 -> Money: only 1 Money input, not aggregate.
        // C? only 1 distinct Money ref. So nothing.
        assert!(out.is_empty());
    }

    #[test]
    fn non_aggregation_not_flagged() {
        let facts = facts();
        let decls = vec![decl("Money", "src/domain/money.rs", "combine")];
        // singleton constructor: not aggregation shape
        let sigs = vec![sig("make", "src/infra/repo.rs", 20, None, &["u64"], "Money")];
        let out = check_preserve(&decls, &law_for("Money"), &config(PreserveSeverity::Warn), &facts, &sigs, std::path::Path::new(""));
        assert!(out.is_empty());
    }
}
