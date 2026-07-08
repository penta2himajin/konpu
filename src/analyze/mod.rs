pub mod baseline;
pub mod check;
pub mod extract;
pub mod parser;
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
}

/// `konpu.toml` 由来の設定を適用して解析する。
pub fn analyze_with_config(
    path: &Path,
    config: &template::ResolvedConfig,
) -> Vec<AnalyzedDiagnostic> {
    analyze_full(path, config).diagnostics
}

/// 診断以外の情報（ignore 抽出や declaration 収集）も返す統合 API。
pub fn analyze_full(path: &Path, config: &template::ResolvedConfig) -> AnalysisResult {
    let files = parser::collect_rust_files(path);
    let mut all_decls = Vec::new();
    let mut all_impls = Vec::new();
    let mut all_law_tests = Vec::new();
    let mut all_ignores = Vec::new();
    let mut all_type_infos = Vec::new();
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
    AnalysisResult {
        diagnostics: out,
        ignores: all_ignores,
        declarations: all_decls,
        impls: all_impls,
        law_tests: all_law_tests,
        expectation_mismatches: mismatches,
    }
}