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

use super::directive::{higher_from, parse_directive, structure_from, Directive};
use super::extract::{
    ignore_reason_from_str, law_from_name, AnalyzedDeclaration, IgnoreInfo, ImplInfo, LawTestInfo,
    MethodInfo, SelfKind, UseStatement,
};
use super::parser::Language;
use super::propagation::{TypeInfo, TypeKind};

/// 型宣言ノードの種別（class / abstract class / interface / enum）。
const TYPE_KINDS: &[&str] = &[
    "class_declaration",
    "abstract_class_declaration",
    "interface_declaration",
    "enum_declaration",
];

pub fn extract_all_file(root: Node, source: &str, path: &Path) -> super::FileExtract {
    super::FileExtract {
        decls: extract_declarations(root, source, path),
        impls: extract_impls(root, source),
        free_fns: extract_free_fns(root, source),
        law_tests: extract_law_tests(root, source, path),
        ignores: extract_ignores(root, source, path),
        uses: extract_use_statements(root, source, path),
        type_sites: extract_type_sites(root, source, path),
        type_infos: extract_type_infos(root, source),
    }
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
        if child.kind() == "function_declaration" {
            if let Some(m) = parse_fn_like(child, source, true) {
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
}
