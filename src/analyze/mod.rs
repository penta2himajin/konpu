pub mod baseline;
pub mod call_graph;
pub mod check;
pub mod extract;
pub mod parser;
#[cfg(feature = "call-graph")]
pub mod preserve_cg;
pub mod propagation;
pub mod scaffold;
pub mod template;

use std::path::{Path, PathBuf};

use crate::domain::konpu::Diagnostic;

#[derive(Debug, Clone)]
pub struct AnalyzedDiagnostic {
    pub path: PathBuf,
    pub line: usize,
    pub diag: Diagnostic,
}

/// 設定なし（空の `konpu.toml` 相当）で解析する。
pub fn analyze_path(path: &Path) -> Vec<AnalyzedDiagnostic> {
    analyze_with_config(path, &template::ResolvedConfig::empty())
}

/// ベースラインと ignore を考慮してレポートに必要な情報を返す。
#[derive(Debug, Clone, Default)]
pub struct AnalysisResult {
    pub diagnostics: Vec<AnalyzedDiagnostic>,
    pub ignores: Vec<extract::IgnoreInfo>,
    pub declarations: Vec<extract::AnalyzedDeclaration>,
    pub impls: Vec<extract::ImplInfo>,
    pub law_tests: Vec<extract::LawTestInfo>,
    pub expectation_mismatches: Vec<template::LayerExpectationMismatch>,
    pub boundary_violations: Vec<template::BoundaryViolation>,
    /// コールグラフ provider が `Some` の場合、resolve_outgoing_calls が
    /// 何か返したかどうかのトレース用 (Phase 2 拡張用)。
    pub call_graph_resolutions: usize,
}

/// `konpu.toml` 由来の設定を適用して解析する。
pub fn analyze_with_config(
    path: &Path,
    config: &template::ResolvedConfig,
) -> Vec<AnalyzedDiagnostic> {
    analyze_full_with_cg(path, config, None).diagnostics
}

/// 診断以外の情報（ignore 抽出や declaration 収集）も返す統合 API。
pub fn analyze_full(path: &Path, config: &template::ResolvedConfig) -> AnalysisResult {
    analyze_full_with_cg(path, config, None)
}

/// コールグラフ provider を渡せる統合 API。
///
/// `provider` が `Some` の場合、preserve 検査で実際のコールエッジも参照する
/// (Phase 2 拡張)。`None` のときは従来の近似検査 (同名型 rank 降格) に
/// フォールバック。
pub fn analyze_full_with_cg(
    path: &Path,
    config: &template::ResolvedConfig,
    _provider: Option<&dyn call_graph::CallGraphProvider>,
) -> AnalysisResult {
    // Phase 0: provider 未使用。Phase 1 以降でここを resolve_outgoing_calls
    // 呼び出しに置き換える。
    let mut r = analyze_full_body(path, config);
    r.call_graph_resolutions = 0;
    r
}

/// 実際の解析本体。provider 非依存の従来ロジック。
fn analyze_full_body(path: &Path, config: &template::ResolvedConfig) -> AnalysisResult {
    let files = parser::collect_rust_files(path);
    let mut all_decls = Vec::new();
    let mut all_impls = Vec::new();
    let mut all_law_tests = Vec::new();
    let mut all_ignores = Vec::new();
    let mut all_type_infos = Vec::new();
    let mut all_uses = Vec::new();
    for file in &files {
        let Some((_, tree)) = parser::parse_file(file) else {
            continue;
        };
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let root = tree.root_node();
        all_decls.extend(extract::extract_declarations(root, &source, file));
        all_impls.extend(extract::extract_impls(root, &source));
        all_law_tests.extend(extract::extract_law_tests(root, &source, file));
        all_ignores.extend(extract::extract_ignores(root, &source, file));
        all_type_infos.extend(propagation::extract_type_infos(root, &source));
        all_uses.extend(extract::extract_use_statements(root, &source, file));
    }
    for decl in &mut all_decls {
        let (size, _count) = propagation::compute_propagation(&decl.type_name, &all_type_infos);
        decl.propagation = Some(size);
    }
    let mut out: Vec<AnalyzedDiagnostic> = Vec::new();
    for decl in &all_decls {
        for diag in check::check_declaration(decl, &all_impls) {
            out.push(AnalyzedDiagnostic {
                path: decl.path.clone(),
                line: decl.line,
                diag,
            });
        }
        for diag in check::check_propagation(decl, config) {
            out.push(AnalyzedDiagnostic {
                path: decl.path.clone(),
                line: decl.line,
                diag,
            });
        }
    }
    for (path, line, diag) in check::check_law_tests(&all_decls, &all_law_tests) {
        out.push(AnalyzedDiagnostic { path, line, diag });
    }
    let mut mismatches = Vec::new();
    for decl in &all_decls {
        let Some(layer) = template::match_layer(config, &decl.path) else {
            continue;
        };
        if !layer.expect_structures.is_empty()
            && !layer.expect_structures.contains(&decl.target_structure)
        {
            let expected: Vec<String> =
                layer.expect_structures.iter().map(|s| format!("{s:?}")).collect();
            mismatches.push(template::LayerExpectationMismatch {
                layer_name: layer.name.clone(),
                path: decl.path.clone(),
                line: decl.line,
                type_name: decl.type_name.clone(),
                reason: format!(
                    "expected one of [{}], got {:?}",
                    expected.join(", "),
                    decl.target_structure
                ),
            });
        }
        if let Some(hk) = &decl.higher_kinded {
            if !layer.expect_higher.is_empty() && !layer.expect_higher.contains(hk) {
                let expected: Vec<String> =
                    layer.expect_higher.iter().map(|s| format!("{s:?}")).collect();
                mismatches.push(template::LayerExpectationMismatch {
                    layer_name: layer.name.clone(),
                    path: decl.path.clone(),
                    line: decl.line,
                    type_name: decl.type_name.clone(),
                    reason: format!(
                        "expected higher-kinded one of [{}], got {hk:?}",
                        expected.join(", ")
                    ),
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
    let mut boundary_violations = Vec::new();
    for b in &config.boundaries {
        let from_key = from_pattern_key(&b.from_pattern);
        for u in &all_uses {
            let caller_path = u.path.clone();
            let to_match = glob_match_path(&b.to_pattern, &caller_path);
            if !to_match {
                continue;
            }
            if from_key.is_empty() {
                continue;
            }
            if imported_matches(&u.imported_path, &from_key) {
                boundary_violations.push(template::BoundaryViolation {
                    boundary_name: b.name.clone(),
                    from_path: caller_path.clone(),
                    to_path: caller_path.clone(),
                    line: u.line,
                    imported_path: u.imported_path.clone(),
                    reason: format!(
                        "{} (in `to` layer) imports `{from_key}` (in `from` layer); boundary `{}` permits `from` -> `to` only",
                        caller_path.display(),
                        b.name
                    ),
                });
            }
        }
        // `preserve` check (Phase 2-A minimum): for each AlgebraicStructure
        // declared in `from` layer with `target_structure` ∈ boundary.preserve,
        // check whether a same-named struct exists in `to` layer. If it does
        // and is NOT annotated with the same AlgebraicStructure, emit a
        // BoundaryPreserveWarning. If no same-named struct exists, that's
        // outside the scope of static-without-call-graph — we skip silently
        // (this minimum does not prove absence of preservation).
        if b.preserve.is_empty() {
            continue;
        }
        let from_decls: Vec<&extract::AnalyzedDeclaration> = all_decls
            .iter()
            .filter(|d| {
                glob_match_path(&b.from_pattern, &d.path)
                    && b.preserve.contains(&d.target_structure)
            })
            .collect();
        if from_decls.is_empty() {
            continue;
        }
        let to_decls: Vec<&extract::AnalyzedDeclaration> = all_decls
            .iter()
            .filter(|d| glob_match_path(&b.to_pattern, &d.path))
            .collect();
        for fd in &from_decls {
            for td in &to_decls {
                if fd.type_name == td.type_name
                    && fd.target_structure.rank() > td.target_structure.rank()
                {
                    boundary_violations.push(template::BoundaryViolation {
                        boundary_name: b.name.clone(),
                        from_path: fd.path.clone(),
                        to_path: td.path.clone(),
                        line: td.line,
                        imported_path: String::new(),
                        reason: format!(
                            "preserve violation: `from` layer declares `{}` as {:?} (rank {}); `to` layer has same-named type but only as {:?} (rank {})",
                            fd.type_name,
                            fd.target_structure,
                            fd.target_structure.rank(),
                            td.target_structure,
                            td.target_structure.rank()
                        ),
                    });
                }
            }
        }
    }
    AnalysisResult {
        diagnostics: out,
        ignores: all_ignores,
        declarations: all_decls,
        impls: all_impls,
        law_tests: all_law_tests,
        expectation_mismatches: mismatches,
        boundary_violations,
        call_graph_resolutions: 0,
    }
}

fn glob_match_path(pattern: &str, file_path: &Path) -> bool {
    let rel = file_path
        .strip_prefix(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .unwrap_or(file_path);
    let s = rel.to_string_lossy();
    template::glob_match(pattern, &s)
}

fn from_pattern_key(from_pattern: &str) -> String {
    from_pattern
        .split("**")
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string()
}

fn imported_matches(imported_path: &str, from_key: &str) -> bool {
    imported_path.contains(from_key)
        || imported_path.replace("::", "/").contains(from_key)
}