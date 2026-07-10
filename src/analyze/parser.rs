use std::path::{Path, PathBuf};

use tree_sitter::{Parser, Tree};

/// 解析対象の言語。抽出器（`extract` / `extract_swift`）の切り替えに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Language {
    Rust,
    Swift,
    Kotlin,
    Ts,
}

impl Language {
    /// 拡張子から言語を判定。未対応拡張子は `None`。
    pub fn from_path(path: &Path) -> Option<Language> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("rs") => Some(Language::Rust),
            Some("swift") => Some(Language::Swift),
            Some("kt") | Some("kts") => Some(Language::Kotlin),
            Some("ts") | Some("tsx") | Some("mts") | Some("cts") => Some(Language::Ts),
            _ => None,
        }
    }

    fn ts_language(self) -> tree_sitter::Language {
        match self {
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            Language::Swift => tree_sitter_swift::LANGUAGE.into(),
            Language::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
            Language::Ts => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }
}

/// 解析対象のソースファイルを言語付きで集める（`.rs` と `.swift`）。
pub fn collect_source_files(path: &Path) -> Vec<(PathBuf, Language)> {
    if path.is_file() {
        return Language::from_path(path)
            .map(|l| vec![(path.to_path_buf(), l)])
            .unwrap_or_default();
    }
    if !path.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    walk(path, &mut out);
    out.sort();
    out
}

/// Rust ファイルのみ集める（call graph / scaffold は Rust 専用）。
pub fn collect_rust_files(path: &Path) -> Vec<PathBuf> {
    collect_source_files(path)
        .into_iter()
        .filter(|(_, l)| *l == Language::Rust)
        .map(|(p, _)| p)
        .collect()
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, Language)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name.starts_with('.') {
            continue;
        }
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out);
        } else if let Some(lang) = Language::from_path(&p) {
            out.push((p, lang));
        }
    }
}

fn make_parser(lang: Language) -> Option<Parser> {
    let mut parser = Parser::new();
    parser.set_language(&lang.ts_language()).ok()?;
    Some(parser)
}

/// 指定言語でソースをパースする。
pub fn parse_with(source: &str, lang: Language) -> Option<Tree> {
    make_parser(lang)?.parse(source, None)
}

/// Rust ソースをパースする（call graph 用の後方互換）。
pub fn parse_source(source: &str) -> Option<Tree> {
    parse_with(source, Language::Rust)
}

/// Rust ファイルをパースする（scaffold 用の後方互換）。
pub fn parse_file(path: &Path) -> Option<(PathBuf, Tree)> {
    let source = std::fs::read_to_string(path).ok()?;
    let tree = parse_with(&source, Language::from_path(path)?)?;
    Some((path.to_path_buf(), tree))
}
