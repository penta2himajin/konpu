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
//! - TS には callable-value 規約（Swift callAsFunction / Kotlin invoke）が無いので、
//!   bare 識別子呼び `foo()` は self メソッド → 自由関数 → Dynamic の順で解決する。
//!
//! 精度モデル（Swift/Kotlin と共通）: 型が解決できたら Static、解決できたが index 外なら
//! External（エッジ無し）、本当に未解決なら Dynamic（同名全てに繋ぐ過大近似）。

use std::collections::HashMap;
use std::path::Path;

use tree_sitter::Node;

use konpu_cg::{CallSite, CallTargetKind, Facts, FuncId, ImplEntry, TraitMethod};

use super::{FnSig, MergeConstruction};
use crate::analyze::parser::{self, Language};
use crate::analyze::template::ResolvedConfig;

/// 型宣言ノード（class / abstract class）。interface/enum は呼び出し本体を持たないので対象外。
const CLASS_KINDS: &[&str] = &["class_declaration", "abstract_class_declaration"];

/// TS プロジェクトから Facts を構築する（外部ツール不要）。
/// `config.exclude`（konpu.toml）に一致するファイルは除外する（テスト等をハブ集計から外す）。
/// パスはプロジェクトルート相対で格納する（preserve の `to`/`from` glob は相対パス前提）。
pub fn facts_from_ts_project(path: &Path, config: &ResolvedConfig) -> Facts {
    let sources: Vec<_> = parser::collect_source_files(path)
        .into_iter()
        .filter(|(_, l)| *l == Language::Ts)
        .filter(|(f, _)| !config.is_excluded(f, path))
        .filter_map(|(f, _)| {
            let rel = f.strip_prefix(path).unwrap_or(&f).to_path_buf();
            std::fs::read_to_string(&f).ok().map(|s| (rel, s))
        })
        .collect();
    facts_from_ts_sources(sources)
}

/// (path, source) の集合から Facts を構築する（テスト・in-memory 用）。
pub fn facts_from_ts_sources(sources: Vec<(std::path::PathBuf, String)>) -> Facts {
    let parsed: Vec<(std::path::PathBuf, String, tree_sitter::Tree)> = sources
        .into_iter()
        .filter_map(|(f, src)| {
            let tree = parser::parse_with(&src, Language::Ts)?;
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
    /// 型 -> (フィールド名 -> 基底型名)。受け手が field のとき型解決に使う。
    fields: HashMap<String, HashMap<String, String>>,
}

/// 型文字列の基底名（`A | null`→A ではなく末尾。`Foo<T>`→Foo、`a.b.C`→C）。
fn base_type_name(s: &str) -> String {
    let mut s = s.trim();
    if let Some(i) = s.find('<') {
        s = s[..i].trim();
    }
    s.rsplit(['.', ':']).next().unwrap_or(s).trim().to_string()
}

/// TS プロジェクトの全関数シグネチャ（preserve 検査 B/C 用）。
/// `config.exclude` に一致するファイルは除外する（facts と対象集合を揃える）。
pub fn fn_signatures_ts(path: &Path, config: &ResolvedConfig) -> Vec<FnSig> {
    let mut out = Vec::new();
    for (f, lang) in parser::collect_source_files(path) {
        if lang != Language::Ts || config.is_excluded(&f, path) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&f) else { continue };
        let Some(tree) = parser::parse_with(&src, Language::Ts) else { continue };
        walk_fn_sigs(tree.root_node(), &src, &f, None, &mut out);
    }
    out
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
                collect_base_idents(args, source, &mut refs);
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

/// 式木の基底識別子（`a.x` → "a"、`this`）を重複なく集める。member の末尾（.x）は除く。
fn collect_base_idents(n: Node, source: &str, out: &mut Vec<String>) {
    match n.kind() {
        // member の property `.amount` は参照ではない。object 側だけ辿る。
        "property_identifier" | "shorthand_property_identifier" => return,
        "this" => {
            if !out.iter().any(|s| s == "self") {
                out.push("self".to_string());
            }
            return;
        }
        "identifier" => {
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

fn is_pascal_case(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_uppercase())
}

/// 型宣言/メソッド/関数の名前（`name` フィールド）。
fn type_name(n: Node, source: &str) -> Option<String> {
    n.child_by_field_name("name").map(|c| text_of(c, source).trim().to_string())
}

fn func_name(n: Node, source: &str) -> Option<String> {
    n.child_by_field_name("name").map(|c| text_of(c, source).trim().to_string())
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
                collect_calls(child, source, r, ty.as_deref(), caller, facts);
            }
        }
        return;
    }
    // 関数/メソッドに入ったら caller とローカル変数型を確定して本体を処理。
    // static メソッドは暗黙 self が無いが、TS は静的呼びも `Type.m()` で明示するので
    // enclosing はそのままでよい（bare self 呼びが無いため誤解決しない）。
    if matches!(n.kind(), "method_definition" | "function_declaration") {
        let c = r.ids.get(&n.id()).copied().or(caller);
        let locals = build_locals(n, source);
        if let Some(body) = n.child_by_field_name("body") {
            resolve_body(body, source, r, enclosing, &locals, c, facts);
        }
        return;
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_calls(child, source, r, enclosing, caller, facts);
    }
}

/// 関数本体（アロー関数含む）の呼び出しを解決してエッジ化。ネストした named 関数は
/// それ自身の caller で再入する。`new T(...)` は instantiated を埋める。
fn resolve_body(
    n: Node,
    source: &str,
    r: &Resolver,
    enclosing: Option<&str>,
    locals: &HashMap<String, String>,
    caller: Option<FuncId>,
    facts: &mut Facts,
) {
    match n.kind() {
        "function_declaration" => {
            collect_calls(n, source, r, enclosing, caller, facts);
            return;
        }
        "new_expression" => {
            if let Some(ty) = new_type(n, source) {
                facts.instantiated.insert(ty);
            }
        }
        "call_expression" => {
            resolve_call(n, source, r.index, enclosing, locals, caller, facts);
        }
        _ => {}
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        resolve_body(child, source, r, enclosing, locals, caller, facts);
    }
}

/// 関数のローカル変数の型（引数 + 本体の `const/let x: T` / `= new T(...)`）。
fn build_locals(fn_node: Node, source: &str) -> HashMap<String, String> {
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
        collect_locals(body, source, &mut locals);
    }
    locals
}

fn collect_locals(n: Node, source: &str, out: &mut HashMap<String, String>) {
    if n.kind() == "variable_declarator" {
        if let Some(name) = n.child_by_field_name("name") {
            if name.kind() == "identifier" {
                // ponytail: 型は注釈 `const x: T` か構築 `= new T(...)` からのみ拾う。
                // `const x = foo()`（call 戻り値）や chain `a.b()` はローカルが無型のまま
                // 残り、その受け手 `x.m()` は Dynamic に落ちる（残差の主因）。精密化するなら
                // Index に (型,メソッド)→戻り型 を持たせ戻り型伝播する。
                let ty = n
                    .child_by_field_name("type")
                    .and_then(|ann| type_ann_text(ann, source))
                    .or_else(|| n.child_by_field_name("value").and_then(|v| new_type(v, source)));
                if let Some(t) = ty {
                    out.insert(text_of(name, source).trim().to_string(), base_type_name(&t));
                }
            }
        }
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_locals(child, source, out);
    }
}

/// call_expression を解決してエッジを facts に足す。
fn resolve_call(
    call: Node,
    source: &str,
    index: &Index,
    enclosing: Option<&str>,
    locals: &HashMap<String, String>,
    caller: Option<FuncId>,
    facts: &mut Facts,
) {
    let Some(func) = call.child_by_field_name("function") else { return };

    let resolved: Resolution = match func.kind() {
        // bare 呼び `foo()`: self メソッド → 自由関数 → Dynamic。
        // TS はインスタンスメンバを `this.` で修飾するので、bare は自由関数/インポート
        // が主。PascalCase でも構築は `new`（別ノード）なのでここでは構築扱いしない。
        "identifier" => resolve_bare(index, enclosing, text_of(func, source).trim()),
        // メンバ呼び `recv.method()`。
        "member_expression" => {
            let Some(prop) = func.child_by_field_name("property") else { return };
            let method = text_of(prop, source).trim().to_string();
            let recv = func.child_by_field_name("object");
            match recv.and_then(|o| resolve_receiver(o, source, enclosing, locals, index)) {
                Some(t) => lookup_method(index, &t, &method),
                None => Resolution::Dynamic(method),
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
        _ => None,
    }
}

fn first_named_child(n: Node) -> Option<Node> {
    let mut cur = n.walk();
    n.children(&mut cur).find(|c| c.is_named())
}

enum Resolution {
    /// 型が解決でき、そのメソッドに厳密に結んだ（同名オーバーロードは複数）。
    Targets(Vec<FuncId>),
    /// 型未解決 → 同名メソッド全てに繋ぐ過大近似（偽陰性を出さない）。
    Dynamic(String),
    /// 受け手の型は具体解決できたが index に無い（外部ライブラリ型/継承）→ エッジ無し。
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
            "class B { pong() {} }\nclass C { pong() {} }\nclass X {\n  mk(): B { return new B(); }\n  run() { this.mk().pong(); }\n}",
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
