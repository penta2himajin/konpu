use std::path::{Path, PathBuf};

use tree_sitter::{Parser, Tree};

pub fn collect_rust_files(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            vec![path.to_path_buf()]
        } else {
            Vec::new()
        };
    }
    if !path.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    walk(path, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
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
        } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

pub fn make_parser() -> Option<Parser> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .ok()?;
    Some(parser)
}

pub fn parse_source(source: &str) -> Option<Tree> {
    let mut parser = make_parser()?;
    parser.parse(source, None)
}

pub fn parse_file(path: &Path) -> Option<(PathBuf, Tree)> {
    let source = std::fs::read_to_string(path).ok()?;
    let tree = parse_source(&source)?;
    Some((path.to_path_buf(), tree))
}