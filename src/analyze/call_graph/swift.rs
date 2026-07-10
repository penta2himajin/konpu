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

use super::{FnSig, MergeConstruction};
use crate::analyze::parser::{self, Language};

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

    // Pass 1: 関数定義を登録し、精密解決用の索引（型メソッド・自由関数・フィールド型）を作る。
    let mut index = Index::default();
    let mut fn_ids: Vec<HashMap<usize, FuncId>> = Vec::with_capacity(parsed.len());
    for (fpath, src, tree) in &parsed {
        let mut ids = HashMap::new();
        collect_funcs(tree.root_node(), src, fpath, None, &mut facts, &mut ids, &mut index);
        fn_ids.push(ids);
    }

    // Pass 2: 各関数本体の呼び出しを、受け手の型を解決して精密にエッジ化。
    for (fi, (_, src, tree)) in parsed.iter().enumerate() {
        let r = Resolver { ids: &fn_ids[fi], index: &index };
        collect_calls(tree.root_node(), src, &r, None, None, &mut facts);
    }
    facts
}

/// Pass 2 で不変な参照（node.id()→FuncId と精密解決索引）を束ねる。
struct Resolver<'a> {
    ids: &'a HashMap<usize, FuncId>,
    index: &'a Index,
}

/// 精密な呼び出し解決のための索引。
#[derive(Default)]
struct Index {
    /// (型, メソッド名) -> 候補 FuncId 群（同名オーバーロードは複数）。
    type_methods: HashMap<(String, String), Vec<FuncId>>,
    /// 自由関数名 -> FuncId 群。
    free_funcs: HashMap<String, Vec<FuncId>>,
    /// 型 -> (ストアドプロパティ名 -> 基底型名)。受け手が field のとき型解決に使う。
    fields: HashMap<String, HashMap<String, String>>,
}

/// 型文字列の基底名（`A?`→A、`[T]`→そのまま=解決不能、`Foo<T>`→Foo）。
fn base_type_name(s: &str) -> String {
    let mut s = s.trim().trim_end_matches(['?', '!']).trim();
    if let Some(i) = s.find('<') {
        s = s[..i].trim();
    }
    s.rsplit(['.', ':']).next().unwrap_or(s).trim().to_string()
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
                let name = match enclosing {
                    Some(t) => format!("{t}.{bare}"),
                    None => bare.clone(),
                };
                let id = facts.add_func(name, fpath.to_path_buf(), n.start_position().row + 1);
                ids.insert(n.id(), id);
                facts.impls.push(ImplEntry {
                    trait_method: TraitMethod::new("", bare.clone()),
                    for_type: enclosing.unwrap_or("").to_string(),
                    func: id,
                });
                match enclosing {
                    Some(t) => index.type_methods.entry((t.to_string(), bare)).or_default().push(id),
                    None => index.free_funcs.entry(bare).or_default().push(id),
                }
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

    // 受け手の型を解決するクロージャ。
    let recv_type = |base: &str| -> Option<String> {
        if base == "self" {
            enclosing.map(str::to_string)
        } else if is_pascal_case(base) {
            Some(base.to_string())
        } else {
            locals
                .get(base)
                .cloned()
                .or_else(|| enclosing.and_then(|t| index.fields.get(t)).and_then(|f| f.get(base)).cloned())
        }
    };

    let resolved: Resolution = match first.kind() {
        "simple_identifier" => {
            let name = text_of(first, source).trim().to_string();
            if is_pascal_case(&name) {
                facts.instantiated.insert(name); // `Money(...)` 構築
                return;
            }
            // 値（ローカル/フィールド）なら callAsFunction、そうでなければ関数/メソッド呼び。
            if let Some(t) = recv_type(&name) {
                lookup_method(index, &t, "callAsFunction")
            } else {
                resolve_bare(index, enclosing, &name)
            }
        }
        "navigation_expression" => {
            let Some(method) = nav_method(first, source) else { return };
            match nav_base_ident(first, source).and_then(|b| recv_type(&b)) {
                Some(t) => lookup_method(index, &t, &method),
                None => Resolution::Dynamic(method),
            }
        }
        "postfix_expression" => {
            // `recv!(...)` / `recv?(...)` = callAsFunction on recv。
            let Some(base) = first_child_of_kind(first, "simple_identifier").map(|c| text_of(c, source).trim().to_string()) else {
                return;
            };
            match recv_type(&base) {
                Some(t) => lookup_method(index, &t, "callAsFunction"),
                None => Resolution::Dynamic("callAsFunction".to_string()),
            }
        }
        _ => return,
    };

    let Some(c) = caller else { return };
    match resolved {
        Resolution::Targets(target_ids) => {
            for t in target_ids {
                facts.calls.push(CallSite { caller: c, target: CallTargetKind::Static(t) });
            }
        }
        Resolution::Dynamic(m) => {
            facts.calls.push(CallSite { caller: c, target: CallTargetKind::Dynamic(TraitMethod::new("", m)) });
        }
        Resolution::External => {} // 受け手の型は判ったが index 外＝外部/継承。エッジ無し。
    }
}

enum Resolution {
    /// 型が解決でき、そのメソッドに厳密に結んだ（同名オーバーロードは複数）。
    Targets(Vec<FuncId>),
    /// 型未解決 → 同名メソッド全てに繋ぐ過大近似（偽陰性を出さない）。
    Dynamic(String),
    /// 受け手の型は具体解決できたが index に無い（外部ライブラリ型/継承）→ エッジ無し。
    /// Dynamic で全同名に繋ぐより精密（受け手型が判っている以上、他の自型メソッドではない）。
    External,
}

fn lookup_method(index: &Index, ty: &str, method: &str) -> Resolution {
    match index.type_methods.get(&(ty.to_string(), method.to_string())) {
        Some(ids) => Resolution::Targets(ids.clone()),
        None => Resolution::External,
    }
}

/// 受け手なしの呼び出し: 内包型のメソッド（暗黙 self）→ 自由関数 → Dynamic。
fn resolve_bare(index: &Index, enclosing: Option<&str>, name: &str) -> Resolution {
    if let Some(t) = enclosing {
        if let Some(ids) = index.type_methods.get(&(t.to_string(), name.to_string())) {
            return Resolution::Targets(ids.clone());
        }
    }
    if let Some(ids) = index.free_funcs.get(name) {
        return Resolution::Targets(ids.clone());
    }
    Resolution::Dynamic(name.to_string())
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
