//! Kotlin のコールグラフ事実抽出（tree-sitter-kotlin-ng）。
//!
//! Swift 版（`call_graph::swift`）と同じく、安定した Kotlin SCIP indexer を前提に
//! できないので konpu が持つ tree-sitter-kotlin-ng で `konpu_cg::Facts` を直接
//! 構築する。解釈エンジン（`konpu_cg::graph`）は言語中立なので無改変で再利用でき、
//! 循環/ハブ検出がそのまま効く。
//!
//! 名前解決は構文ベースの精密解決（Swift 版と同一の受け手型解決モデル）:
//! - 全関数を `TraitMethod{trait:"", method:<bare 名>}` の impl として登録。
//!   インスタンスメソッドは `for_type=<型>`、companion/自由関数は `for_type=""`。
//! - 呼び出しは受け手の型（self / 暗黙 self / `Type.` / ローカル / フィールド）を
//!   解決して Static に、解決できたが index 外なら External（エッジ無し）、
//!   本当に未解決なら Dynamic（同名全てに繋ぐ過大近似）。
//! - `Type(...)` 構築サイトを `instantiated` に入れて RTA を効かせる。
//!
//! Kotlin 固有: callable-value 規約は `invoke` 演算子（`foo(x)` = `foo.invoke(x)`）。
//! Swift の `callAsFunction` 特別扱いはメソッド名 "invoke" に対応する。
//!
use std::collections::HashMap;
use std::path::Path;

use tree_sitter::Node;

use konpu_cg::{Facts, FuncId};

use super::engine::{
    self, base_type_name, first_child_of_kind, first_named_child, is_pascal_case,
    last_child_of_kind, text_of, Index, Resolution, Resolver,
};
use super::{FnSig, MergeConstruction};
use crate::analyze::parser::Language;
use crate::analyze::template::ResolvedConfig;

/// collect_base_idents のノード種別（Kotlin）。
const IDENT_KINDS: engine::IdentKinds = engine::IdentKinds {
    // 引数ラベル `amount =` と navigation の末尾 `.amount` は参照ではない。
    skip: &["value_argument_label", "navigation_suffix"],
    self_kind: "this_expression",
    ident: "identifier",
};

/// Kotlin プロジェクトから Facts を構築する（外部ツール不要）。
pub fn facts_from_kotlin_project(path: &Path, config: &ResolvedConfig) -> Facts {
    facts_from_kotlin_sources(engine::project_sources(path, config, Language::Kotlin))
}

/// (path, source) の集合から Facts を構築する（テスト・in-memory 用）。
pub fn facts_from_kotlin_sources(sources: Vec<(std::path::PathBuf, String)>) -> Facts {
    engine::facts_from_sources(
        Language::Kotlin,
        sources,
        |root, src, fpath, facts, ids, index| collect_funcs(root, src, fpath, None, facts, ids, index),
        |root, src, r, facts| collect_calls(root, src, r, None, None, facts),
    )
}

/// Kotlin プロジェクトの全関数シグネチャ（preserve 検査 B/C 用）。
pub fn fn_signatures_kotlin(path: &Path, config: &ResolvedConfig) -> Vec<FnSig> {
    engine::fn_signatures(path, config, Language::Kotlin, |root, src, f, out| {
        walk_fn_sigs(root, src, f, None, false, out)
    })
}

fn walk_fn_sigs(
    n: Node,
    source: &str,
    path: &Path,
    self_ty: Option<&str>,
    in_companion: bool,
    out: &mut Vec<FnSig>,
) {
    match n.kind() {
        "class_declaration" => {
            let ty = type_name(n, source);
            if let Some(body) = first_child_of_kind(n, "class_body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    walk_fn_sigs(child, source, path, ty.as_deref(), false, out);
                }
            }
            return;
        }
        "companion_object" => {
            // companion のメソッドは関連関数（self 無し）。self_ty はそのまま渡す。
            if let Some(body) = first_child_of_kind(n, "class_body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    walk_fn_sigs(child, source, path, self_ty, true, out);
                }
            }
            return;
        }
        "function_declaration" => {
            if let Some(sig) = parse_fn_sig(n, source, path, self_ty, in_companion) {
                out.push(sig);
            }
            return; // ネスト関数は稀。降りない。
        }
        _ => {}
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        walk_fn_sigs(child, source, path, self_ty, in_companion, out);
    }
}

fn parse_fn_sig(
    n: Node,
    source: &str,
    path: &Path,
    self_ty: Option<&str>,
    in_companion: bool,
) -> Option<FnSig> {
    let name = func_name(n, source)?;
    let mut params = Vec::new();
    let mut params_named = Vec::new();
    if let Some(vps) = first_child_of_kind(n, "function_value_parameters") {
        let mut cur = vps.walk();
        for p in vps.children(&mut cur) {
            if p.kind() == "parameter" {
                if let Some(ty) = param_type(p, source) {
                    if let Some(id) = param_name(p, source) {
                        params_named.push((id, ty.clone()));
                    }
                    params.push(ty);
                }
            }
        }
    }
    let ret = fn_return_type(n, source);
    let mut constructions = Vec::new();
    if let Some(body) = first_child_of_kind(n, "function_body") {
        collect_constructions(body, source, &mut constructions);
    }
    Some(FnSig {
        path: path.to_path_buf(),
        line: n.start_position().row + 1,
        // インスタンスメソッドは暗黙 self がその型。companion/自由関数は self 無し。
        self_type: if in_companion { None } else { self_ty.map(str::to_string) },
        name,
        params,
        params_named,
        ret,
        constructions,
    })
}

/// 戻り型: function_value_parameters の後の直下 user_type / nullable_type。
fn fn_return_type(n: Node, source: &str) -> Option<String> {
    let mut cur = n.walk();
    let mut after_params = false;
    for c in n.children(&mut cur) {
        if c.kind() == "function_value_parameters" {
            after_params = true;
        } else if after_params && matches!(c.kind(), "user_type" | "nullable_type") {
            return Some(text_of(c, source).trim().to_string());
        }
    }
    None
}

/// パラメータの型テキスト（`x: T` の user_type）。
fn param_type(n: Node, source: &str) -> Option<String> {
    first_child_of_kind(n, "user_type").map(|c| text_of(c, source).trim().to_string())
}

/// パラメータの内部名（`x: T` → x）。最初の identifier。
fn param_name(n: Node, source: &str) -> Option<String> {
    first_child_of_kind(n, "identifier").map(|c| text_of(c, source).trim().to_string())
}

/// 本体内の `Type(...)` 構築サイト（検出器 C）。refs = 引数式が参照する基底識別子。
fn collect_constructions(n: Node, source: &str, out: &mut Vec<MergeConstruction>) {
    if n.kind() == "call_expression" {
        if let Some(first) = first_named_child(n) {
            if first.kind() == "identifier" {
                let ty = text_of(first, source).trim().to_string();
                if is_pascal_case(&ty) {
                    let mut refs = Vec::new();
                    if let Some(args) = first_child_of_kind(n, "call_suffix")
                        .or_else(|| first_child_of_kind(n, "value_arguments"))
                    {
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

/// class_declaration の型名（最初の identifier）。
fn type_name(n: Node, source: &str) -> Option<String> {
    first_child_of_kind(n, "identifier").map(|c| text_of(c, source).to_string())
}

/// 関数名（最初の identifier）。演算子は生名のまま（call 側も同じ生名で参照するため
/// plus->add のような正規化はしない）。
fn func_name(n: Node, source: &str) -> Option<String> {
    first_child_of_kind(n, "identifier").map(|c| text_of(c, source).trim().to_string())
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
            let ty = type_name(n, source);
            // ストアドプロパティ: primary constructor の class_parameter + class_body の
            // property_declaration。受け手が field のとき型解決に使う。
            if let Some(ty) = &ty {
                collect_fields(n, source, ty, index);
            }
            if let Some(body) = first_child_of_kind(n, "class_body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    collect_funcs(child, source, fpath, ty.as_deref(), facts, ids, index);
                }
            }
            return;
        }
        "companion_object" => {
            // companion のメンバは関連関数（for_type ""）。自由関数と同じ扱い。
            if let Some(body) = first_child_of_kind(n, "class_body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    collect_funcs(child, source, fpath, None, facts, ids, index);
                }
            }
            return;
        }
        "function_declaration" => {
            if let Some(bare) = func_name(n, source) {
                engine::register_func(&bare, n, fpath, enclosing, facts, ids, index);
            }
        }
        _ => {}
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_funcs(child, source, fpath, enclosing, facts, ids, index);
    }
}

/// 型 `ty` のストアドプロパティ（名前→基底型名）を index に登録する。
/// primary_constructor > class_parameters > class_parameter と
/// class_body > property_declaration の両方から。
fn collect_fields(class: Node, source: &str, ty: &str, index: &mut Index) {
    if let Some(pc) = first_child_of_kind(class, "primary_constructor") {
        if let Some(cps) = first_child_of_kind(pc, "class_parameters") {
            let mut cur = cps.walk();
            for cp in cps.children(&mut cur) {
                if cp.kind() == "class_parameter" {
                    if let (Some(name), Some(ut)) =
                        (first_child_of_kind(cp, "identifier"), first_child_of_kind(cp, "user_type"))
                    {
                        let name = text_of(name, source).trim().to_string();
                        let t = base_type_name(text_of(ut, source).trim());
                        index.fields.entry(ty.to_string()).or_default().insert(name, t);
                    }
                }
            }
        }
    }
    if let Some(body) = first_child_of_kind(class, "class_body") {
        let mut cur = body.walk();
        for child in body.children(&mut cur) {
            if child.kind() == "property_declaration" {
                if let (Some(name), Some(t)) = (prop_name(child, source), prop_type(child, source)) {
                    index.fields.entry(ty.to_string()).or_default().insert(name, base_type_name(&t));
                }
            }
        }
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
    // 型に入ったら enclosing を更新。
    if n.kind() == "class_declaration" {
        let ty = type_name(n, source);
        if let Some(body) = first_child_of_kind(n, "class_body") {
            let mut cur = body.walk();
            for child in body.children(&mut cur) {
                collect_calls(child, source, r, ty.as_deref(), caller, facts);
            }
        }
        return;
    }
    // companion のメソッドは関連関数（暗黙 self 無し）。enclosing は None にする。
    if n.kind() == "companion_object" {
        if let Some(body) = first_child_of_kind(n, "class_body") {
            let mut cur = body.walk();
            for child in body.children(&mut cur) {
                collect_calls(child, source, r, None, caller, facts);
            }
        }
        return;
    }
    // 関数に入ったら caller とローカル変数型を確定して本体を処理。
    if n.kind() == "function_declaration" {
        let c = r.ids.get(&n.id()).copied().or(caller);
        let locals = build_locals(n, source);
        if let Some(body) = first_child_of_kind(n, "function_body") {
            resolve_body(body, source, r, enclosing, &locals, c, facts);
        }
        return;
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_calls(child, source, r, enclosing, caller, facts);
    }
}

/// 関数本体（ラムダ含む）の呼び出しを解決してエッジ化。ネストした named 関数は
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

/// 関数のローカル変数の型（引数 + 本体の `val/var x: T` / `val x = T(...)`）。
fn build_locals(fn_node: Node, source: &str) -> HashMap<String, String> {
    let mut locals = HashMap::new();
    // 引数。
    if let Some(vps) = first_child_of_kind(fn_node, "function_value_parameters") {
        let mut cur = vps.walk();
        for c in vps.children(&mut cur) {
            if c.kind() == "parameter" {
                if let (Some(name), Some(t)) = (param_name(c, source), param_type(c, source)) {
                    locals.insert(name, base_type_name(&t));
                }
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
            // ponytail: 型は注釈 `val x: T` か構築 `val x = T(...)` からのみ拾う。
            // `val x = foo()`（call 戻り値）や chain `a.b()` はローカルが無型のまま残り、
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
    let Some(first) = first_named_child(call) else { return };

    let recv_type = |base: &str| engine::recv_type(base, enclosing, locals, index);

    let resolved: Resolution = match first.kind() {
        "identifier" => {
            let name = text_of(first, source).trim().to_string();
            if is_pascal_case(&name) {
                facts.instantiated.insert(name); // `Money(...)` 構築
                return;
            }
            // 値（ローカル/フィールド）なら invoke、そうでなければ関数/メソッド呼び。
            if let Some(t) = recv_type(&name) {
                engine::lookup_method(index, &t, "invoke")
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
            // `recv!!(...)` = invoke on recv。
            let Some(base) = first_child_of_kind(first, "identifier").map(|c| text_of(c, source).trim().to_string()) else {
                return;
            };
            match recv_type(&base) {
                Some(t) => engine::lookup_method(index, &t, "invoke"),
                None => Resolution::Dynamic("invoke".to_string()),
            }
        }
        _ => return,
    };

    engine::push_resolution(resolved, caller, facts);
}

/// navigation_expression の受け手基底識別子（this / 変数 / `x!!`）。チェーンや式は None。
fn nav_base_ident(nav: Node, source: &str) -> Option<String> {
    let base = first_named_child(nav)?;
    match base.kind() {
        "this_expression" => Some("self".to_string()),
        "identifier" => Some(text_of(base, source).trim().to_string()),
        "postfix_expression" => {
            first_child_of_kind(base, "identifier").map(|c| text_of(c, source).trim().to_string())
        }
        _ => None,
    }
}

/// navigation_expression のメソッド名。grammar により方言があるので両方対応する:
/// `[base . navigation_suffix{identifier}]`（旧）と `[base . identifier]`（tree-sitter-kotlin-ng）。
fn nav_method(nav: Node, source: &str) -> Option<String> {
    if let Some(suffix) = last_child_of_kind(nav, "navigation_suffix") {
        return first_child_of_kind(suffix, "identifier").map(|c| text_of(c, source).trim().to_string());
    }
    // 末尾の直下 identifier がメソッド名（base の identifier ではなく最後のもの）。
    let mut cur = nav.walk();
    nav.children(&mut cur)
        .filter(|c| c.kind() == "identifier")
        .last()
        .map(|c| text_of(c, source).trim().to_string())
}

/// property_declaration の名前（variable_declaration > identifier）。
fn prop_name(n: Node, source: &str) -> Option<String> {
    let vd = first_child_of_kind(n, "variable_declaration")?;
    first_child_of_kind(vd, "identifier").map(|c| text_of(c, source).trim().to_string())
}

/// property_declaration の型（`val x: T` の user_type、無ければ初期化子の構築型 `= Foo(...)`）。
fn prop_type(n: Node, source: &str) -> Option<String> {
    if let Some(vd) = first_child_of_kind(n, "variable_declaration") {
        if let Some(ut) = first_child_of_kind(vd, "user_type") {
            return Some(text_of(ut, source).trim().to_string());
        }
    }
    // `val x = Foo(...)` → 構築型。
    if let Some(call) = first_child_of_kind(n, "call_expression") {
        if let Some(id) = first_child_of_kind(call, "identifier") {
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
        facts_from_kotlin_sources(sources)
    }

    #[test]
    fn functions_and_qualified_names() {
        let f = facts_of(&[(
            "A.kt",
            "class A {\n  fun run() {}\n  companion object { fun make(): A { return A() } }\n}\nfun free() {}",
        )]);
        let names: Vec<&str> = f.funcs.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"A.run"));
        assert!(names.contains(&"make")); // companion = 関連関数, for_type ""
        assert!(names.contains(&"free"));
    }

    #[test]
    fn construction_populates_instantiated() {
        let f = facts_of(&[("M.kt", "class M {\n  fun go() { val x = Money(0) }\n}")]);
        assert!(f.instantiated.contains("Money"));
    }

    #[test]
    fn cha_connects_method_call_across_files() {
        // A.ping constructs B and calls b.pong(); B.pong constructs A and calls a.ping().
        let f = facts_of(&[
            ("A.kt", "class A {\n  fun ping() { val b = B()\n    b.pong() }\n}"),
            ("B.kt", "class B {\n  fun pong() { val a = A()\n    a.ping() }\n}"),
        ]);
        let g = CallGraph::build(&f, Precision::Cha);
        assert_eq!(g.cycles().iter().filter(|c| c.len() == 2).count(), 1);
    }

    fn sigs_of(files: &[(&str, &str)]) -> Vec<FnSig> {
        let dir = std::env::temp_dir().join(format!("konpu_cgkt_sig_{}", files.len()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, src) in files {
            std::fs::write(dir.join(name), src).unwrap();
        }
        let out = fn_signatures_kotlin(&dir, &ResolvedConfig::empty());
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
            "R.kt",
            "class R {\n  fun total(items: List<Money>): Money { return Money(0) }\n  fun describe(a: Money, b: Money): String { val m = Money(a.amount + b.amount)\n    return \"x\" }\n}",
        )]);
        let total = sigs.iter().find(|s| s.name == "total").unwrap();
        assert!(is_aggregation_shape(total, "Money")); // List<Money> -> Money
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
            ("A.kt", "class A {\n  fun run() { helper() }\n  fun helper() {}\n}"),
            ("B.kt", "class B {\n  fun helper() {}\n}"),
        ]);
        assert_eq!(edges_from(&f, "A.run"), vec!["A.helper".to_string()]);
    }

    #[test]
    fn field_receiver_resolves_to_declared_type() {
        // D.go calls a.foo() where `a: A`; A2 also has foo(). Must resolve to A.foo.
        let f = facts_of(&[(
            "D.kt",
            "class A {\n  fun foo() {}\n}\nclass A2 {\n  fun foo() {}\n}\nclass D(val a: A) {\n  fun go() { a.foo() }\n}",
        )]);
        assert_eq!(edges_from(&f, "D.go"), vec!["A.foo".to_string()]);
    }

    #[test]
    fn invoke_via_field_is_captured() {
        // `layer(1)` where `layer: Net` calls Net.invoke (callable-value convention).
        let f = facts_of(&[(
            "N.kt",
            "class Net {\n  operator fun invoke(x: Int): Int { return x }\n}\nclass Host(val layer: Net) {\n  fun run() { val y = layer(1) }\n}",
        )]);
        assert_eq!(edges_from(&f, "Host.run"), vec!["Net.invoke".to_string()]);
    }

    #[test]
    fn unresolved_receiver_falls_back_to_dynamic_and_rta_prunes() {
        // `mk().pong()` — receiver is a call result (type unknown) -> Dynamic.
        // Under RTA, only instantiated types' pong survive. B is constructed via
        // B(); C is not, so C.pong is pruned.
        let f = facts_of(&[(
            "X.kt",
            "class B {\n  fun pong() {}\n}\nclass C {\n  fun pong() {}\n}\nclass X {\n  fun mk(): B { return B() }\n  fun run() { mk().pong() }\n}",
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
