//! TypeScript のコールグラフ事実抽出（tree-sitter-typescript）。
//!
//! Swift/Kotlin 版と同じく、konpu が持つ tree-sitter で `konpu_cg::Facts` を直接
//! 構築する（安定した TS SCIP indexer を前提にしない）。解釈エンジン
//! （`konpu_cg::graph`）は言語中立なので無改変で再利用でき、循環/ハブ検出がそのまま効く。
//!
//! 名前解決は Swift/Kotlin と同じ受け手型解決モデルだが、TS の構文差を吸収する:
//! - 構築は `new Foo(...)`（`new_expression`）。Swift/Kotlin の `Foo(...)` call とは別ノード
//!   なので、`instantiated` は new_expression から埋める。
//! - メンバ呼びは `a.foo()` / `this.foo()`（`member_expression`）。TS はインスタンス
//!   メンバを常に `this.` で修飾するので、受け手は `this.a.foo()` のようにネストする。
//!   Swift/Kotlin の平坦な受け手解決の代わりに、`resolve_receiver` が
//!   `this` / ローカル / `this.<field>` / `Type.` を再帰的に型へ解決する。
//! - TS には callable-value 規約（Swift callAsFunction / Kotlin invoke）が無く、
//!   クラスメンバは `this.`/`Type.` 修飾必須なので、bare 識別子呼び `foo()` は
//!   レキシカルスコープで解決する: 同一ファイルの自由関数（在れば必ずそれ）→
//!   全ファイルの同名自由関数（import 先の過大近似）→ Dynamic。
//! - `const f = (…) => …`（トップレベル/ネスト）と class field の関数値も関数として
//!   登録する（TS の支配的な定義スタイル）。
//!
//! 精度モデル（Swift/Kotlin と共通）: 型が解決できたら Static、解決できたが index 外なら
//! External（エッジ無し）、本当に未解決なら Dynamic（同名全てに繋ぐ過大近似）。

use std::collections::HashMap;
use std::path::Path;

use tree_sitter::Node;

use konpu_cg::{Facts, FuncId};

use super::engine::{
    self, base_type_name, first_named_child, is_pascal_case, text_of, Index, Resolution, Resolver,
};
use super::{FnSig, MergeConstruction};
use crate::analyze::parser::Language;
use crate::analyze::template::ResolvedConfig;

/// 型宣言ノード（class / abstract class）。interface/enum は呼び出し本体を持たないので対象外。
const CLASS_KINDS: &[&str] = &["class_declaration", "abstract_class_declaration"];

/// collect_base_idents のノード種別（TS）。
const IDENT_KINDS: engine::IdentKinds = engine::IdentKinds {
    // member の property `.amount` は参照ではない。object 側だけ辿る。
    skip: &["property_identifier", "shorthand_property_identifier"],
    self_kind: "this",
    ident: "identifier",
};

/// TS プロジェクトから Facts を構築する（外部ツール不要）。
/// `config.exclude`（konpu.toml）に一致するファイルは除外する（テスト等をハブ集計から外す）。
pub fn facts_from_ts_project(path: &Path, config: &ResolvedConfig) -> Facts {
    facts_from_ts_sources(engine::project_sources(path, config, Language::Ts))
}

/// (path, source) の集合から Facts を構築する（テスト・in-memory 用）。
pub fn facts_from_ts_sources(sources: Vec<(std::path::PathBuf, String)>) -> Facts {
    engine::facts_from_sources(
        Language::Ts,
        sources,
        |root, src, fpath, facts, ids, index| collect_funcs(root, src, fpath, None, facts, ids, index),
        |root, src, fpath, r, facts| collect_calls(root, src, fpath, r, None, None, facts),
    )
}

/// TS プロジェクトの全関数シグネチャ（preserve 検査 B/C 用）。
/// `config.exclude` に一致するファイルは除外する（facts と対象集合を揃える）。
pub fn fn_signatures_ts(path: &Path, config: &ResolvedConfig) -> Vec<FnSig> {
    engine::fn_signatures(path, config, Language::Ts, |root, src, f, out| {
        walk_fn_sigs(root, src, f, None, out)
    })
}

fn walk_fn_sigs(n: Node, source: &str, path: &Path, self_ty: Option<&str>, out: &mut Vec<FnSig>) {
    if CLASS_KINDS.contains(&n.kind()) {
        let ty = type_name(n, source);
        if let Some(body) = n.child_by_field_name("body") {
            let mut cur = body.walk();
            for child in body.children(&mut cur) {
                walk_fn_sigs(child, source, path, ty.as_deref(), out);
            }
        }
        return;
    }
    if matches!(n.kind(), "method_definition" | "function_declaration") {
        if let Some(sig) = parse_fn_sig(n, source, path, self_ty) {
            out.push(sig);
        }
        return; // ネスト関数は稀。降りない。
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
    if let Some(fps) = n.child_by_field_name("parameters") {
        let mut cur = fps.walk();
        for p in fps.children(&mut cur) {
            if matches!(p.kind(), "required_parameter" | "optional_parameter") {
                if let Some(ty) = param_type(p, source) {
                    if let Some(id) = param_name(p, source) {
                        params_named.push((id, ty.clone()));
                    }
                    params.push(ty);
                }
            }
        }
    }
    let ret = n.child_by_field_name("return_type").and_then(|ann| type_ann_text(ann, source));
    let mut constructions = Vec::new();
    if let Some(body) = n.child_by_field_name("body") {
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

/// メンバに `static` キーワードが付いているか。
fn has_static(n: Node, source: &str) -> bool {
    let mut cur = n.walk();
    n.children(&mut cur).any(|c| !c.is_named() && text_of(c, source) == "static")
}

/// パラメータの型テキスト（`x: T` の type_annotation）。
fn param_type(n: Node, source: &str) -> Option<String> {
    n.child_by_field_name("type").and_then(|ann| type_ann_text(ann, source))
}

/// パラメータの内部名（`x: T` → x）。pattern の identifier。
fn param_name(n: Node, source: &str) -> Option<String> {
    let pat = n.child_by_field_name("pattern")?;
    if pat.kind() == "identifier" {
        Some(text_of(pat, source).trim().to_string())
    } else {
        None
    }
}

/// `type_annotation`（`: T`）から型テキストを取り出す。
fn type_ann_text(ann: Node, source: &str) -> Option<String> {
    let mut cur = ann.walk();
    ann.children(&mut cur)
        .find(|c| c.is_named())
        .map(|t| text_of(t, source).trim().to_string())
}

/// 本体内の `new Type(...)` 構築サイト（検出器 C）。refs = 引数式が参照する基底識別子。
fn collect_constructions(n: Node, source: &str, out: &mut Vec<MergeConstruction>) {
    if n.kind() == "new_expression" {
        if let Some(ty) = new_type(n, source) {
            let mut refs = Vec::new();
            if let Some(args) = n.child_by_field_name("arguments") {
                engine::collect_base_idents(&IDENT_KINDS, args, source, &mut refs);
            }
            out.push(MergeConstruction { type_name: ty, line: n.start_position().row + 1, refs });
        }
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_constructions(child, source, out);
    }
}

/// `new_expression` の構築型名（PascalCase の constructor 識別子）。
fn new_type(n: Node, source: &str) -> Option<String> {
    let ctor = n.child_by_field_name("constructor")?;
    if ctor.kind() == "identifier" {
        let t = text_of(ctor, source).trim().to_string();
        if is_pascal_case(&t) {
            return Some(t);
        }
    }
    None
}

/// 型宣言/メソッド/関数の名前（`name` フィールド）。
fn type_name(n: Node, source: &str) -> Option<String> {
    n.child_by_field_name("name").map(|c| text_of(c, source).trim().to_string())
}

fn func_name(n: Node, source: &str) -> Option<String> {
    n.child_by_field_name("name").map(|c| text_of(c, source).trim().to_string())
}

/// 関数/メソッド/arrow の戻り型テキスト（`return_type` フィールドの注釈）。
fn fn_ret(n: Node, source: &str) -> Option<String> {
    n.child_by_field_name("return_type").and_then(|ann| type_ann_text(ann, source))
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
    if CLASS_KINDS.contains(&n.kind()) {
        let ty = type_name(n, source);
        if let Some(ty) = &ty {
            collect_fields(n, source, ty, index);
        }
        if let Some(body) = n.child_by_field_name("body") {
            let mut cur = body.walk();
            for child in body.children(&mut cur) {
                collect_funcs(child, source, fpath, ty.as_deref(), facts, ids, index);
            }
        }
        return;
    }
    if matches!(n.kind(), "method_definition" | "function_declaration") {
        if let Some(bare) = func_name(n, source) {
            let ret = fn_ret(n, source);
            engine::register_func(&bare, n, fpath, enclosing, ret.as_deref(), facts, ids, index);
        }
    }
    // `const f = (…) => …` / `const f = function () {}` は TS の支配的な関数定義。
    // ids のキーは関数値ノード（collect_calls/resolve_body が値ノードで引くため）。
    if n.kind() == "variable_declarator" {
        if let (Some(name), Some(value)) = (n.child_by_field_name("name"), n.child_by_field_name("value")) {
            if name.kind() == "identifier" && matches!(value.kind(), "arrow_function" | "function_expression") {
                let bare = text_of(name, source).trim().to_string();
                let ret = fn_ret(value, source);
                engine::register_func(&bare, value, fpath, enclosing, ret.as_deref(), facts, ids, index);
            }
        }
    }
    // class field の関数値（`handler = (x) => …`）はそのクラスのメソッド。
    if n.kind() == "public_field_definition" {
        if let Some(value) = n.child_by_field_name("value") {
            if matches!(value.kind(), "arrow_function" | "function_expression") {
                if let Some(bare) = field_name(n, source) {
                    let ret = fn_ret(value, source);
                    engine::register_func(&bare, value, fpath, enclosing, ret.as_deref(), facts, ids, index);
                }
            }
        }
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_funcs(child, source, fpath, enclosing, facts, ids, index);
    }
}

/// 型 `ty` のフィールド（名前→基底型名）を index に登録する。
/// `public_field_definition`（非 static）と constructor パラメータプロパティ
/// （`constructor(readonly x: T)` / accessibility 修飾子付き）の両方から。
fn collect_fields(class: Node, source: &str, ty: &str, index: &mut Index) {
    let Some(body) = class.child_by_field_name("body") else { return };
    let mut cur = body.walk();
    for child in body.children(&mut cur) {
        match child.kind() {
            "public_field_definition" if !has_static(child, source) => {
                if let (Some(name), Some(t)) = (field_name(child, source), field_type(child, source)) {
                    index.fields.entry(ty.to_string()).or_default().insert(name, base_type_name(&t));
                }
            }
            "method_definition" if func_name(child, source).as_deref() == Some("constructor") => {
                collect_param_properties(child, source, ty, index);
            }
            _ => {}
        }
    }
}

/// constructor のパラメータプロパティ（`readonly`/accessibility 修飾子付き引数）を
/// フィールドとして index に登録する。
fn collect_param_properties(ctor: Node, source: &str, ty: &str, index: &mut Index) {
    let Some(fps) = ctor.child_by_field_name("parameters") else { return };
    let mut cur = fps.walk();
    for p in fps.children(&mut cur) {
        if p.kind() != "required_parameter" && p.kind() != "optional_parameter" {
            continue;
        }
        // アクセス修飾子 or readonly が付いた引数だけがインスタンスフィールドになる。
        let is_property = {
            let mut c = p.walk();
            p.children(&mut c)
                .any(|ch| ch.kind() == "accessibility_modifier" || (!ch.is_named() && text_of(ch, source) == "readonly"))
        };
        if !is_property {
            continue;
        }
        if let (Some(name), Some(t)) = (param_name(p, source), param_type(p, source)) {
            index.fields.entry(ty.to_string()).or_default().insert(name, base_type_name(&t));
        }
    }
}

/// public_field_definition の名前（property_identifier）。
fn field_name(n: Node, source: &str) -> Option<String> {
    n.child_by_field_name("name").map(|c| text_of(c, source).trim().to_string())
}

/// public_field_definition の型（type_annotation、無ければ初期化子 `= new T(...)`）。
fn field_type(n: Node, source: &str) -> Option<String> {
    if let Some(ann) = n.child_by_field_name("type") {
        if let Some(t) = type_ann_text(ann, source) {
            return Some(t);
        }
    }
    n.child_by_field_name("value").and_then(|v| new_type(v, source))
}

fn collect_calls(
    n: Node,
    source: &str,
    fpath: &Path,
    r: &Resolver,
    enclosing: Option<&str>,
    caller: Option<FuncId>,
    facts: &mut Facts,
) {
    // 型に入ったら enclosing を更新。
    if CLASS_KINDS.contains(&n.kind()) {
        let ty = type_name(n, source);
        if let Some(body) = n.child_by_field_name("body") {
            let mut cur = body.walk();
            for child in body.children(&mut cur) {
                collect_calls(child, source, fpath, r, ty.as_deref(), caller, facts);
            }
        }
        return;
    }
    // 関数/メソッドに入ったら caller とローカル変数型を確定して本体を処理。
    // static メソッドは暗黙 self が無いが、TS は静的呼びも `Type.m()` で明示するので
    // enclosing はそのままでよい（bare self 呼びが無いため誤解決しない）。
    if matches!(n.kind(), "method_definition" | "function_declaration") {
        let c = r.ids.get(&n.id()).copied().or(caller);
        let locals = build_locals(n, source, r.index, enclosing);
        if let Some(body) = n.child_by_field_name("body") {
            resolve_body(body, source, fpath, r, enclosing, &locals, c, facts);
        }
        return;
    }
    // 変数/フィールドの初期化式: 関数値なら登録済み caller で本体を処理、
    // 非関数なら caller 無しで walk して構築 `new T(...)` を instantiated に拾う。
    if matches!(n.kind(), "variable_declarator" | "public_field_definition") {
        if let Some(v) = n.child_by_field_name("value") {
            if matches!(v.kind(), "arrow_function" | "function_expression") {
                let c = r.ids.get(&v.id()).copied().or(caller);
                let locals = build_locals(v, source, r.index, enclosing);
                if let Some(body) = v.child_by_field_name("body") {
                    resolve_body(body, source, fpath, r, enclosing, &locals, c, facts);
                }
            } else {
                let locals = HashMap::new();
                resolve_body(v, source, fpath, r, enclosing, &locals, None, facts);
            }
        }
        return;
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_calls(child, source, fpath, r, enclosing, caller, facts);
    }
}

/// 関数本体（アロー関数含む）の呼び出しを解決してエッジ化。ネストした named 関数は
/// それ自身の caller で再入する。`new T(...)` は instantiated を埋める。
#[allow(clippy::too_many_arguments)] // 文脈スレッディング。構造体化は間接化が勝るだけ。
fn resolve_body(
    n: Node,
    source: &str,
    fpath: &Path,
    r: &Resolver,
    enclosing: Option<&str>,
    locals: &HashMap<String, String>,
    caller: Option<FuncId>,
    facts: &mut Facts,
) {
    match n.kind() {
        "function_declaration" => {
            collect_calls(n, source, fpath, r, enclosing, caller, facts);
            return;
        }
        // ネストした `const g = () => …`（Pass 1 で登録済み）は g 自身の caller で再入。
        "variable_declarator" => {
            let is_registered_fn = n.child_by_field_name("value").is_some_and(|v| {
                matches!(v.kind(), "arrow_function" | "function_expression") && r.ids.contains_key(&v.id())
            });
            if is_registered_fn {
                collect_calls(n, source, fpath, r, enclosing, caller, facts);
                return;
            }
        }
        "new_expression" => {
            if let Some(ty) = new_type(n, source) {
                facts.instantiated.insert(ty);
            }
        }
        "call_expression" => {
            resolve_call(n, source, fpath, r.index, enclosing, locals, caller, facts);
        }
        _ => {}
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        resolve_body(child, source, fpath, r, enclosing, locals, caller, facts);
    }
}

/// 関数のローカル変数の型（引数 + 本体の `const/let x: T` / `= new T(...)`）。
fn build_locals(fn_node: Node, source: &str, index: &Index, enclosing: Option<&str>) -> HashMap<String, String> {
    let mut locals = HashMap::new();
    if let Some(fps) = fn_node.child_by_field_name("parameters") {
        let mut cur = fps.walk();
        for p in fps.children(&mut cur) {
            if matches!(p.kind(), "required_parameter" | "optional_parameter") {
                if let (Some(name), Some(t)) = (param_name(p, source), param_type(p, source)) {
                    locals.insert(name, base_type_name(&t));
                }
            }
        }
    }
    if let Some(body) = fn_node.child_by_field_name("body") {
        collect_locals(body, source, index, enclosing, &mut locals);
    }
    locals
}

fn collect_locals(
    n: Node,
    source: &str,
    index: &Index,
    enclosing: Option<&str>,
    out: &mut HashMap<String, String>,
) {
    if n.kind() == "variable_declarator" {
        if let Some(name) = n.child_by_field_name("name") {
            if name.kind() == "identifier" {
                // 型は注釈 `const x: T`、構築 `= new T(...)`、または呼び出しの戻り型
                // （`= foo()` / `= recv.m()` の戻り型伝播）。宣言順走査なので
                // locals-so-far（out）で受け手も解決できる。
                // ponytail: 索引に無い戻り型（外部 API・複雑な式）は無型のまま → Dynamic。
                let ty = n
                    .child_by_field_name("type")
                    .and_then(|ann| type_ann_text(ann, source))
                    .or_else(|| n.child_by_field_name("value").and_then(|v| new_type(v, source)))
                    .or_else(|| {
                        n.child_by_field_name("value")
                            .filter(|v| v.kind() == "call_expression")
                            .and_then(|v| call_ret_type(v, source, index, enclosing, out))
                    });
                if let Some(t) = ty {
                    out.insert(text_of(name, source).trim().to_string(), base_type_name(&t));
                }
            }
        }
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_locals(child, source, index, enclosing, out);
    }
}

/// 呼び出し式の戻り型（戻り型伝播）。`foo()` は自由関数の戻り型、`recv.m()` は
/// 受け手（resolve_receiver。chain の call は再帰でここへ戻る）の型メソッドの戻り型。
fn call_ret_type(
    call: Node,
    source: &str,
    index: &Index,
    enclosing: Option<&str>,
    locals: &HashMap<String, String>,
) -> Option<String> {
    let f = call.child_by_field_name("function")?;
    match f.kind() {
        "identifier" => index.free_returns.get(text_of(f, source).trim()).cloned(),
        "member_expression" => {
            let obj = f.child_by_field_name("object")?;
            let prop = f.child_by_field_name("property")?;
            let recv_ty = resolve_receiver(obj, source, enclosing, locals, index)?;
            index.returns.get(&(recv_ty, text_of(prop, source).trim().to_string())).cloned()
        }
        _ => None,
    }
}

/// call_expression を解決してエッジを facts に足す。
#[allow(clippy::too_many_arguments)]
fn resolve_call(
    call: Node,
    source: &str,
    fpath: &Path,
    index: &Index,
    enclosing: Option<&str>,
    locals: &HashMap<String, String>,
    caller: Option<FuncId>,
    facts: &mut Facts,
) {
    let Some(func) = call.child_by_field_name("function") else { return };

    let resolved: Resolution = match func.kind() {
        // bare 呼び `foo()`: TS はレキシカルスコープ＝クラスメンバは `this.`/`Type.`
        // 修飾必須なので enclosing は見ない。同一ファイルの自由関数が在れば必ずそれ
        // （import はファイル内定義に負ける）。無ければ全ファイルの同名（import 先の
        // 過大近似、健全）。PascalCase 構築は `new`（別ノード）なのでここでは扱わない。
        "identifier" => resolve_bare_ts(index, facts, text_of(func, source).trim(), fpath),
        // メンバ呼び `recv.method()`。
        "member_expression" => {
            let Some(prop) = func.child_by_field_name("property") else { return };
            let method = text_of(prop, source).trim().to_string();
            let recv = func.child_by_field_name("object");
            match recv.and_then(|o| resolve_receiver(o, source, enclosing, locals, index)) {
                Some(t) => engine::lookup_method(index, &t, &method),
                None => Resolution::Dynamic(method),
            }
        }
        _ => return,
    };

    engine::push_resolution(resolved, caller, facts);
}

/// TS の bare 呼び解決: 同一ファイルの自由関数優先 → 全ファイル → Dynamic。
fn resolve_bare_ts(index: &Index, facts: &Facts, name: &str, file: &Path) -> Resolution {
    if let Some(ids) = index.free_funcs.get(name) {
        let local: Vec<FuncId> =
            ids.iter().copied().filter(|&i| facts.funcs[i].path == file).collect();
        return Resolution::Targets(if local.is_empty() { ids.clone() } else { local });
    }
    Resolution::Dynamic(name.to_string())
}

/// 受け手式の型を再帰的に解決する。TS の `this.a.foo()` のようなネスト参照に対応:
/// - `this` → 内包型
/// - `identifier` → PascalCase なら型（静的呼び `Type.m()`）、ローカル/引数なら宣言型
/// - `this.<field>` / `<typed>.<field>` → フィールドの型（`index.fields` を辿る）
/// - call の戻り値やそれ以外 → None（未解決 → Dynamic に落ちる）
fn resolve_receiver(
    node: Node,
    source: &str,
    enclosing: Option<&str>,
    locals: &HashMap<String, String>,
    index: &Index,
) -> Option<String> {
    match node.kind() {
        "this" => enclosing.map(str::to_string),
        "identifier" => {
            let name = text_of(node, source).trim();
            if is_pascal_case(name) {
                Some(name.to_string())
            } else {
                locals.get(name).cloned()
            }
        }
        "member_expression" => {
            let obj = node.child_by_field_name("object")?;
            let prop = node.child_by_field_name("property")?;
            let base_ty = resolve_receiver(obj, source, enclosing, locals, index)?;
            index.fields.get(&base_ty)?.get(text_of(prop, source).trim()).cloned()
        }
        // `(expr)` 括弧はそのまま中身へ。
        "parenthesized_expression" => {
            let inner = first_named_child(node)?;
            resolve_receiver(inner, source, enclosing, locals, index)
        }
        // chain `this.factory().run()`: 呼び出しの戻り型で受け手を解決（戻り型伝播）。
        "call_expression" => call_ret_type(node, source, index, enclosing, locals),
        _ => None,
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use konpu_cg::{CallGraph, Precision};
    use std::path::PathBuf;

    fn facts_of(files: &[(&str, &str)]) -> Facts {
        let sources = files.iter().map(|(p, s)| (PathBuf::from(p), s.to_string())).collect();
        facts_from_ts_sources(sources)
    }

    #[test]
    fn functions_and_qualified_names() {
        let f = facts_of(&[(
            "A.ts",
            "class A {\n  run() {}\n  static make(): A { return new A(); }\n}\nfunction free() {}",
        )]);
        let names: Vec<&str> = f.funcs.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"A.run"));
        assert!(names.contains(&"A.make")); // static も for_type=A（`A.make()` で解決）
        assert!(names.contains(&"free"));
    }

    #[test]
    fn construction_populates_instantiated() {
        let f = facts_of(&[("M.ts", "class M {\n  go() { const x = new Money(0); }\n}")]);
        assert!(f.instantiated.contains("Money"));
    }

    #[test]
    fn cha_connects_method_call_across_files() {
        // A.ping constructs B and calls b.pong(); B.pong constructs A and calls a.ping().
        let f = facts_of(&[
            ("A.ts", "class A {\n  ping() { const b = new B(); b.pong(); }\n}"),
            ("B.ts", "class B {\n  pong() { const a = new A(); a.ping(); }\n}"),
        ]);
        let g = CallGraph::build(&f, Precision::Cha);
        assert_eq!(g.cycles().iter().filter(|c| c.len() == 2).count(), 1);
    }

    fn sigs_of(files: &[(&str, &str)]) -> Vec<FnSig> {
        let dir = std::env::temp_dir().join(format!("konpu_cgts_sig_{}", files.len()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, src) in files {
            std::fs::write(dir.join(name), src).unwrap();
        }
        let out = fn_signatures_ts(&dir, &ResolvedConfig::empty());
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
            "R.ts",
            "class R {\n  total(items: Money[]): Money { return new Money(0); }\n  describe(a: Money, b: Money): string { const m = new Money(a.amount + b.amount); return \"x\"; }\n}",
        )]);
        let total = sigs.iter().find(|s| s.name == "total").unwrap();
        assert!(is_aggregation_shape(total, "Money")); // Money[] -> Money
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
    fn arrow_const_functions_register_and_collect_calls() {
        // `const f = (…) => …` は TS の支配的な関数定義スタイル。関数として登録し、
        // 本体の呼び出しも f を caller としてエッジ化する。
        let f = facts_of(&[(
            "a.ts",
            "export const add = (a: number): number => free(a);\nexport function free(x: number): number { return x; }\nexport function user(): number { return add(1); }",
        )]);
        let names: Vec<&str> = f.funcs.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"add"), "arrow const registered: {names:?}");
        assert!(edges_from(&f, "add").contains(&"free".to_string()), "calls inside arrow body");
        assert!(edges_from(&f, "user").contains(&"add".to_string()), "call to arrow const resolves");
    }

    #[test]
    fn nested_arrow_const_gets_its_own_caller() {
        // 関数本体内の `const g = () => …` は外側でなく g 自身の caller でエッジ化。
        let f = facts_of(&[(
            "n.ts",
            "export function outer(): void {\n  const g = (): number => free(1);\n  g();\n}\nexport function free(x: number): number { return x; }",
        )]);
        assert!(edges_from(&f, "g").contains(&"free".to_string()), "nested arrow body attributed to g");
        assert!(edges_from(&f, "outer").contains(&"g".to_string()), "outer calls g");
    }

    #[test]
    fn class_field_arrow_registers_as_method() {
        let f = facts_of(&[(
            "s.ts",
            "class Svc {\n  handler = (x: number) => this.helper(x);\n  helper(x: number): number { return x; }\n  run(): void { this.handler(1); }\n}",
        )]);
        let names: Vec<&str> = f.funcs.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"Svc.handler"), "field arrow registered: {names:?}");
        assert!(edges_from(&f, "Svc.handler").contains(&"Svc.helper".to_string()), "this.helper in field arrow");
        assert!(edges_from(&f, "Svc.run").contains(&"Svc.handler".to_string()), "this.handler(1) resolves");
    }

    #[test]
    fn field_and_toplevel_initializer_new_populates_instantiated() {
        let f = facts_of(&[(
            "w.ts",
            "class Wallet {\n  money = new Money(1);\n}\nclass Money { constructor(readonly amount: number) {} }\nconst top = new Wallet();",
        )]);
        assert!(f.instantiated.contains("Money"), "field initializer construction");
        assert!(f.instantiated.contains("Wallet"), "top-level initializer construction");
    }

    #[test]
    fn return_type_propagation_types_locals_and_chains() {
        // `const x = zero()` / chain `zero().combine(x)` / `this.make().combine(x)` が
        // 戻り型で解決され、同名メソッドの他型へ Dynamic 漏れしない。
        let f = facts_of(&[(
            "p.ts",
            "class Money { combine(o: Money): Money { return o; } }\nclass Other { combine(o: Money): Money { return o; } }\nfunction zero(): Money { return new Money(); }\nclass App {\n  make(): Money { return zero(); }\n  run(): void {\n    const x = zero();\n    x.combine(x);\n    zero().combine(x);\n    this.make().combine(x);\n  }\n}",
        )]);
        let edges = edges_from(&f, "App.run");
        assert!(edges.contains(&"Money.combine".to_string()), "propagated: {edges:?}");
        assert!(!edges.contains(&"Other.combine".to_string()), "no Dynamic leak: {edges:?}");
    }

    #[test]
    fn bare_call_prefers_same_file_free_fn() {
        // fp-ts 型のモジュール構成: 各ファイルが同名 export（map 等）を持つ。
        // bare 呼びはレキシカルスコープなので同一ファイル定義のみに結ぶ。
        let f = facts_of(&[
            ("Option.ts", "export const map = (x: number): number => x;\nexport const use1 = (): number => map(1);"),
            ("Arr.ts", "export const map = (x: number): number => x;"),
        ]);
        let cid = f.funcs.iter().position(|x| x.name == "use1").unwrap();
        let g = CallGraph::build(&f, Precision::Cha);
        let target_paths: Vec<&std::path::Path> =
            g.edges[cid].iter().map(|&t| f.funcs[t].path.as_path()).collect();
        assert_eq!(target_paths, vec![std::path::Path::new("Option.ts")], "same-file map only");

        // 同一ファイルに定義が無い bare 呼びは全ファイルの同名へ（import 先の過大近似）。
        let f2 = facts_of(&[
            ("A.ts", "export const map = (x: number): number => x;"),
            ("B.ts", "export const map = (x: number): number => x;"),
            ("C.ts", "export const go = (): number => map(1);"),
        ]);
        let cid2 = f2.funcs.iter().position(|x| x.name == "go").unwrap();
        let g2 = CallGraph::build(&f2, Precision::Cha);
        assert_eq!(g2.edges[cid2].len(), 2, "no local def → all same-name candidates");
    }

    #[test]
    fn implicit_self_call_via_this_does_not_leak_to_other_type() {
        // A.run calls this.helper(); B also has helper(). Must connect only A.helper.
        let f = facts_of(&[
            ("A.ts", "class A {\n  run() { this.helper(); }\n  helper() {}\n}"),
            ("B.ts", "class B {\n  helper() {}\n}"),
        ]);
        assert_eq!(edges_from(&f, "A.run"), vec!["A.helper".to_string()]);
    }

    #[test]
    fn field_receiver_resolves_to_declared_type() {
        // D.go calls this.a.foo() where `a: A` (constructor param property);
        // A2 also has foo(). Must resolve to A.foo.
        let f = facts_of(&[(
            "D.ts",
            "class A { foo() {} }\nclass A2 { foo() {} }\nclass D {\n  constructor(private a: A) {}\n  go() { this.a.foo(); }\n}",
        )]);
        assert_eq!(edges_from(&f, "D.go"), vec!["A.foo".to_string()]);
    }

    #[test]
    fn public_field_receiver_resolves() {
        // field via `x: A` public field + this.x.foo().
        let f = facts_of(&[(
            "H.ts",
            "class A { foo() {} }\nclass H {\n  svc: A;\n  run() { this.svc.foo(); }\n}",
        )]);
        assert_eq!(edges_from(&f, "H.run"), vec!["A.foo".to_string()]);
    }

    #[test]
    fn unresolved_receiver_falls_back_to_dynamic_and_rta_prunes() {
        // `this.mk().pong()` — receiver is a call result (type unknown) -> Dynamic.
        // Under RTA only instantiated types' pong survive. B is constructed via
        // new B(); C is not, so C.pong is pruned.
        let f = facts_of(&[(
            "X.ts",
            "class B { pong() {} }\nclass C { pong() {} }\nclass X {\n  mk(): B { return new B(); }\n  run() { this.ext().pong(); }\n}",
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
