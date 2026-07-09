//! Konpu's analyze facade — re-exports the trait/struct from konpu-cg when the
//! `call-graph` feature is enabled, otherwise exposes a local shim so the
//! `analyze_full_with_cg` API exists in both build modes.

#[cfg(feature = "call-graph")]
pub use konpu_cg::{
    facts_from_project, facts_from_scip_file, CallGraph, CallGraphProvider, CallTarget, Facts,
    Precision,
};

#[cfg(not(feature = "call-graph"))]
pub use shim::{CallGraphProvider, CallTarget};

use std::collections::HashSet;
use std::path::Path;

use tree_sitter::Node;

use super::parser;

/// ソース中で「値位置で構築される型名」の集合を tree-sitter で抽出する。
///
/// RTA 精緻化 (docs/layer2-call-graph-design.md §6.1): SCIP は構築と型言及を
/// 区別できないので、代わりに Rust の構文から実際の構築サイトを拾う。捕捉するのは
/// - 構造体リテラル `Foo { .. }` / `Path::Foo { .. }`
/// - タプル構造体・関連関数呼び出し `Foo(..)` / `Foo::new(..)` (先頭大文字の型)
///
/// 型注釈・trait 境界・`impl Trait for T` ヘッダ・戻り値型は値位置ではないので
/// 拾わない。マクロ/serde/reflection 由来の構築は見えない (設計 §6 が認める穴)。
/// 型名は末尾セグメントのみで照合する (SCIP の `for_type` も同様に非修飾)。
pub fn constructed_types(path: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    for file in parser::collect_rust_files(path) {
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        collect_constructed(&source, &mut out);
    }
    out
}

/// 1 ソース分の構築型名を集める (ファイル I/O 非依存、テスト用)。
pub fn collect_constructed(source: &str, out: &mut HashSet<String>) {
    let Some(tree) = parser::parse_source(source) else {
        return;
    };
    walk_constructions(tree.root_node(), source, out);
}

fn text_of(node: Node, source: &str) -> Option<String> {
    node.utf8_text(source.as_bytes()).ok().map(str::to_string)
}

fn starts_upper(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_uppercase())
}

/// name ノード (type_identifier / scoped_type_identifier / generic_type) 内の
/// 最初の type_identifier をその型名とみなす。
fn first_type_ident(node: Node, source: &str) -> Option<String> {
    if node.kind() == "type_identifier" {
        return text_of(node, source);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(t) = first_type_ident(child, source) {
            return Some(t);
        }
    }
    None
}

/// call_expression の function ノードから、構築される型名候補を返す。
fn callee_type(f: Node, source: &str) -> Option<String> {
    match f.kind() {
        // タプル構造体 / タプル列挙子: `Foo(..)`
        "identifier" => {
            let t = text_of(f, source)?;
            starts_upper(&t).then_some(t)
        }
        // 関連関数 / 列挙子: `Foo::new(..)`, `a::Foo::from(..)` の `Foo`
        "scoped_identifier" => {
            let path = f.child_by_field_name("path")?;
            let name = match path.kind() {
                "identifier" => text_of(path, source)?,
                "scoped_identifier" => text_of(path.child_by_field_name("name")?, source)?,
                _ => return None,
            };
            starts_upper(&name).then_some(name)
        }
        // `Foo::<T>::new(..)`
        "generic_function" => callee_type(f.child_by_field_name("function")?, source),
        _ => None,
    }
}

fn walk_constructions(node: Node, source: &str, out: &mut HashSet<String>) {
    match node.kind() {
        "struct_expression" => {
            if let Some(name) = node.child_by_field_name("name") {
                if let Some(t) = first_type_ident(name, source) {
                    out.insert(t);
                }
            }
        }
        "call_expression" => {
            if let Some(f) = node.child_by_field_name("function") {
                if let Some(t) = callee_type(f, source) {
                    out.insert(t);
                }
            }
        }
        // 値位置の PascalCase 識別子 = 裸の unit 構造体 / 列挙子構築や
        // 関連関数レシーバ。tree-sitter は型位置を `type_identifier` にするので、
        // 型注釈・`impl Trait for T` ヘッダ・ジェネリクス引数・戻り値型は含まれない
        // （＝ SCIP の縮退要因を排除できる）。SCREAMING_CASE 定数は小文字を含まず除外。
        "identifier" => {
            if let Some(t) = text_of(node, source) {
                if starts_upper(&t) && t.chars().any(|c| c.is_lowercase()) {
                    out.insert(t);
                }
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_constructions(child, source, out);
    }
}

#[cfg(test)]
mod construction_tests {
    use super::*;

    fn constructed(src: &str) -> HashSet<String> {
        let mut out = HashSet::new();
        collect_constructed(src, &mut out);
        out
    }

    #[test]
    fn detects_real_constructions_not_type_mentions() {
        let src = r#"
            struct Circle { r: f64 }
            struct Square;
            struct Unit;
            fn build() -> Circle {
                let _c = Circle { r: 1.0 };   // struct literal
                let _s = Square::new();        // assoc-fn construction
                let _u = Unit;                 // bare unit-struct construction
                make(&_c)
            }
            fn make(_: &Circle) -> Only { todo!() }  // Only only in type position
            struct Only;
            struct Point(i32, i32);
            fn pt() -> Point { Point(1, 2) }         // tuple-struct construction
        "#;
        let got = constructed(src);
        assert!(got.contains("Circle")); // struct literal
        assert!(got.contains("Square")); // Square::new()
        assert!(got.contains("Unit")); // bare unit struct in value position
        assert!(got.contains("Point")); // Point(1, 2)
        // `Only` appears only as a return type (type position) — not a construction.
        assert!(!got.contains("Only"));
    }
}

#[cfg(not(feature = "call-graph"))]
mod shim {
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct CallTarget {
        pub target_path: PathBuf,
        pub target_line: usize,
        pub target_name: String,
    }

    pub trait CallGraphProvider {
        fn resolve_outgoing_calls(
            &self,
            _file_path: &Path,
            _line: usize,
            _column: usize,
        ) -> Vec<CallTarget> {
            Vec::new()
        }
    }
}
