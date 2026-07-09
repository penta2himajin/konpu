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

// ---- 関数シグネチャ抽出（preserve 検査 B: 集約シェイプ判定用）----

use std::path::PathBuf;

/// 1 関数のシグネチャ。SCIP の FuncId と `path`+`name`(+行) で結合する。
#[derive(Debug, Clone)]
pub struct FnSig {
    pub path: PathBuf,
    pub line: usize, // 1-based, function_item 開始行
    pub name: String,
    /// receiver を持つメソッドなら、その impl 対象型（`Self` 解決用）。
    pub self_type: Option<String>,
    /// 明示引数の型文字列（self を除く。生ソースのまま: "Money", "&[Money]" 等）。
    pub params: Vec<String>,
    pub ret: Option<String>,
}

/// path 配下の全関数シグネチャを収集する。
pub fn fn_signatures(path: &Path) -> Vec<FnSig> {
    let mut out = Vec::new();
    for file in parser::collect_rust_files(path) {
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        collect_fn_sigs(&source, &file, &mut out);
    }
    out
}

/// 1 ソース分の関数シグネチャを集める（テスト用）。
pub fn collect_fn_sigs(source: &str, path: &Path, out: &mut Vec<FnSig>) {
    let Some(tree) = parser::parse_source(source) else {
        return;
    };
    walk_fn_sigs(tree.root_node(), source, path, None, out);
}

fn impl_type_of(node: Node, source: &str) -> Option<String> {
    let type_node = node.child_by_field_name("type")?;
    let raw = text_of(type_node, source)?;
    let raw = raw.trim();
    // ジェネリクス/空白の手前を型名とみなす（extract::impl_type_name と同じ扱い）。
    let head = raw
        .split(|c: char| c == '<' || c.is_whitespace())
        .next()
        .unwrap_or(raw);
    (!head.is_empty()).then(|| head.to_string())
}

fn walk_fn_sigs(node: Node, source: &str, path: &Path, self_ty: Option<&str>, out: &mut Vec<FnSig>) {
    match node.kind() {
        "impl_item" => {
            let ty = impl_type_of(node, source);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_fn_sigs(child, source, path, ty.as_deref(), out);
            }
            return;
        }
        "function_item" => {
            if let Some(sig) = parse_fn_sig(node, source, path, self_ty) {
                out.push(sig);
            }
            // 本体内のネスト関数は self を持たない。
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_fn_sigs(child, source, path, None, out);
            }
            return;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_fn_sigs(child, source, path, self_ty, out);
    }
}

fn parse_fn_sig(node: Node, source: &str, path: &Path, impl_ty: Option<&str>) -> Option<FnSig> {
    let name = text_of(node.child_by_field_name("name")?, source)?;
    let mut has_self = false;
    let mut params = Vec::new();
    if let Some(pn) = node.child_by_field_name("parameters") {
        let mut cursor = pn.walk();
        for param in pn.children(&mut cursor) {
            match param.kind() {
                "self_parameter" => has_self = true,
                "parameter" => {
                    if let Some(t) = param.child_by_field_name("type") {
                        if let Some(txt) = text_of(t, source) {
                            params.push(txt.trim().to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let ret = node.child_by_field_name("return_type").map(|n| {
        text_of(n, source)
            .unwrap_or_default()
            .trim()
            .trim_start_matches("->")
            .trim()
            .to_string()
    });
    Some(FnSig {
        path: path.to_path_buf(),
        line: node.start_position().row + 1,
        name,
        self_type: if has_self { impl_ty.map(str::to_string) } else { None },
        params,
        ret,
    })
}

fn strip_refs(s: &str) -> &str {
    let mut s = s.trim();
    loop {
        if let Some(r) = s.strip_prefix('&') {
            s = r.trim_start();
        } else if let Some(r) = s.strip_prefix("mut ") {
            s = r.trim_start();
        } else {
            break;
        }
    }
    s
}

fn last_seg(s: &str) -> &str {
    s.rsplit("::").next().unwrap_or(s)
}

/// 型文字列 `param` が（参照を剥がして）ちょうど型 `ty` か。コレクション/ジェネリクスは除く。
fn is_exactly(param: &str, ty: &str, self_ty: Option<&str>) -> bool {
    let s = strip_refs(param);
    if s == "Self" {
        return self_ty == Some(ty);
    }
    if s.contains(['<', '[', '(', ',', ' ']) {
        return false;
    }
    last_seg(s) == ty
}

/// 型 `ty` が `s` 内に 1 セグメントとして現れるか（`Vec<Money>` の Money 等）。
fn mentions_type(s: &str, ty: &str) -> bool {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
        .filter(|t| !t.is_empty())
        .any(|t| last_seg(t) == ty)
}

/// `param` が `ty` のコレクション（`&[Money]`, `Vec<Money>`, `impl Iterator<Item=Money>` 等）か。
fn is_collection_of(param: &str, ty: &str) -> bool {
    let s = strip_refs(param);
    (s.contains('[') || s.contains('<')) && mentions_type(s, ty)
}

/// `sig` が「複数の `ty` を 1 個の `ty` に併合する」集約シェイプか（検出器 B）。
///
/// 条件: 戻り値が単一の `ty`（`Self` 含む）、かつ入力が
/// (i) `ty` のコレクション、または (ii) `ty` 型の入力（self + 引数）が 2 個以上。
// ponytail: シグネチャの型文字列を末尾セグメントで照合する近似。別モジュール同名型は
// 衝突しうる。将来 SCIP の型シンボルで厳密化できる。カスタムコレクション型
// (自作の `NonEmpty<T>` 等) は `<T>`/`[T]` を含めば拾えるが、型引数を持たない
// ラッパは見逃す。改善経路: データフロー / 型解決。
pub fn is_aggregation_shape(sig: &FnSig, ty: &str) -> bool {
    let st = sig.self_type.as_deref();
    let returns_ty = sig.ret.as_deref().is_some_and(|r| is_exactly(r, ty, st));
    if !returns_ty {
        return false;
    }
    let self_is_ty = sig.self_type.as_deref() == Some(ty);
    let mut ty_inputs = usize::from(self_is_ty);
    let mut has_collection = false;
    for p in &sig.params {
        if is_exactly(p, ty, st) {
            ty_inputs += 1;
        } else if is_collection_of(p, ty) {
            has_collection = true;
        }
    }
    has_collection || ty_inputs >= 2
}

#[cfg(test)]
mod fnsig_tests {
    use super::*;

    fn sigs(src: &str) -> Vec<FnSig> {
        let mut out = Vec::new();
        collect_fn_sigs(src, Path::new("t.rs"), &mut out);
        out
    }

    fn named<'a>(sigs: &'a [FnSig], name: &str) -> &'a FnSig {
        sigs.iter().find(|s| s.name == name).unwrap()
    }

    #[test]
    fn aggregation_shapes_detected() {
        let src = r#"
            struct Money;
            impl Money {
                fn combine(self, other: Self) -> Self { self }   // self + arg -> Self: aggregate
                fn amount(&self) -> u64 { 0 }                      // not aggregate
            }
            fn sum_all(items: &[Money]) -> Money { todo!() }       // collection -> Money: aggregate
            fn merge_two(a: Money, b: Money) -> Money { a }        // 2 params -> Money: aggregate
            fn make(x: u64) -> Money { todo!() }                   // singleton ctor: not aggregate
            fn describe(m: &Money) -> String { todo!() }           // not returning Money
        "#;
        let s = sigs(src);
        assert!(is_aggregation_shape(named(&s, "combine"), "Money"));
        assert!(is_aggregation_shape(named(&s, "sum_all"), "Money"));
        assert!(is_aggregation_shape(named(&s, "merge_two"), "Money"));
        assert!(!is_aggregation_shape(named(&s, "amount"), "Money"));
        assert!(!is_aggregation_shape(named(&s, "make"), "Money"));
        assert!(!is_aggregation_shape(named(&s, "describe"), "Money"));
    }

    #[test]
    fn self_type_resolved_for_methods() {
        let src = r#"
            struct Money;
            impl Money { fn combine(self, o: Self) -> Self { self } }
        "#;
        let s = sigs(src);
        let c = named(&s, "combine");
        assert_eq!(c.self_type.as_deref(), Some("Money"));
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
