pub mod baseline;
pub mod call_graph;
pub mod check;
pub mod extract;
pub mod infer;
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

/// 抽出フェーズの成果物。
struct Extracted {
    decls: Vec<extract::AnalyzedDeclaration>,
    impls: Vec<extract::ImplInfo>,
    law_tests: Vec<extract::LawTestInfo>,
    ignores: Vec<extract::IgnoreInfo>,
    uses: Vec<extract::UseStatement>,
}

/// 実際の解析本体。provider 非依存の従来ロジック。フェーズに分割して各段を委譲する。
fn analyze_full_body(path: &Path, config: &template::ResolvedConfig) -> AnalysisResult {
    let files: Vec<PathBuf> = parser::collect_rust_files(path)
        .into_iter()
        .filter(|f| !config.is_excluded(f, path))
        .collect();
    // glob 照合の基準。ディレクトリ解析ならそのディレクトリ（外部プロジェクトを
    // 別 CWD から解析しても合う）、単一ファイル解析ならプロジェクトルート＝CWD 想定。
    let root: PathBuf = if path.is_dir() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };
    let ex = extract_all(&files, config.infer);
    let diagnostics = run_diagnostics(&ex, config, &root);
    let expectation_mismatches = layer_mismatches(&ex.decls, config, &root);
    let boundary_violations = boundary_checks(&ex, config, &root);
    AnalysisResult {
        diagnostics,
        ignores: ex.ignores,
        declarations: ex.decls,
        impls: ex.impls,
        law_tests: ex.law_tests,
        expectation_mismatches,
        boundary_violations,
        call_graph_resolutions: 0,
    }
}

/// フェーズ1: 全ファイルから宣言・impl・law test・ignore・use を抽出し、
/// 各宣言の伝播度を算出する。`infer` が真ならアノテーション無しの型も推論して足す。
fn extract_all(files: &[PathBuf], infer: bool) -> Extracted {
    let mut ex = Extracted {
        decls: Vec::new(),
        impls: Vec::new(),
        law_tests: Vec::new(),
        ignores: Vec::new(),
        uses: Vec::new(),
    };
    let mut type_infos = Vec::new();
    let mut type_sites: std::collections::HashMap<String, (PathBuf, usize)> =
        std::collections::HashMap::new();
    for file in files {
        let Some((_, tree)) = parser::parse_file(file) else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        let root = tree.root_node();
        ex.decls.extend(extract::extract_declarations(root, &source, file));
        ex.impls.extend(extract::extract_impls(root, &source));
        ex.law_tests.extend(extract::extract_law_tests(root, &source, file));
        ex.ignores.extend(extract::extract_ignores(root, &source, file));
        type_infos.extend(propagation::extract_type_infos(root, &source));
        ex.uses.extend(extract::extract_use_statements(root, &source, file));
        if infer {
            for (name, path, line) in extract::extract_type_sites(root, &source, file) {
                type_sites.entry(name).or_insert((path, line));
            }
        }
    }
    if infer {
        let annotated: std::collections::HashSet<String> =
            ex.decls.iter().map(|d| d.type_name.clone()).collect();
        ex.decls
            .extend(infer::infer_declarations(&ex.impls, &type_sites, &annotated));
    }
    for decl in &mut ex.decls {
        let (size, _count) = propagation::compute_propagation(&decl.type_name, &type_infos);
        decl.propagation = Some(size);
    }
    ex
}

/// フェーズ2: 宣言・伝播・law test の診断を集めてパス/行でソートする。
fn run_diagnostics(ex: &Extracted, config: &template::ResolvedConfig, root: &Path) -> Vec<AnalyzedDiagnostic> {
    let mut out: Vec<AnalyzedDiagnostic> = Vec::new();
    for decl in &ex.decls {
        for diag in check::check_declaration(decl, &ex.impls) {
            out.push(AnalyzedDiagnostic { path: decl.path.clone(), line: decl.line, diag });
        }
        for diag in check::check_propagation(decl, config, root) {
            out.push(AnalyzedDiagnostic { path: decl.path.clone(), line: decl.line, diag });
        }
    }
    for (path, line, diag) in check::check_law_tests(&ex.decls, &ex.law_tests) {
        out.push(AnalyzedDiagnostic { path, line, diag });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
    out
}

/// フェーズ3: 層の期待構造（`expect` / `expect_higher`）との不一致を集める。
fn layer_mismatches(
    decls: &[extract::AnalyzedDeclaration],
    config: &template::ResolvedConfig,
    root: &Path,
) -> Vec<template::LayerExpectationMismatch> {
    let mut mismatches = Vec::new();
    for decl in decls {
        let Some(layer) = template::match_layer(config, &decl.path, root) else {
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
    mismatches
}

/// フェーズ4: 層間境界を検査する（逆方向 import + preserve の名前ベース近似）。
/// コールグラフ版 preserve は `preserve_cg` モジュール（`call-graph` feature）。
fn boundary_checks(
    ex: &Extracted,
    config: &template::ResolvedConfig,
    root: &Path,
) -> Vec<template::BoundaryViolation> {
    let mut boundary_violations = Vec::new();
    for b in &config.boundaries {
        let from_key = from_pattern_key(&b.from_pattern);
        for u in &ex.uses {
            let caller_path = u.path.clone();
            if !glob_match_path(&b.to_pattern, &caller_path, root) || from_key.is_empty() {
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
        boundary_violations.extend(preserve_nominal(b, &ex.decls, root));
    }
    boundary_violations
}

/// preserve の名前ベース近似（Phase 2-A minimum）: `from` 層で preserve 対象構造として
/// 宣言された型 `T` と同名の型が `to` 層に低い rank で存在すれば違反。同名型が無い場合は
/// 静的テキストだけでは判定不能なので黙ってスキップ（コールグラフ版が `preserve_cg`）。
fn preserve_nominal(
    b: &template::ResolvedBoundary,
    decls: &[extract::AnalyzedDeclaration],
    root: &Path,
) -> Vec<template::BoundaryViolation> {
    if b.preserve.is_empty() {
        return Vec::new();
    }
    let from_decls: Vec<&extract::AnalyzedDeclaration> = decls
        .iter()
        .filter(|d| {
            glob_match_path(&b.from_pattern, &d.path, root)
                && b.preserve.contains(&d.target_structure)
        })
        .collect();
    if from_decls.is_empty() {
        return Vec::new();
    }
    let to_decls: Vec<&extract::AnalyzedDeclaration> = decls
        .iter()
        .filter(|d| glob_match_path(&b.to_pattern, &d.path, root))
        .collect();
    let mut out = Vec::new();
    for fd in &from_decls {
        for td in &to_decls {
            if fd.type_name == td.type_name
                && fd.target_structure.rank() > td.target_structure.rank()
            {
                out.push(template::BoundaryViolation {
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
    out
}

fn glob_match_path(pattern: &str, file_path: &Path, root: &Path) -> bool {
    template::glob_match(pattern, &template::rel_to_root(file_path, root))
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