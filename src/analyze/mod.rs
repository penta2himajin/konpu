pub mod check;
pub mod extract;
pub mod parser;

use std::path::{Path, PathBuf};

use crate::domain::konpu::Diagnostic;

#[derive(Debug, Clone)]
pub struct AnalyzedDiagnostic {
    pub path: PathBuf,
    pub line: usize,
    pub diag: Diagnostic,
}

pub fn analyze_path(path: &Path) -> Vec<AnalyzedDiagnostic> {
    let files = parser::collect_rust_files(path);
    let mut all_decls = Vec::new();
    let mut all_impls = Vec::new();
    let mut all_law_tests = Vec::new();
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
    }
    for (path, line, diag) in check::check_law_tests(&all_decls, &all_law_tests) {
        out.push(AnalyzedDiagnostic { path, line, diag });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
    out
}