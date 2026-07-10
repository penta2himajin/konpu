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

use super::parser::{self, Language};

/// Swift プロジェクトから Facts を構築する（外部ツール不要）。
pub fn facts_from_swift_project(path: &Path) -> Facts {
    let sources: Vec<_> = parser::collect_source_files(path)
        .into_iter()
        .filter(|(_, l)| *l == Language::Swift)
        .filter_map(|(f, _)| std::fs::read_to_string(&f).ok().map(|s| (f, s)))
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
