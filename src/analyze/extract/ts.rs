//! TypeScript 抽出器（tree-sitter-typescript）。TS の構文を konpu のコア語彙
//! （`ImplInfo` / `MethodInfo` / …）へ正規化する。解析エンジンは言語非依存なので
//! この正規化さえ合えば Rust/Swift/Kotlin と同じ検査が効く。
//!
//! 正規化の要点:
//! - 型宣言は `class_declaration` / `abstract_class_declaration` / `interface_declaration`
//!   / `enum_declaration`（名前は `name` フィールド）。
//! - メソッドは `method_definition`（class/abstract）または `method_signature`（interface）。
//!   `static` 修飾子付きは関連関数（self 無し）、それ以外はインスタンス（≈ `&self`）。
//! - TS には演算子オーバーロードが無いので演算子マップは不要。単位元は名前付き
//!   `combine`/`merge` + `static zero()` / `static readonly zero: T`（Kotlin の companion 相当）で表す。
//! - `// konpu:` コメント注釈は共有 `directive.rs` で解析（Rust/Swift/Kotlin と同一契約）。

use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::analyze::directive::{higher_from, parse_directive, structure_from, Directive};
use super::{
    ignore_reason_from_str, law_from_name, AnalyzedDeclaration, IgnoreInfo, ImplInfo, LawTestInfo,
    MethodInfo, SelfKind, UseStatement,
};
use crate::analyze::parser::Language;
use crate::analyze::propagation::{TypeInfo, TypeKind};

/// 型宣言ノードの種別（class / abstract class / interface / enum）。
const TYPE_KINDS: &[&str] = &[
    "class_declaration",
    "abstract_class_declaration",
    "interface_declaration",
    "enum_declaration",
];

pub fn extract_all_file(root: Node, source: &str, path: &Path) -> crate::analyze::FileExtract {
    let mut impls = extract_impls(root, source);
    let mut type_sites = extract_type_sites(root, source, path);
    // 関数型エンコーディングの代数インスタンス（`const M: Monoid<T> = { concat, empty }`）も
    // クラスと同じ ImplInfo/type_site として足す → 既存の推論経路がそのまま Semigroup/Monoid を導く。
    for (impl_info, site) in extract_instances(root, source, path) {
        impls.push(impl_info);
        type_sites.push(site);
    }
    crate::analyze::FileExtract {
        decls: extract_declarations(root, source, path),
        impls,
        free_fns: extract_free_fns(root, source),
        law_tests: extract_law_tests(root, source, path),
        ignores: extract_ignores(root, source, path),
        uses: extract_use_statements(root, source, path),
        type_sites,
        type_infos: extract_type_infos(root, source),
        singletons: Vec::new(),
    }
}

/// 関数型の代数インスタンスを ImplInfo + type_site として抽出する。
///
/// fp-ts 系の型クラスは `interface Semigroup<A> { concat: (x: A, y: A) => A }` を
/// クラスでなく **値**で実装する。3 形態を拾う:
/// - const オブジェクト: `const M: Monoid<number> = { concat, empty }`（fp-ts）。
/// - const factory: `const min = (O): Semigroup<A> => make(...)`（Effect、値が arrow で戻り型注釈）。
/// - function factory: `function getMonoid<A>(): Monoid<A> { ... }`。
///
/// いずれも「宣言名を carrier 型とみなした ImplInfo」に正規化する（op/identity の型は
/// 宣言名に揃える）。トリガは **型注釈/戻り型注釈が `Semigroup`/`Monoid`/`Group`
/// （`Se.Semigroup` 等の修飾可）を名指すこと**（tsc 検証済み＝combine/empty の存在保証。
/// 任意オブジェクトの誤検出を避ける）。body が object literal なら実際の op/identity 名を
/// shape から取り、factory（object が見えない）なら注釈の型クラスから構造を合成する。
fn extract_instances(root: Node, source: &str, path: &Path) -> Vec<(ImplInfo, (String, PathBuf, usize))> {
    let mut out = Vec::new();
    recurse(root, &mut |n| {
        let (name_node, iface, body) = match n.kind() {
            // const X: Iface<..> = <value>  /  const X = (..): Iface<..> => <value>
            "variable_declarator" => {
                let Some(name) = n.child_by_field_name("name") else { return };
                let value = n.child_by_field_name("value");
                // 宣言自体の型注釈、無ければ値が arrow のときその戻り型注釈。
                let iface = n
                    .child_by_field_name("type")
                    .and_then(|ann| algebra_interface(ann, source))
                    .or_else(|| {
                        value
                            .filter(|v| v.kind() == "arrow_function")
                            .and_then(|v| v.child_by_field_name("return_type"))
                            .and_then(|rt| algebra_interface(rt, source))
                    });
                (name, iface, value)
            }
            // function getMonoid<A>(): Iface<..> { ... }
            "function_declaration" => {
                let Some(name) = n.child_by_field_name("name") else { return };
                let iface = n
                    .child_by_field_name("return_type")
                    .and_then(|rt| algebra_interface(rt, source));
                (name, iface, None)
            }
            _ => return,
        };
        if name_node.kind() != "identifier" {
            return;
        }
        let Some(iface) = iface else { return };
        let name = text_of(name_node, source).trim().to_string();
        // body が object literal ならその shape から実 op/identity 名を、そうでなければ
        // （factory 等）型クラス名から構造を合成する。
        let methods = match body.filter(|b| b.kind() == "object") {
            Some(obj) => object_methods(obj, source, &name),
            None => synth_methods(iface, &name),
        };
        if methods.is_empty() {
            return;
        }
        let line = n.start_position().row + 1;
        out.push((
            ImplInfo { type_name: name.clone(), methods },
            (name, path.to_path_buf(), line),
        ));
    });
    out
}

/// 型クラス名から期待される代数構造を合成する（body が opaque な factory 用）。
/// carrier=宣言名。op は必ず、Monoid/Group は単位元、Group は逆元を足す。名前は
/// 族の正準名（infer が族照合で拾える汎用名）。実際の op 名は factory 内部で不可視。
fn synth_methods(iface: &str, inst: &str) -> Vec<MethodInfo> {
    let m = |name: &str, arity: usize| MethodInfo {
        name: name.to_string(),
        self_param: None,
        params: vec![inst.to_string(); arity],
        return_type: Some(inst.to_string()),
        is_assoc_fn: true,
        // factory 内部は不可視なので純粋性は判定できない → false（保守的に confidence を出す）。
        impure: false,
    };
    let mut out = vec![m("combine", 2)];
    if matches!(iface, "Monoid" | "Group") {
        out.push(m("empty", 0));
    }
    if iface == "Group" {
        out.push(m("inverse", 1));
    }
    out
}

/// type_annotation が代数型クラス（`Semigroup`/`Monoid`/`Group`、`Se.Semigroup` 等の
/// 修飾も可）を名指していればその基底名を返す。
fn algebra_interface<'a>(ann: Node, source: &'a str) -> Option<&'a str> {
    let inner = first_named_child(ann)?;
    let base = match inner.kind() {
        "generic_type" => {
            let name = inner.child_by_field_name("name")?;
            match name.kind() {
                "type_identifier" => text_of(name, source),
                // `Se.Semigroup<..>` → nested_type_identifier の name 側。
                "nested_type_identifier" => text_of(name.child_by_field_name("name")?, source),
                _ => return None,
            }
        }
        "type_identifier" => text_of(inner, source),
        _ => return None,
    };
    matches!(base.trim(), "Semigroup" | "Monoid" | "Group").then_some(base.trim())
}

/// オブジェクトリテラルのプロパティを MethodInfo に正規化する（型は inst 名に揃える）。
/// arrow の引数数で op/inverse/identity を作り分ける（族名照合は infer 側に任せる）。
fn object_methods(obj: Node, source: &str, inst: &str) -> Vec<MethodInfo> {
    let mut methods = Vec::new();
    let mut cur = obj.walk();
    for child in obj.children(&mut cur) {
        // `key: value`（pair）と メソッド短縮 `key(...) {}`（method_definition）の両対応。
        // op 本体（arrow/function/method body）が見えれば非純粋シグナルを判定する。
        let (key, arity, impure) = match child.kind() {
            "pair" => {
                let Some(k) = child.child_by_field_name("key") else { continue };
                if k.kind() != "property_identifier" {
                    continue;
                }
                let Some(value) = child.child_by_field_name("value") else { continue };
                let impure = matches!(value.kind(), "arrow_function" | "function_expression")
                    && value
                        .child_by_field_name("body")
                        .is_some_and(|b| body_is_impure(b, source));
                (text_of(k, source).trim().to_string(), value_arity(value), impure)
            }
            "method_definition" => {
                let Some(k) = child.child_by_field_name("name") else { continue };
                let impure = child
                    .child_by_field_name("body")
                    .is_some_and(|b| body_is_impure(b, source));
                (text_of(k, source).trim().to_string(), param_count(child), impure)
            }
            _ => continue,
        };
        methods.push(MethodInfo {
            name: key,
            self_param: None,
            // 引数・戻り型を inst 名に正規化（carrier が builtin/外部型でも推論に乗せる）。
            params: vec![inst.to_string(); arity],
            return_type: Some(inst.to_string()),
            is_assoc_fn: true,
            impure,
        });
    }
    methods
}

/// 演算本体の非純粋シグナルをヒューリスティックに検出する。confidence を
/// withhold するための保守的判定（偽陽性＝warning 過多は許容、偽陰性を避ける）:
/// - 自由変数（本体で束縛されていない識別子）への `++`/`--`/代入 → 外部状態変更
/// - `Math.random()` / `Date.now()` / `performance.now()` → 非決定的
///
/// arrow の本体は式 or ブロック。`body` ノードを渡す。
fn body_is_impure(body: Node, source: &str) -> bool {
    // 本体で束縛される名前（引数は含まれない: body の外側なので、代入対象が
    // 引数なら「本体外束縛」だが引数は純粋な入力。ここでは body 内 let/const/var と
    // catch/関数引数のみ bound とみなし、引数への再代入も稀なので free 扱いで良い）。
    let mut bound: std::collections::HashSet<String> = std::collections::HashSet::new();
    collect_bound_names(body, source, &mut bound);
    let mut impure = false;
    recurse(body, &mut |n| {
        match n.kind() {
            "update_expression" => {
                if let Some(arg) = n.child_by_field_name("argument") {
                    if let Some(name) = base_ident(arg, source) {
                        if !bound.contains(&name) {
                            impure = true;
                        }
                    }
                }
            }
            "assignment_expression" | "augmented_assignment_expression" => {
                if let Some(lhs) = n.child_by_field_name("left") {
                    if let Some(name) = base_ident(lhs, source) {
                        if !bound.contains(&name) {
                            impure = true;
                        }
                    }
                }
            }
            "call_expression" => {
                if let Some(f) = n.child_by_field_name("function") {
                    let t = text_of(f, source);
                    if matches!(t, "Math.random" | "Date.now" | "performance.now") {
                        impure = true;
                    }
                }
            }
            _ => {}
        }
    });
    impure
}

/// `let`/`const`/`var` で宣言された名前を集める（本体内ローカル = 純粋な変更対象）。
fn collect_bound_names(node: Node, source: &str, out: &mut std::collections::HashSet<String>) {
    recurse(node, &mut |n| {
        if n.kind() == "variable_declarator" {
            if let Some(name) = n.child_by_field_name("name") {
                if name.kind() == "identifier" {
                    out.insert(text_of(name, source).to_string());
                }
            }
        }
    });
}

/// 代入/更新の対象の基底識別子名。`x` → x、`x.y`/`x[i]` → x（メンバ変更も
/// 基底オブジェクトの変更）。それ以外は None。
fn base_ident(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_of(node, source).to_string()),
        "member_expression" | "subscript_expression" => {
            base_ident(node.child_by_field_name("object")?, source)
        }
        _ => None,
    }
}

/// プロパティ値ノードから演算の項数を推定する。
/// - 関数リテラル（arrow / function）→ 実際の仮引数数（正確）。
/// - データリテラル（数値/文字列/配列/オブジェクト等）→ 0（単位元候補）。
/// - 参照（`SemigroupSum.concat` / 識別子 / 呼び出し）→ 関数エイリアスとみなし 2（二項演算）。
///   fp-ts の Monoid は `concat: SemigroupX.concat` と別インスタンスの演算を借用するため。
///   非族名プロパティを 2 と誤っても infer は族名でしか op を拾わないので無害。
fn value_arity(value: Node) -> usize {
    match value.kind() {
        "arrow_function" | "function_expression" => param_count(value),
        // データリテラル = 0 引数（単位元）。
        "number" | "string" | "template_string" | "true" | "false" | "null" | "array"
        | "object" | "unary_expression" | "regex" => 0,
        // 参照・呼び出し等 = 関数エイリアス（二項演算の借用）とみなす。
        _ => 2,
    }
}

/// arrow/function/method の仮引数の数（required + optional）。
fn param_count(fn_node: Node) -> usize {
    let Some(fps) = fn_node.child_by_field_name("parameters") else { return 0 };
    let mut cur = fps.walk();
    fps.children(&mut cur)
        .filter(|p| matches!(p.kind(), "required_parameter" | "optional_parameter"))
        .count()
}

/// `import ... from "module"` を UseStatement として集める。imported_path = モジュール指定子。
/// 境界検査は import 指定子を `from_modules` と前方一致で照合（Swift/Kotlin と同経路）。
pub fn extract_use_statements(root: Node, source: &str, path: &Path) -> Vec<UseStatement> {
    let mut out = Vec::new();
    recurse(root, &mut |n| {
        if n.kind() == "import_statement" {
            if let Some(src) = n.child_by_field_name("source") {
                let spec = string_literal_text(src, source);
                if !spec.is_empty() {
                    out.push(UseStatement {
                        path: path.to_path_buf(),
                        imported_path: spec,
                        line: n.start_position().row + 1,
                        language: Language::Ts,
                    });
                }
            }
        }
    });
    out
}

/// `string` ノード（`"..."` / `'...'`）の中身を引用符抜きで取り出す。
fn string_literal_text(n: Node, source: &str) -> String {
    if let Some(frag) = first_child_of_kind(n, "string_fragment") {
        return text_of(frag, source).to_string();
    }
    text_of(n, source).trim_matches(['"', '\'']).to_string()
}

/// 全 konpu ディレクティブコメント（TS は `comment`）を (Directive, 直後宣言, 行) で列挙。
fn directives<'a>(root: Node<'a>, source: &'a str) -> Vec<(Directive, Option<Node<'a>>, usize)> {
    let mut comments = Vec::new();
    recurse_nodes(root, "comment", &mut comments);
    comments
        .into_iter()
        .filter_map(|n| {
            parse_directive(text_of(n, source)).map(|d| (d, following_decl(n), n.start_position().row + 1))
        })
        .collect()
}

fn recurse_nodes<'a>(n: Node<'a>, kind: &str, out: &mut Vec<Node<'a>>) {
    if n.kind() == kind {
        out.push(n);
    }
    let mut cur = n.walk();
    for c in n.children(&mut cur) {
        recurse_nodes(c, kind, out);
    }
}

fn following_decl(comment: Node) -> Option<Node> {
    let mut sib = comment.next_named_sibling();
    while let Some(n) = sib {
        if n.kind() == "comment" {
            sib = n.next_named_sibling();
        } else {
            return Some(unwrap_export(n));
        }
    }
    None
}

/// `export class Foo {}` / `export default class` は宣言を `export_statement` で
/// ラップする。中の宣言ノードへ剥がす（無ければそのまま）。
fn unwrap_export(n: Node) -> Node {
    if n.kind() == "export_statement" {
        let mut cur = n.walk();
        if let Some(inner) = n.children(&mut cur).find(|c| {
            TYPE_KINDS.contains(&c.kind()) || c.kind() == "function_declaration"
        }) {
            return inner;
        }
    }
    n
}

/// `// konpu: monoid(...)` 等 → 直後の型への宣言。
pub fn extract_declarations(root: Node, source: &str, path: &Path) -> Vec<AnalyzedDeclaration> {
    let mut out = Vec::new();
    for (d, decl, line) in directives(root, source) {
        let Some(structure) = structure_from(&d.head) else { continue };
        let Some(decl) = decl else { continue };
        if !TYPE_KINDS.contains(&decl.kind()) {
            continue;
        }
        let Some(type_name) = type_name(decl, source) else { continue };
        out.push(AnalyzedDeclaration {
            target_structure: structure,
            higher_kinded: d.kwargs.get("higher").and_then(|v| higher_from(v)),
            type_name,
            operation_name: d.kwargs.get("op").cloned().unwrap_or_default(),
            identity_name: d.kwargs.get("identity").cloned(),
            inverse_name: d.kwargs.get("inverse").cloned(),
            path: path.to_path_buf(),
            line,
            propagation: None,
        });
    }
    out
}

/// `// konpu: law(...)` → 直後のテスト。TS のテストは対象型と別スコープなので
/// enclosing_type=None（全型に一致）。test_fn は `function foo()` 名 or `test("name", ...)` の文字列。
pub fn extract_law_tests(root: Node, source: &str, path: &Path) -> Vec<LawTestInfo> {
    let mut out = Vec::new();
    for (d, decl, line) in directives(root, source) {
        if d.head != "law" {
            continue;
        }
        let laws: Vec<_> = d.positional.iter().filter_map(|s| law_from_name(s)).collect();
        if laws.is_empty() {
            continue;
        }
        let test_fn = decl.and_then(|n| test_name(n, source));
        out.push(LawTestInfo { laws, enclosing_type: None, test_fn, path: path.to_path_buf(), line });
    }
    out
}

/// テスト名を取り出す。`function foo() {}` → "foo"、`test("name", ...)` / `it("name", ...)` → "name"。
fn test_name(n: Node, source: &str) -> Option<String> {
    if n.kind() == "function_declaration" {
        return n.child_by_field_name("name").map(|c| text_of(c, source).to_string());
    }
    // expression_statement > call_expression `test("name", ...)`
    let call = if n.kind() == "call_expression" {
        Some(n)
    } else {
        first_child_of_kind(n, "call_expression")
    }?;
    let fn_name = call.child_by_field_name("function").map(|f| text_of(f, source))?;
    if fn_name != "test" && fn_name != "it" {
        return None;
    }
    let args = call.child_by_field_name("arguments")?;
    let s = first_child_of_kind(args, "string")?;
    Some(string_literal_text(s, source))
}

/// `// konpu: ignore(reason: ..., note: "...")` → IgnoreInfo。
pub fn extract_ignores(root: Node, source: &str, path: &Path) -> Vec<IgnoreInfo> {
    let mut out = Vec::new();
    for (d, decl, line) in directives(root, source) {
        if d.head != "ignore" {
            continue;
        }
        let Some(reason) = d.kwargs.get("reason").and_then(|r| ignore_reason_from_str(r)) else {
            continue;
        };
        let type_name = decl.and_then(|n| TYPE_KINDS.contains(&n.kind()).then(|| type_name(n, source)).flatten());
        out.push(IgnoreInfo { reason, note: d.kwargs.get("note").cloned(), type_name, path: path.to_path_buf(), line });
    }
    out
}

fn first_named_child(n: Node) -> Option<Node> {
    let mut cur = n.walk();
    n.children(&mut cur).find(|c| c.is_named())
}

fn text_of<'a>(n: Node, source: &'a str) -> &'a str {
    n.utf8_text(source.as_bytes()).unwrap_or("")
}

fn first_child_of_kind<'a>(n: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cur = n.walk();
    n.children(&mut cur).find(|c| c.kind() == kind)
}

/// 型宣言の名前（`name` フィールド）。class/interface は type_identifier、enum は identifier。
fn type_name(n: Node, source: &str) -> Option<String> {
    n.child_by_field_name("name").map(|c| text_of(c, source).to_string())
}

/// メンバに `static` キーワードが付いているか。
fn has_static(n: Node, source: &str) -> bool {
    let mut cur = n.walk();
    n.children(&mut cur).any(|c| !c.is_named() && text_of(c, source) == "static")
}

/// `type_annotation`（`: T`）から型テキストを取り出す。無ければ None。
fn type_ann_text(ann: Node, source: &str) -> Option<String> {
    let mut cur = ann.walk();
    ann.children(&mut cur)
        .find(|c| c.is_named())
        .map(|t| text_of(t, source).trim().to_string())
}

/// TS コレクション/配列を konpu 正準名へ（propagation の非有界判定用）。
fn normalize_type(s: &str) -> String {
    let s = s.trim().trim_start_matches("readonly ").trim();
    if let Some(inner) = s.strip_suffix("[]") {
        return format!("Vec<{}>", normalize_type(inner));
    }
    for (ts, rust) in [
        ("Array", "Vec"),
        ("ReadonlyArray", "Vec"),
        ("Set", "HashSet"),
        ("ReadonlySet", "HashSet"),
        ("Map", "HashMap"),
        ("ReadonlyMap", "HashMap"),
    ] {
        if let Some(rest) = s.strip_prefix(ts) {
            if rest.starts_with('<') {
                return format!("{rust}{rest}");
            }
        }
    }
    s.to_string()
}

pub fn extract_type_sites(root: Node, source: &str, path: &Path) -> Vec<(String, PathBuf, usize)> {
    let mut out = Vec::new();
    recurse(root, &mut |n| {
        if TYPE_KINDS.contains(&n.kind()) {
            if let Some(name) = type_name(n, source) {
                out.push((name, path.to_path_buf(), n.start_position().row + 1));
            }
        }
    });
    out
}

pub fn extract_impls(root: Node, source: &str) -> Vec<ImplInfo> {
    let mut out = Vec::new();
    recurse(root, &mut |n| {
        if !TYPE_KINDS.contains(&n.kind()) {
            return;
        }
        let Some(ty) = type_name(n, source) else { return };
        let Some(body) = n.child_by_field_name("body") else { return };
        let mut methods = Vec::new();
        collect_members(body, source, &mut methods);
        if !methods.is_empty() {
            out.push(ImplInfo { type_name: ty, methods });
        }
    });
    out
}

/// 型本体（class_body / interface_body）の直接メンバを集約。
/// `static` 付きメソッド/フィールドは関連関数（self 無し）。
fn collect_members(body: Node, source: &str, methods: &mut Vec<MethodInfo>) {
    let mut cur = body.walk();
    for member in body.children(&mut cur) {
        match member.kind() {
            "method_definition" | "method_signature" => {
                if let Some(m) = parse_method(member, source) {
                    methods.push(m);
                }
            }
            "public_field_definition" => {
                if let Some(m) = parse_field(member, source) {
                    methods.push(m);
                }
            }
            _ => {}
        }
    }
}

pub fn extract_free_fns(root: Node, source: &str) -> Vec<MethodInfo> {
    let mut out = Vec::new();
    let mut cur = root.walk();
    for child in root.children(&mut cur) {
        // `export function f()` は export_statement にラップされる。剥がして拾う。
        let node = unwrap_export(child);
        if node.kind() == "function_declaration" {
            if let Some(m) = parse_fn_like(node, source, true) {
                out.push(m); // トップレベル関数は self 無し・関連関数扱い。
            }
        }
    }
    out
}

/// class/interface のメソッド。`static` なら関連関数、それ以外はインスタンス(&self)。
fn parse_method(n: Node, source: &str) -> Option<MethodInfo> {
    let assoc = has_static(n, source);
    parse_fn_like(n, source, assoc)
}

/// method_definition / method_signature / function_declaration から MethodInfo を作る。
fn parse_fn_like(n: Node, source: &str, assoc: bool) -> Option<MethodInfo> {
    let name = n.child_by_field_name("name").map(|c| text_of(c, source).to_string())?;
    let mut params = Vec::new();
    if let Some(fps) = n.child_by_field_name("parameters") {
        let mut cur = fps.walk();
        for p in fps.children(&mut cur) {
            if matches!(p.kind(), "required_parameter" | "optional_parameter") {
                if let Some(ann) = p.child_by_field_name("type") {
                    if let Some(t) = type_ann_text(ann, source) {
                        params.push(t);
                    }
                }
            }
        }
    }
    let ret = n
        .child_by_field_name("return_type")
        .and_then(|ann| type_ann_text(ann, source));
    Some(MethodInfo {
        name,
        self_param: if assoc { None } else { Some(SelfKind::Ref) },
        params,
        return_type: ret,
        is_assoc_fn: assoc,
        impure: n
            .child_by_field_name("body")
            .is_some_and(|b| body_is_impure(b, source)),
    })
}

/// `static readonly zero: T = ...` を単位元候補 MethodInfo に正規化。
/// static でないインスタンスフィールドは単位元ではない。
fn parse_field(n: Node, source: &str) -> Option<MethodInfo> {
    if !has_static(n, source) {
        return None;
    }
    let name = n.child_by_field_name("name").map(|c| text_of(c, source).to_string())?;
    let ret = n.child_by_field_name("type").and_then(|ann| type_ann_text(ann, source))?;
    Some(MethodInfo {
        name,
        self_param: None,
        params: Vec::new(),
        return_type: Some(ret),
        is_assoc_fn: true,
        impure: false,
    })
}

pub fn extract_type_infos(root: Node, source: &str) -> Vec<TypeInfo> {
    let mut out = Vec::new();
    recurse(root, &mut |n| match n.kind() {
        "enum_declaration" => {
            let Some(name) = type_name(n, source) else { return };
            let count = count_enum_members(n);
            out.push(TypeInfo { name, kind: TypeKind::Enum, variant_count: count, field_types: Vec::new() });
        }
        "class_declaration" | "abstract_class_declaration" => {
            let Some(name) = type_name(n, source) else { return };
            let mut field_types = Vec::new();
            if let Some(body) = n.child_by_field_name("body") {
                let mut cur = body.walk();
                for member in body.children(&mut cur) {
                    if member.kind() == "public_field_definition" && !has_static(member, source) {
                        if let Some(ann) = member.child_by_field_name("type") {
                            if let Some(t) = type_ann_text(ann, source) {
                                field_types.push(normalize_type(&t));
                            }
                        }
                    }
                }
            }
            out.push(TypeInfo { name, kind: TypeKind::Struct, variant_count: 0, field_types });
        }
        _ => {}
    });
    out
}

/// enum_body 内のメンバ数（`enum_assignment` と bare `property_identifier`）。
fn count_enum_members(n: Node) -> usize {
    let mut count = 0;
    recurse(n, &mut |m| {
        if matches!(m.kind(), "enum_assignment" | "property_identifier") {
            count += 1;
        }
    });
    count
}

fn recurse<F: FnMut(Node)>(n: Node, f: &mut F) {
    f(n);
    let mut cur = n.walk();
    for c in n.children(&mut cur) {
        recurse(c, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::parser;
    use crate::domain::konpu::AlgebraicStructure;

    fn impls_of(src: &str) -> Vec<ImplInfo> {
        let tree = parser::parse_with(src, parser::Language::Ts).unwrap();
        extract_impls(tree.root_node(), src)
    }

    fn method<'a>(imp: &'a ImplInfo, name: &str) -> &'a MethodInfo {
        imp.methods.iter().find(|m| m.name == name).unwrap()
    }

    #[test]
    fn instance_method_and_static_zero_identity() {
        let impls = impls_of(
            "class Money {\n  amount: number = 0;\n  combine(o: Money): Money { return this; }\n  static zero(): Money { return new Money(); }\n}",
        );
        let m = impls.iter().find(|i| i.type_name == "Money").unwrap();
        let add = method(m, "combine");
        assert_eq!(add.self_param, Some(SelfKind::Ref));
        assert_eq!(add.params, vec!["Money".to_string()]);
        assert_eq!(add.return_type.as_deref(), Some("Money"));
        let zero = method(m, "zero");
        assert_eq!(zero.self_param, None);
        assert!(zero.is_assoc_fn);
        assert_eq!(zero.return_type.as_deref(), Some("Money"));
    }

    #[test]
    fn static_readonly_field_is_identity() {
        let impls = impls_of(
            "class V {\n  merge(o: V): V { return this; }\n  static readonly zero: V = new V();\n}",
        );
        let zero = method(&impls[0], "zero");
        assert_eq!(zero.self_param, None);
        assert!(zero.params.is_empty());
        assert_eq!(zero.return_type.as_deref(), Some("V"));
    }

    #[test]
    fn instance_field_is_not_identity() {
        let impls = impls_of("class V {\n  merge(o: V): V { return this; }\n  x: number = 0;\n}");
        // インスタンスフィールド x は単位元候補にならない。
        assert!(impls[0].methods.iter().all(|m| m.name != "x"));
    }

    #[test]
    fn comment_annotation_declares_monoid_and_law() {
        let src = "// konpu: monoid(op: combine, identity: zero)\nclass Money {}\n// konpu: law(associativity)\ntest(\"assoc\", () => {});";
        let tree = parser::parse_with(src, parser::Language::Ts).unwrap();
        let decls = extract_declarations(tree.root_node(), src, Path::new("M.ts"));
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].type_name, "Money");
        assert_eq!(decls[0].target_structure, AlgebraicStructure::Monoid);
        assert_eq!(decls[0].operation_name, "combine");
        let laws = extract_law_tests(tree.root_node(), src, Path::new("M.ts"));
        assert_eq!(laws.len(), 1);
        assert_eq!(laws[0].test_fn.as_deref(), Some("assoc"));
    }

    #[test]
    fn directive_targets_exported_class() {
        // `export class` は export_statement でラップされる — 剥がして型に届くこと。
        let src = "// konpu: monoid(op: combine, identity: zero)\nexport class Money {}";
        let tree = parser::parse_with(src, parser::Language::Ts).unwrap();
        let decls = extract_declarations(tree.root_node(), src, Path::new("M.ts"));
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].type_name, "Money");
    }

    #[test]
    fn class_field_types_normalized() {
        let src = "class Bag {\n  items: Money[] = [];\n  n: number = 0;\n}";
        let tree = parser::parse_with(src, parser::Language::Ts).unwrap();
        let infos = extract_type_infos(tree.root_node(), src);
        let bag = infos.iter().find(|i| i.name == "Bag").unwrap();
        assert!(bag.field_types.contains(&"Vec<Money>".to_string()));
        assert!(bag.field_types.contains(&"number".to_string()));
    }

    #[test]
    fn enum_variant_count() {
        let src = "enum Color { Red, Green, Blue }";
        let tree = parser::parse_with(src, parser::Language::Ts).unwrap();
        let infos = extract_type_infos(tree.root_node(), src);
        let c = infos.iter().find(|i| i.name == "Color").unwrap();
        assert_eq!(c.kind, TypeKind::Enum);
        assert_eq!(c.variant_count, 3);
    }

    #[test]
    fn import_specifier_collected() {
        let src = "import { Foo } from \"./domain/money\";";
        let tree = parser::parse_with(src, parser::Language::Ts).unwrap();
        let uses = extract_use_statements(tree.root_node(), src, Path::new("infra.ts"));
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].imported_path, "./domain/money");
        assert_eq!(uses[0].language, Language::Ts);
    }

    fn instances_of(src: &str) -> Vec<ImplInfo> {
        let tree = parser::parse_with(src, parser::Language::Ts).unwrap();
        extract_instances(tree.root_node(), src, Path::new("m.ts"))
            .into_iter()
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn functional_monoid_instance_becomes_impl() {
        // fp-ts 風: interface + const インスタンス。const 名を carrier とみなし、
        // concat=二項演算(2引数)、empty=単位元(0引数) を inst 名の型に正規化。
        let src = "export const monoidSum: Monoid<number> = { concat: (x, y) => x + y, empty: 0 };";
        let impls = instances_of(src);
        let m = impls.iter().find(|i| i.type_name == "monoidSum").unwrap();
        let concat = m.methods.iter().find(|m| m.name == "concat").unwrap();
        assert_eq!(concat.self_param, None);
        assert!(concat.is_assoc_fn);
        assert_eq!(concat.params, vec!["monoidSum".to_string(), "monoidSum".to_string()]);
        assert_eq!(concat.return_type.as_deref(), Some("monoidSum"));
        let empty = m.methods.iter().find(|m| m.name == "empty").unwrap();
        assert!(empty.params.is_empty());
        assert_eq!(empty.return_type.as_deref(), Some("monoidSum"));
    }

    #[test]
    fn functional_instance_infers_monoid_end_to_end() {
        use crate::analyze::infer::infer_declarations;
        let src = "export const monoidSum: Monoid<number> = { concat: (x, y) => x + y, empty: 0 };";
        let tree = parser::parse_with(src, parser::Language::Ts).unwrap();
        let root = tree.root_node();
        let impls = extract_instances(root, src, Path::new("m.ts"));
        let (impl_infos, sites): (Vec<_>, Vec<_>) = impls.into_iter().unzip();
        let type_sites: std::collections::HashMap<String, (std::path::PathBuf, usize)> =
            sites.into_iter().map(|(n, p, l)| (n, (p, l))).collect();
        let decls = infer_declarations(&impl_infos, &[], &type_sites, &std::collections::HashSet::new());
        let d = decls.iter().find(|d| d.type_name == "monoidSum").unwrap();
        assert_eq!(d.target_structure, AlgebraicStructure::Monoid);
        assert_eq!(d.operation_name, "concat");
        assert_eq!(d.identity_name.as_deref(), Some("empty"));
    }

    #[test]
    fn semigroup_instance_without_empty_is_semigroup() {
        let src = "const S: Se.Semigroup<string> = { concat: (x, y) => x + y };";
        let impls = instances_of(src);
        assert!(impls.iter().any(|i| i.type_name == "S")); // 修飾 `Se.Semigroup` も拾う
    }

    #[test]
    fn aliased_concat_is_still_binary_op() {
        // fp-ts の Monoid は op を別インスタンスから借用する: `concat: SemigroupSum.concat`。
        // 参照値でも二項演算とみなし、`empty: 0`（リテラル）を単位元にして Monoid に届く。
        let src = "export const MonoidSum: Monoid<number> = { concat: SemigroupSum.concat, empty: 0 };";
        let m = &instances_of(src)[0];
        let concat = m.methods.iter().find(|m| m.name == "concat").unwrap();
        assert_eq!(concat.params.len(), 2); // 参照でも 2 引数
        let empty = m.methods.iter().find(|m| m.name == "empty").unwrap();
        assert_eq!(empty.params.len(), 0); // リテラルは 0 引数
    }

    #[test]
    fn plain_object_without_algebra_annotation_ignored() {
        // 型注釈が代数型クラスを名指さないオブジェクトは拾わない（誤検出回避）。
        let src = "const cfg = { concat: (x, y) => x + y, empty: 0 };";
        assert!(instances_of(src).is_empty());
    }

    #[test]
    fn const_arrow_factory_synthesizes_from_return_type() {
        // Effect 風: `const min = (O): Semigroup<A> => make(...)`。body は object でないので
        // 戻り型注釈の型クラスから構造を合成（combine のみ → Semigroup）。
        let src = "export const min = <A>(O: Order<A>): Semigroup<A> => make((x, y) => x);";
        let m = &instances_of(src)[0];
        assert_eq!(m.type_name, "min");
        let combine = m.methods.iter().find(|m| m.name == "combine").unwrap();
        assert_eq!(combine.params.len(), 2);
        assert!(m.methods.iter().all(|m| m.name != "empty")); // Semigroup: 単位元無し
    }

    #[test]
    fn function_factory_monoid_synthesizes_op_and_identity() {
        let src = "export function getMonoid<A>(): Monoid<A> { return make(); }";
        let m = &instances_of(src)[0];
        assert_eq!(m.type_name, "getMonoid");
        assert!(m.methods.iter().any(|m| m.name == "combine" && m.params.len() == 2));
        assert!(m.methods.iter().any(|m| m.name == "empty" && m.params.is_empty()));
    }

    #[test]
    fn non_algebra_return_type_ignored() {
        let src = "export const mk = <A>(): Order<A> => makeOrder();\nfunction f(): number { return 0; }";
        assert!(instances_of(src).is_empty());
    }

    #[test]
    fn impure_concat_flagged_impure() {
        // concat mutates a module-level free variable — not associative-safe.
        let src = "let counter = 0;\nexport const M: Monoid<number> = { concat: (x: number, y: number): number => x + y + counter++, empty: 0 };";
        let m = &instances_of(src)[0];
        let concat = m.methods.iter().find(|m| m.name == "concat").unwrap();
        assert!(concat.impure);
    }

    #[test]
    fn nondeterministic_concat_flagged_impure() {
        let src = "export const M: Monoid<number> = { concat: (x: number, y: number): number => x + y + Math.random(), empty: 0 };";
        let m = &instances_of(src)[0];
        assert!(m.methods.iter().find(|m| m.name == "concat").unwrap().impure);
    }

    #[test]
    fn pure_concat_not_flagged() {
        let src = "export const M: Monoid<number> = { concat: (x: number, y: number): number => x + y, empty: 0 };";
        let m = &instances_of(src)[0];
        assert!(!m.methods.iter().find(|m| m.name == "concat").unwrap().impure);
    }

    #[test]
    fn local_mutation_is_pure() {
        // Mutating a locally-declared accumulator is pure — must NOT be flagged.
        let src = "export const M: Monoid<number> = { concat: (x: number, y: number): number => { let acc = x; acc += y; return acc; }, empty: 0 };";
        let m = &instances_of(src)[0];
        assert!(!m.methods.iter().find(|m| m.name == "concat").unwrap().impure);
    }
}
