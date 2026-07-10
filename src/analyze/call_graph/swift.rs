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

use konpu_cg::{Facts, FuncId};

use super::engine::{
    self, base_type_name, first_child_of_kind, is_pascal_case, last_child_of_kind, text_of, Index,
    Resolution, Resolver,
};
use super::{FnSig, MergeConstruction};
use crate::analyze::parser::Language;
use crate::analyze::template::ResolvedConfig;

/// collect_base_idents のノード種別（Swift）。
const IDENT_KINDS: engine::IdentKinds = engine::IdentKinds {
    // 引数ラベル `amount:` と navigation の末尾 `.amount` は参照ではない。
    skip: &["value_argument_label", "navigation_suffix"],
    self_kind: "self_expression",
    ident: "simple_identifier",
};

/// Swift プロジェクトから Facts を構築する（外部ツール不要）。
pub fn facts_from_swift_project(path: &Path, config: &ResolvedConfig) -> Facts {
    facts_from_swift_sources(engine::project_sources(path, config, Language::Swift))
}

/// (path, source) の集合から Facts を構築する（テスト・in-memory 用）。
pub fn facts_from_swift_sources(sources: Vec<(std::path::PathBuf, String)>) -> Facts {
    engine::facts_from_sources(
        Language::Swift,
        sources,
        |root, src, fpath, facts, ids, index| collect_funcs(root, src, fpath, None, facts, ids, index),
        |root, src, _, r, facts| collect_calls(root, src, r, None, None, facts),
    )
}

/// Swift プロジェクトの全関数シグネチャ（preserve 検査 B/C 用）。
/// 集約シェイプ判定（`is_aggregation_shape`）は末尾セグメント + `[T]`/`<T>` 照合で
/// Swift 型文字列をそのまま扱えるので正規化不要。
pub fn fn_signatures_swift(path: &Path, config: &ResolvedConfig) -> Vec<FnSig> {
    engine::fn_signatures(path, config, Language::Swift, |root, src, f, out| {
        walk_fn_sigs(root, src, f, None, out)
    })
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
                        engine::collect_base_idents(&IDENT_KINDS, args, source, &mut refs);
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
    index: &mut Index,
) {
    match n.kind() {
        "class_declaration" => {
            let ty = decl_keyword(n, source).and_then(|kw| decl_type_name(n, source, &kw));
            let body = first_child_of_kind(n, "class_body")
                .or_else(|| first_child_of_kind(n, "enum_class_body"));
            if let Some(body) = body {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    // ストアドプロパティの型を索引（インスタンス・型注釈付きのみ）。
                    if child.kind() == "property_declaration" && !has_static(child, source) {
                        if let (Some(name), Some(t)) = (prop_name(child, source), prop_type(child, source)) {
                            if let Some(ty) = &ty {
                                index.fields.entry(ty.clone()).or_default().insert(name, base_type_name(&t));
                            }
                        }
                    }
                    collect_funcs(child, source, fpath, ty.as_deref(), facts, ids, index);
                }
            }
            return;
        }
        "function_declaration" => {
            if let Some(bare) = func_name(n, source) {
                engine::register_func(&bare, n, fpath, enclosing, facts, ids, index);
            }
        }
        // init/deinit/subscript も本体を持つ呼び出し元。bare 名で登録して caller を与える。
        "init_declaration" => engine::register_func("init", n, fpath, enclosing, facts, ids, index),
        "deinit_declaration" => engine::register_func("deinit", n, fpath, enclosing, facts, ids, index),
        "subscript_declaration" => engine::register_func("subscript", n, fpath, enclosing, facts, ids, index),
        // computed property（`var x: T { ... }`）も本体を持つ。プロパティ名で登録。
        "property_declaration" if first_child_of_kind(n, "computed_property").is_some() => {
            if let Some(name) = prop_name(n, source) {
                engine::register_func(&name, n, fpath, enclosing, facts, ids, index);
            }
        }
        _ => {}
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_funcs(child, source, fpath, enclosing, facts, ids, index);
    }
}

fn collect_calls(
    n: Node,
    source: &str,
    r: &Resolver,
    enclosing: Option<&str>,
    caller: Option<FuncId>,
    facts: &mut Facts,
) {
    // 型に入ったら enclosing を更新（extension も対象型に解決）。
    if n.kind() == "class_declaration" {
        let ty = decl_keyword(n, source).and_then(|kw| decl_type_name(n, source, &kw));
        let body = first_child_of_kind(n, "class_body")
            .or_else(|| first_child_of_kind(n, "enum_class_body"));
        if let Some(body) = body {
            let mut cur = body.walk();
            for child in body.children(&mut cur) {
                collect_calls(child, source, r, ty.as_deref(), caller, facts);
            }
        }
        return;
    }
    // 関数に入ったら caller とローカル変数型を確定して本体を処理。
    // init/deinit/subscript も同じ扱い（Pass 1 で登録済み。本体は function_body か
    // computed_property）。
    if matches!(
        n.kind(),
        "function_declaration" | "init_declaration" | "deinit_declaration" | "subscript_declaration"
    ) {
        let c = r.ids.get(&n.id()).copied().or(caller);
        let mut locals = build_locals(n, source);
        let body = first_child_of_kind(n, "function_body")
            .or_else(|| first_child_of_kind(n, "computed_property"));
        if let Some(body) = body {
            collect_locals(body, source, &mut locals);
            resolve_body(body, source, r, enclosing, &locals, c, facts);
        }
        return;
    }
    // プロパティ宣言: computed body は登録済み caller で、ストアド初期化式は
    // caller 無しで walk する（構築 `= Money(...)` を instantiated に拾うため。
    // resolve_call は caller 無しでも構築だけは記録する）。
    if n.kind() == "property_declaration" {
        let c = r.ids.get(&n.id()).copied();
        let mut locals = HashMap::new();
        if let Some(body) = first_child_of_kind(n, "computed_property") {
            collect_locals(body, source, &mut locals);
            resolve_body(body, source, r, enclosing, &locals, c, facts);
        } else {
            resolve_body(n, source, r, enclosing, &locals, None, facts);
        }
        return;
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_calls(child, source, r, enclosing, caller, facts);
    }
}

/// 関数本体（クロージャ含む）の呼び出しを解決してエッジ化。ネストした named 関数は
/// それ自身の caller で再入する。
fn resolve_body(
    n: Node,
    source: &str,
    r: &Resolver,
    enclosing: Option<&str>,
    locals: &HashMap<String, String>,
    caller: Option<FuncId>,
    facts: &mut Facts,
) {
    if n.kind() == "function_declaration" {
        collect_calls(n, source, r, enclosing, caller, facts);
        return;
    }
    if n.kind() == "call_expression" {
        resolve_call(n, source, r.index, enclosing, locals, caller, facts);
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        resolve_body(child, source, r, enclosing, locals, caller, facts);
    }
}

/// 関数のローカル変数の型（引数 + 本体の `let/var x: T` / `let x = T(...)`）。
fn build_locals(fn_node: Node, source: &str) -> HashMap<String, String> {
    let mut locals = HashMap::new();
    // 引数。
    let mut cur = fn_node.walk();
    for c in fn_node.children(&mut cur) {
        if c.kind() == "parameter" {
            if let (Some(name), Some(t)) = (param_name(c, source), param_type(c, source)) {
                locals.insert(name, base_type_name(&t));
            }
        }
    }
    // 本体のローカル宣言。
    if let Some(body) = first_child_of_kind(fn_node, "function_body") {
        collect_locals(body, source, &mut locals);
    }
    locals
}

fn collect_locals(n: Node, source: &str, out: &mut HashMap<String, String>) {
    if n.kind() == "property_declaration" {
        if let Some(name) = prop_name(n, source) {
            // ponytail: 型は注釈 `let x: T` か構築 `let x = T(...)` からのみ拾う。
            // `let x = foo()`（call 戻り値）や chain `a.b()` はローカルが無型のまま残り、
            // その受け手 `x.m()` は resolve_call で Dynamic に落ちる（残差の主因）。
            // 精密化するなら Index に (型,メソッド)→戻り型 を持たせ戻り型伝播する。
            if let Some(t) = prop_type(n, source) {
                out.insert(name, base_type_name(&t));
            }
        }
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_locals(child, source, out);
    }
}

/// call_expression を解決してエッジ/構築サイトを facts に足す。
fn resolve_call(
    call: Node,
    source: &str,
    index: &Index,
    enclosing: Option<&str>,
    locals: &HashMap<String, String>,
    caller: Option<FuncId>,
    facts: &mut Facts,
) {
    let mut cur = call.walk();
    let Some(first) = call.children(&mut cur).find(|c| c.is_named()) else { return };

    let recv_type = |base: &str| engine::recv_type(base, enclosing, locals, index);

    let resolved: Resolution = match first.kind() {
        "simple_identifier" => {
            let name = text_of(first, source).trim().to_string();
            if is_pascal_case(&name) {
                facts.instantiated.insert(name); // `Money(...)` 構築
                return;
            }
            // 値（ローカル/フィールド）なら callAsFunction、そうでなければ関数/メソッド呼び。
            if let Some(t) = recv_type(&name) {
                engine::lookup_method(index, &t, "callAsFunction")
            } else {
                engine::resolve_bare(index, enclosing, &name)
            }
        }
        "navigation_expression" => {
            let Some(method) = nav_method(first, source) else { return };
            match nav_base_ident(first, source).and_then(|b| recv_type(&b)) {
                Some(t) => engine::lookup_method(index, &t, &method),
                None => Resolution::Dynamic(method),
            }
        }
        "postfix_expression" => {
            // `recv!(...)` / `recv?(...)` = callAsFunction on recv。
            let Some(base) = first_child_of_kind(first, "simple_identifier").map(|c| text_of(c, source).trim().to_string()) else {
                return;
            };
            match recv_type(&base) {
                Some(t) => engine::lookup_method(index, &t, "callAsFunction"),
                None => Resolution::Dynamic("callAsFunction".to_string()),
            }
        }
        _ => return,
    };

    engine::push_resolution(resolved, caller, facts);
}

/// navigation_expression の受け手基底識別子（self / 変数 / `x!`）。チェーンや式は None。
fn nav_base_ident(nav: Node, source: &str) -> Option<String> {
    let mut cur = nav.walk();
    let base = nav.children(&mut cur).find(|c| c.is_named())?;
    match base.kind() {
        "self_expression" => Some("self".to_string()),
        "simple_identifier" => Some(text_of(base, source).trim().to_string()),
        "postfix_expression" => {
            first_child_of_kind(base, "simple_identifier").map(|c| text_of(c, source).trim().to_string())
        }
        _ => None,
    }
}

fn nav_method(nav: Node, source: &str) -> Option<String> {
    let suffix = last_child_of_kind(nav, "navigation_suffix")?;
    first_child_of_kind(suffix, "simple_identifier").map(|c| text_of(c, source).trim().to_string())
}

/// property_declaration の名前（pattern > simple_identifier）。
fn prop_name(n: Node, source: &str) -> Option<String> {
    let pat = first_child_of_kind(n, "pattern")?;
    first_child_of_kind(pat, "simple_identifier").map(|c| text_of(c, source).trim().to_string())
}

/// property_declaration の型（type_annotation の型、無ければ初期化子の構築型）。
fn prop_type(n: Node, source: &str) -> Option<String> {
    if let Some(ta) = first_child_of_kind(n, "type_annotation") {
        let mut cur = ta.walk();
        if let Some(t) = ta.children(&mut cur).find(|c| c.is_named()) {
            return Some(text_of(t, source).trim().to_string());
        }
    }
    // `let x = Foo(...)` → 構築型。
    if let Some(call) = first_child_of_kind(n, "call_expression") {
        if let Some(id) = first_child_of_kind(call, "simple_identifier") {
            let t = text_of(id, source).trim().to_string();
            if is_pascal_case(&t) {
                return Some(t);
            }
        }
    }
    None
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
    fn calls_inside_init_deinit_subscript_and_computed_properties_are_collected() {
        let f = facts_of(&[(
            "M.swift",
            "struct Money {\n  let amount: Int\n  var doubled: Int { timesTwo() }\n  subscript(i: Int) -> Int { timesTwo() }\n  init(amount: Int) { self.amount = amount; validate() }\n  func timesTwo() -> Int { amount }\n  func validate() {}\n}\nclass Wallet {\n  deinit { cleanup() }\n}\nfunc cleanup() {}",
        )]);
        assert!(edges_from(&f, "Money.init").contains(&"Money.validate".to_string()), "init body calls");
        assert!(edges_from(&f, "Money.doubled").contains(&"Money.timesTwo".to_string()), "computed property body calls");
        assert!(edges_from(&f, "Money.subscript").contains(&"Money.timesTwo".to_string()), "subscript body calls");
        assert!(edges_from(&f, "Wallet.deinit").contains(&"cleanup".to_string()), "deinit body calls");
    }

    #[test]
    fn stored_property_initializer_construction_populates_instantiated() {
        // クラスレベルの `var money = Money(amount: 1)` は関数外だが構築サイト。RTA 用に拾う。
        let f = facts_of(&[("W.swift", "class Wallet {\n  var money = Money(amount: 1)\n}\nstruct Money { let amount: Int }")]);
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
        let out = fn_signatures_swift(&dir, &ResolvedConfig::empty());
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

    fn edges_from(f: &Facts, caller: &str) -> Vec<String> {
        let cid = f.funcs.iter().position(|x| x.name == caller).unwrap();
        let g = CallGraph::build(f, Precision::Cha);
        g.edges[cid].iter().map(|&t| f.funcs[t].name.clone()).collect()
    }

    #[test]
    fn implicit_self_call_does_not_leak_to_same_named_other_type() {
        // A.run calls bare helper(); B also has helper(). Precise self-resolution
        // must connect A.run only to A.helper, not B.helper.
        let f = facts_of(&[
            ("A.swift", "struct A {\n  func run() { helper() }\n  func helper() {}\n}"),
            ("B.swift", "struct B {\n  func helper() {}\n}"),
        ]);
        assert_eq!(edges_from(&f, "A.run"), vec!["A.helper".to_string()]);
    }

    #[test]
    fn field_receiver_resolves_to_declared_type() {
        // D.go calls a.foo() where `a: A`; A2 also has foo(). Must resolve to A.foo.
        let f = facts_of(&[(
            "D.swift",
            "struct A { func foo() {} }\nstruct A2 { func foo() {} }\nstruct D {\n  let a: A\n  func go() { a.foo() }\n}",
        )]);
        assert_eq!(edges_from(&f, "D.go"), vec!["A.foo".to_string()]);
    }

    #[test]
    fn call_as_function_via_field_is_captured() {
        // `layer(x)` where `layer: Net` calls Net.callAsFunction (ML convention).
        let f = facts_of(&[(
            "N.swift",
            "struct Net { func callAsFunction(_ x: Int) -> Int { x } }\nstruct Host {\n  let layer: Net\n  func run() { let _ = layer(1) }\n}",
        )]);
        assert_eq!(edges_from(&f, "Host.run"), vec!["Net.callAsFunction".to_string()]);
    }

    #[test]
    fn unresolved_receiver_falls_back_to_dynamic_and_rta_prunes() {
        // `mk().pong()` — receiver is a call result (type unknown) -> Dynamic.
        // Under RTA, only instantiated types' pong survive. B is constructed via
        // B(); C is not, so C.pong is pruned.
        let f = facts_of(&[(
            "X.swift",
            "struct B { func pong() {} }\nstruct C { func pong() {} }\nstruct X {\n  func mk() -> B { B() }\n  func run() { mk().pong() }\n}",
        )]);
        let cha = CallGraph::build(&f, Precision::Cha);
        let rta = CallGraph::build(&f, Precision::Rta);
        let names = |g: &CallGraph| -> Vec<String> {
            let rid = f.funcs.iter().position(|x| x.name == "X.run").unwrap();
            g.edges[rid].iter().map(|&t| f.funcs[t].name.clone()).collect()
        };
        assert!(names(&cha).contains(&"B.pong".to_string()) && names(&cha).contains(&"C.pong".to_string()));
        assert!(names(&rta).contains(&"B.pong".to_string()) && !names(&rta).contains(&"C.pong".to_string()));
    }
}
