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
    let mut out: Vec<AnalyzedDiagnostic> = Vec::new();
    for file in files {
        let Some((_path, tree)) = parser::parse_file(&file) else {
            continue;
        };
        let source = match std::fs::read_to_string(&file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let root = tree.root_node();
        let decls = extract::extract_declarations(root, &source, &file);
        let impls = extract::extract_impls(root, &source);
        for decl in &decls {
            for diag in check::check_declaration(decl, &impls) {
                out.push(AnalyzedDiagnostic {
                    path: file.clone(),
                    line: decl.line,
                    diag,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.line.cmp(&b.line))
    });
    out
}