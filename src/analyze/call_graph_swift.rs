//! Swift のコールグラフ事実抽出（tree-sitter-swift）。
//!
//! Rust は `rust-analyzer scip` で意味解析済みの事実を得るが、Swift 向けの
//! 安定した SCIP indexer を前提にできないので、konpu が既に持つ tree-sitter-swift
//! で `konpu_cg::Facts` を直接構築する。解釈エンジン（`konpu_cg::graph`）は言語
//! 中立なので無改変で再利用でき、循環/ハブ検出がそのまま効く。
//!
//! 名前解決は構文ベースの過大近似（layer2 §4「偽陽性許容・偽陰性なし」）:
//! - 全関数を `TraitMethod{trait:"", method:<bare 名>}` の impl として登録。
//!   メソッドは `for_type=<型>`、自由関数は `for_type=""`。
//! - 全呼び出しを `Dynamic{TraitMethod{"", callee}}` にする。CHA は同名関数すべて、
//!   RTA は構築された型（+ 常に残す自由関数 `""`）に絞る。
//! - `Type(...)` 構築サイトを `instantiated` に入れて RTA を効かせる。
//!
//! 精度の天井: 構文のみなので受け手の型は分からず、メソッド呼びは同名全てに繋ぐ
//! （over-approx）。`Money.zero()` のような型限定静的呼びの精密化は将来課題。

use std::collections::HashMap;
use std::path::Path;

use tree_sitter::Node;

use konpu_cg::{CallSite, CallTargetKind, Facts, FuncId, ImplEntry, TraitMethod};

use super::call_graph::{FnSig, MergeConstruction};
use super::parser::{self, Language};

/// Swift プロジェクトから Facts を構築する（外部ツール不要）。
/// パスはプロジェクトルート相対で格納する（preserve の `to`/`from` glob は
/// SCIP 同様に相対パス前提のため）。
pub fn facts_from_swift_project(path: &Path) -> Facts {
    let sources: Vec<_> = parser::collect_source_files(path)
        .into_iter()
        .filter(|(_, l)| *l == Language::Swift)
        .filter_map(|(f, _)| {
            let rel = f.strip_prefix(path).unwrap_or(&f).to_path_buf();
            std::fs::read_to_string(&f).ok().map(|s| (rel, s))
        })
        .collect();
    facts_from_swift_sources(sources)
}

/// (path, source) の集合から Facts を構築する（テスト・in-memory 用）。
pub fn facts_from_swift_sources(sources: Vec<(std::path::PathBuf, String)>) -> Facts {
    let parsed: Vec<(std::path::PathBuf, String, tree_sitter::Tree)> = sources
        .into_iter()
        .filter_map(|(f, src)| {
            let tree = parser::parse_with(&src, Language::Swift)?;
            Some((f, src, tree))
        })
        .collect();

    let mut facts = Facts::default();
    // 自由関数（for_type ""）は RTA でも常に残す。
    facts.instantiated.insert(String::new());

    // Pass 1: 関数定義を登録し、node.id() -> FuncId を記録。
    let mut fn_ids: Vec<HashMap<usize, FuncId>> = Vec::with_capacity(parsed.len());
    for (fpath, src, tree) in &parsed {
        let mut ids = HashMap::new();
        collect_funcs(tree.root_node(), src, fpath, None, &mut facts, &mut ids);
        fn_ids.push(ids);
    }

    // Pass 2: 各関数本体の呼び出しをエッジ化。
    for (fi, (_, src, tree)) in parsed.iter().enumerate() {
        collect_calls(tree.root_node(), src, &fn_ids[fi], None, &mut facts);
    }
    facts
}

/// Swift プロジェクトの全関数シグネチャ（preserve 検査 B/C 用）。
/// 集約シェイプ判定（`is_aggregation_shape`）は末尾セグメント + `[T]`/`<T>` 照合で
/// Swift 型文字列をそのまま扱えるので正規化不要。
pub fn fn_signatures_swift(path: &Path) -> Vec<FnSig> {
    let mut out = Vec::new();
    for (f, lang) in parser::collect_source_files(path) {
        if lang != Language::Swift {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&f) else { continue };
        let Some(tree) = parser::parse_with(&src, Language::Swift) else { continue };
        walk_fn_sigs(tree.root_node(), &src, &f, None, &mut out);
    }
    out
}

fn walk_fn_sigs(n: Node, source: &str, path: &Path, self_ty: Option<&str>, out: &mut Vec<FnSig>) {
    match n.kind() {
        "class_declaration" => {
            let ty = decl_keyword(n, source).and_then(|kw| decl_type_name(n, source, &kw));
            let body = first_child_of_kind(n, "class_body")
                .or_else(|| first_child_of_kind(n, "enum_class_body"));
            if let Some(body) = body {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    walk_fn_sigs(child, source, path, ty.as_deref(), out);
                }
            }
            return;
        }
        "function_declaration" => {
            if let Some(sig) = parse_fn_sig(n, source, path, self_ty) {
                out.push(sig);
            }
            return; // ネスト関数は稀。降りない。
        }
        _ => {}
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        walk_fn_sigs(child, source, path, self_ty, out);
    }
}

fn parse_fn_sig(n: Node, source: &str, path: &Path, self_ty: Option<&str>) -> Option<FnSig> {
    let name = func_name(n, source)?;
    let is_static = has_static(n, source);
    let mut params = Vec::new();
    let mut params_named = Vec::new();
    let mut ret = None;
    let mut cur = n.walk();
    for c in n.children(&mut cur) {
        match c.kind() {
            "parameter" => {
                if let Some(ty) = param_type(c, source) {
                    if let Some(id) = param_name(c, source) {
                        params_named.push((id, ty.clone()));
                    }
                    params.push(ty);
                }
            }
            "user_type" | "optional_type" | "tuple_type" | "array_type" | "dictionary_type" => {
                ret = Some(text_of(c, source).trim().to_string());
            }
            _ => {}
        }
    }
    let mut constructions = Vec::new();
    if let Some(body) = first_child_of_kind(n, "function_body") {
        collect_constructions(body, source, &mut constructions);
    }
    Some(FnSig {
        path: path.to_path_buf(),
        line: n.start_position().row + 1,
        // インスタンスメソッドは暗黙 self がその型。static/自由関数は self 無し。
        self_type: if is_static { None } else { self_ty.map(str::to_string) },
        name,
        params,
        params_named,
        ret,
        constructions,
    })
}

fn has_static(n: Node, source: &str) -> bool {
    first_child_of_kind(n, "modifiers").is_some_and(|m| {
        let mut cur = m.walk();
        m.children(&mut cur).any(|c| matches!(text_of(c, source).trim(), "static" | "class"))
    })
}

/// パラメータの型テキスト。
fn param_type(n: Node, source: &str) -> Option<String> {
    let mut cur = n.walk();
    n.children(&mut cur)
        .find(|c| matches!(c.kind(), "user_type" | "optional_type" | "tuple_type" | "array_type" | "dictionary_type"))
        .map(|c| text_of(c, source).trim().to_string())
}

/// パラメータの内部名（`_ other: T` → other、`items: T` → items）。最後の simple_identifier。
fn param_name(n: Node, source: &str) -> Option<String> {
    let mut cur = n.walk();
    n.children(&mut cur)
        .filter(|c| c.kind() == "simple_identifier")
        .last()
        .map(|c| text_of(c, source).trim().to_string())
}

/// 本体内の `Type(...)` 構築サイト（検出器 C）。refs = 引数式が参照する基底識別子。
fn collect_constructions(n: Node, source: &str, out: &mut Vec<MergeConstruction>) {
    if n.kind() == "call_expression" {
        if let Some(first) = {
            let mut cur = n.walk();
            n.children(&mut cur).find(|c| c.is_named())
        } {
            if first.kind() == "simple_identifier" {
                let ty = text_of(first, source).trim().to_string();
                if is_pascal_case(&ty) {
                    let mut refs = Vec::new();
                    if let Some(args) = first_child_of_kind(n, "call_suffix") {
                        collect_base_idents(args, source, &mut refs);
                    }
                    out.push(MergeConstruction { type_name: ty, line: n.start_position().row + 1, refs });
                }
            }
        }
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_constructions(child, source, out);
    }
}

/// 式木の基底識別子（`a.x` → "a"、`self`）を重複なく集める。引数ラベルと
/// navigation の末尾（.x）は除く。
fn collect_base_idents(n: Node, source: &str, out: &mut Vec<String>) {
    match n.kind() {
        // 引数ラベル `amount:` と navigation の末尾 `.amount` は参照ではない。降りない。
        "value_argument_label" | "navigation_suffix" => return,
        "self_expression" => {
            if !out.iter().any(|s| s == "self") {
                out.push("self".to_string());
            }
            return;
        }
        "simple_identifier" => {
            let t = text_of(n, source).trim().to_string();
            if !t.is_empty() && !out.contains(&t) {
                out.push(t);
            }
        }
        _ => {}
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_base_idents(child, source, out);
    }
}

fn text_of<'a>(n: Node, source: &'a str) -> &'a str {
    n.utf8_text(source.as_bytes()).unwrap_or("")
}

fn first_child_of_kind<'a>(n: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cur = n.walk();
    n.children(&mut cur).find(|c| c.kind() == kind)
}

fn is_pascal_case(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_uppercase())
}

/// `class_declaration` の種別トークン（struct/class/enum/extension）。
fn decl_keyword(n: Node, source: &str) -> Option<String> {
    let mut cur = n.walk();
    n.children(&mut cur)
        .find(|c| {
            !c.is_named() && matches!(text_of(*c, source).trim(), "struct" | "class" | "enum" | "extension")
        })
        .map(|c| text_of(c, source).trim().to_string())
}

fn decl_type_name(n: Node, source: &str, keyword: &str) -> Option<String> {
    if keyword == "extension" {
        let ut = first_child_of_kind(n, "user_type")?;
        first_child_of_kind(ut, "type_identifier").map(|t| text_of(t, source).to_string())
    } else {
        first_child_of_kind(n, "type_identifier").map(|t| text_of(t, source).to_string())
    }
}

/// 関数名（`func` トークンの次の子）。演算子は生のまま（call_expression に現れない）。
fn func_name(n: Node, source: &str) -> Option<String> {
    let mut cur = n.walk();
    let mut seen_func = false;
    for c in n.children(&mut cur) {
        if seen_func {
            return Some(text_of(c, source).trim().to_string());
        }
        if !c.is_named() && text_of(c, source).trim() == "func" {
            seen_func = true;
        }
    }
    None
}

fn collect_funcs(
    n: Node,
    source: &str,
    fpath: &Path,
    enclosing: Option<&str>,
    facts: &mut Facts,
    ids: &mut HashMap<usize, FuncId>,
) {
    match n.kind() {
        "class_declaration" => {
            let ty = decl_keyword(n, source).and_then(|kw| decl_type_name(n, source, &kw));
            let body = first_child_of_kind(n, "class_body")
                .or_else(|| first_child_of_kind(n, "enum_class_body"));
            if let Some(body) = body {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    collect_funcs(child, source, fpath, ty.as_deref(), facts, ids);
                }
            }
            return;
        }
        "function_declaration" => {
            if let Some(bare) = func_name(n, source) {
                let name = match enclosing {
                    Some(t) => format!("{t}.{bare}"),
                    None => bare.clone(),
                };
                let id = facts.add_func(name, fpath.to_path_buf(), n.start_position().row + 1);
                ids.insert(n.id(), id);
                facts.impls.push(ImplEntry {
                    trait_method: TraitMethod::new("", bare),
                    for_type: enclosing.unwrap_or("").to_string(),
                    func: id,
                });
            }
            // ネストした関数も拾うため本体を降りる。
        }
        _ => {}
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_funcs(child, source, fpath, enclosing, facts, ids);
    }
}

fn collect_calls(
    n: Node,
    source: &str,
    ids: &HashMap<usize, FuncId>,
    caller: Option<FuncId>,
    facts: &mut Facts,
) {
    // この関数に入ったら caller を切り替える。
    let caller = if n.kind() == "function_declaration" {
        ids.get(&n.id()).copied().or(caller)
    } else {
        caller
    };

    if n.kind() == "call_expression" {
        if let Some(callee) = callee_of(n, source) {
            match callee {
                Callee::Construct(ty) => {
                    facts.instantiated.insert(ty);
                }
                Callee::Name(name) => {
                    if let Some(c) = caller {
                        facts.calls.push(CallSite {
                            caller: c,
                            target: CallTargetKind::Dynamic(TraitMethod::new("", name)),
                        });
                    }
                }
            }
        }
    }

    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_calls(child, source, ids, caller, facts);
    }
}

enum Callee {
    /// 通常の呼び出し（自由関数 or メソッド）。bare 名。
    Name(String),
    /// `Type(...)` 構築サイト。
    Construct(String),
}

/// call_expression の被呼び出しを判定。
fn callee_of(call: Node, source: &str) -> Option<Callee> {
    let mut cur = call.walk();
    let first = call.children(&mut cur).find(|c| c.is_named())?;
    match first.kind() {
        "simple_identifier" => {
            let name = text_of(first, source).trim().to_string();
            if is_pascal_case(&name) {
                Some(Callee::Construct(name)) // `Money(...)`
            } else {
                Some(Callee::Name(name)) // `foo(...)`
            }
        }
        "navigation_expression" => {
            // `recv.method` の末尾セグメントがメソッド名。
            let suffix = last_child_of_kind(first, "navigation_suffix")?;
            let m = first_child_of_kind(suffix, "simple_identifier")?;
            Some(Callee::Name(text_of(m, source).trim().to_string()))
        }
        _ => None,
    }
}

fn last_child_of_kind<'a>(n: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cur = n.walk();
    n.children(&mut cur).filter(|c| c.kind() == kind).last()
}

#[cfg(test)]
mod tests {
    use super::*;
    use konpu_cg::{CallGraph, Precision};
    use std::path::PathBuf;

    fn facts_of(files: &[(&str, &str)]) -> Facts {
        let sources = files
            .iter()
            .map(|(p, s)| (PathBuf::from(p), s.to_string()))
            .collect();
        facts_from_swift_sources(sources)
    }

    #[test]
    fn functions_and_qualified_names() {
        let f = facts_of(&[("A.swift", "struct A {\n  func run() {}\n  static func make() -> A { A() }\n}\nfunc free() {}")]);
        let names: Vec<&str> = f.funcs.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"A.run"));
        assert!(names.contains(&"A.make"));
        assert!(names.contains(&"free"));
    }

    #[test]
    fn construction_populates_instantiated() {
        let f = facts_of(&[("M.swift", "struct M { func go() { let _ = Money(amount: 0) } }")]);
        assert!(f.instantiated.contains("Money"));
    }

    #[test]
    fn cha_connects_method_call_across_files() {
        // A.ping calls B().pong(); B.pong calls A().ping() -> a 2-cycle.
        let f = facts_of(&[
            ("A.swift", "struct A { func ping() { B().pong() } }"),
            ("B.swift", "struct B { func pong() { A().ping() } }"),
        ]);
        let g = CallGraph::build(&f, Precision::Cha);
        assert_eq!(g.cycles().iter().filter(|c| c.len() == 2).count(), 1);
    }

    fn sigs_of(files: &[(&str, &str)]) -> Vec<FnSig> {
        // write to a temp dir so fn_signatures_swift (path-based) can read them.
        let dir = std::env::temp_dir().join(format!("konpu_cgsw_sig_{}", files.len()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, src) in files {
            std::fs::write(dir.join(name), src).unwrap();
        }
        let out = fn_signatures_swift(&dir);
        for (name, _) in files {
            std::fs::remove_file(dir.join(name)).ok();
        }
        std::fs::remove_dir(&dir).ok();
        out
    }

    #[test]
    fn fn_sig_aggregation_and_construction() {
        use crate::analyze::call_graph::is_aggregation_shape;
        let sigs = sigs_of(&[(
            "R.swift",
            "struct R {\n  func total(_ items: [Money]) -> Money { Money(amount: 0) }\n  func describe(_ a: Money, _ b: Money) -> String { let m = Money(amount: a.amount + b.amount); return \"x\" }\n}",
        )]);
        let total = sigs.iter().find(|s| s.name == "total").unwrap();
        assert!(is_aggregation_shape(total, "Money")); // [Money] -> Money
        let describe = sigs.iter().find(|s| s.name == "describe").unwrap();
        // C: constructs Money referencing two Money params a, b.
        let c = describe.constructions.iter().find(|c| c.type_name == "Money").unwrap();
        assert!(c.refs.contains(&"a".to_string()));
        assert!(c.refs.contains(&"b".to_string()));
        assert_eq!(describe.self_type, Some("R".to_string()));
    }

    #[test]
    fn rta_prunes_calls_into_never_constructed_type() {
        // Orchestrator is never constructed; under RTA its method targets vanish.
        let f = facts_of(&[(
            "O.swift",
            "struct O {\n  func run() { stepA(); stepB() }\n  func stepA() {}\n  func stepB() {}\n}",
        )]);
        let cha = CallGraph::build(&f, Precision::Cha);
        let rta = CallGraph::build(&f, Precision::Rta);
        let cha_edges: usize = cha.edges.iter().map(|s| s.len()).sum();
        let rta_edges: usize = rta.edges.iter().map(|s| s.len()).sum();
        assert!(cha_edges >= 2);
        assert_eq!(rta_edges, 0); // O never instantiated -> no kept method targets
    }
}
