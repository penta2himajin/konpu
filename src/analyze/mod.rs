pub mod check;
pub mod extract;
pub mod parser;
pub mod propagation;
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

/// `konpu.toml` 由来の設定を適用して解析する。
pub fn analyze_with_config(
    path: &Path,
    config: &template::ResolvedConfig,
) -> Vec<AnalyzedDiagnostic> {
    let files = parser::collect_rust_files(path);
    let mut all_decls = Vec::new();
    let mut all_impls = Vec::new();
    let mut all_law_tests = Vec::new();
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
    out.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
    out
}