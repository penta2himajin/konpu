//! Kotlin 抽出器（tree-sitter-kotlin-ng）。Kotlin の構文を konpu のコア語彙
//! （`ImplInfo` / `MethodInfo` / …）へ正規化する。解析エンジンは言語非依存なので
//! この正規化さえ合えば Rust/Swift と同じ検査が効く。
//!
//! 正規化の要点:
//! - `class` / `data class` / `interface` / `object` は `class_declaration`（名前=identifier）。
//! - メソッドは `function_declaration`。`companion object` 内は関連関数（self 無し）、
//!   それ以外のクラス直下メソッドはインスタンス（≈ `&self`）。
//! - Kotlin の演算子は名前付き関数: `plus`→add / `times`→mul にマップ（非結合 `minus`/`div` は生のまま無視）。
//! - `companion object { val zero: T }` / `fun zero(): T` は単位元 → 引数0・self無し・戻り型 T に正規化。
//!
//! MVP: 推論経路（型・メソッド・演算子・companion 単位元・伝播度）。コメント注釈・law・
//! ignore は共有ディレクティブモジュール化後に対応。tree-sitter-kotlin-ng は明示戻り型付きで
//! クリーンに解析する（戻り型省略の expression body は稀に ERROR 回復になるが抽出は継続）。

use std::path::{Path, PathBuf};

use tree_sitter::Node;

use super::directive::{higher_from, parse_directive, structure_from, Directive};
use super::extract::{
    ignore_reason_from_str, law_from_name, AnalyzedDeclaration, IgnoreInfo, ImplInfo, LawTestInfo,
    MethodInfo, SelfKind,
};
use super::propagation::{TypeInfo, TypeKind};

pub fn extract_all_file(root: Node, source: &str, path: &Path) -> super::FileExtract {
    super::FileExtract {
        decls: extract_declarations(root, source, path),
        impls: extract_impls(root, source),
        free_fns: extract_free_fns(root, source),
        law_tests: extract_law_tests(root, source, path),
        ignores: extract_ignores(root, source, path),
        type_sites: extract_type_sites(root, source, path),
        type_infos: extract_type_infos(root, source),
        ..super::FileExtract::empty()
    }
}

/// 全 konpu ディレクティブコメント（Kotlin は `line_comment`）を
/// (Directive, 直後宣言, 行) で列挙。
fn directives<'a>(root: Node<'a>, source: &'a str) -> Vec<(Directive, Option<Node<'a>>, usize)> {
    let mut comments = Vec::new();
    recurse_nodes(root, "line_comment", &mut comments);
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
        if n.kind() == "line_comment" {
            sib = n.next_named_sibling();
        } else {
            return Some(n);
        }
    }
    None
}

/// `// konpu: monoid(...)` 等 → 直後の型への宣言。
pub fn extract_declarations(root: Node, source: &str, path: &Path) -> Vec<AnalyzedDeclaration> {
    let mut out = Vec::new();
    for (d, decl, line) in directives(root, source) {
        let Some(structure) = structure_from(&d.head) else { continue };
        let Some(decl) = decl else { continue };
        if decl.kind() != "class_declaration" {
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

/// `// konpu: law(...)` → 直後のテスト関数。Kotlin のテストクラスは対象型と別なので
/// enclosing_type=None（全型に一致）。
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
        let test_fn = decl.and_then(|n| {
            (n.kind() == "function_declaration")
                .then(|| first_child_of_kind(n, "identifier").map(|c| text_of(c, source).to_string()))
                .flatten()
        });
        out.push(LawTestInfo { laws, enclosing_type: None, test_fn, path: path.to_path_buf(), line });
    }
    out
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
        let type_name = decl.and_then(|n| (n.kind() == "class_declaration").then(|| type_name(n, source)).flatten());
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

fn is_pascal_case(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_uppercase())
}

/// class_declaration の型名（最初の identifier）。
fn type_name(n: Node, source: &str) -> Option<String> {
    first_child_of_kind(n, "identifier").map(|c| text_of(c, source).to_string())
}

fn has_modifier(n: Node, source: &str, want: &str) -> bool {
    first_child_of_kind(n, "modifiers").is_some_and(|m| {
        let mut cur = m.walk();
        m.children(&mut cur).any(|c| text_of(c, source).contains(want))
    })
}

/// Kotlin 演算子名を konpu の族名へ正規化（plus→add, times→mul）。他は生のまま。
fn normalize_op_name(name: &str) -> String {
    match name {
        "plus" => "add".to_string(),
        "times" => "mul".to_string(),
        other => other.to_string(),
    }
}

/// Kotlin コレクション/nullable を konpu 正準名へ（propagation の非有界判定用）。
fn normalize_type(s: &str) -> String {
    let s = s.trim();
    if let Some(inner) = s.strip_suffix('?') {
        return format!("Option<{}>", normalize_type(inner));
    }
    for (kt, rust) in [
        ("List", "Vec"),
        ("MutableList", "Vec"),
        ("Set", "HashSet"),
        ("MutableSet", "HashSet"),
        ("Map", "HashMap"),
        ("MutableMap", "HashMap"),
        ("Array", "Vec"),
    ] {
        if let Some(rest) = s.strip_prefix(kt) {
            if rest.is_empty() || rest.starts_with('<') {
                return format!("{rust}{rest}");
            }
        }
    }
    s.to_string()
}

pub fn extract_type_sites(root: Node, source: &str, path: &Path) -> Vec<(String, PathBuf, usize)> {
    let mut out = Vec::new();
    recurse(root, &mut |n| {
        if n.kind() == "class_declaration" {
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
        if n.kind() != "class_declaration" {
            return;
        }
        let Some(ty) = type_name(n, source) else { return };
        let Some(body) = first_child_of_kind(n, "class_body") else { return };
        let mut methods = Vec::new();
        collect_members(body, source, false, &mut methods);
        if !methods.is_empty() {
            out.push(ImplInfo { type_name: ty, methods });
        }
    });
    out
}

/// class_body の直接メンバ（メソッド・プロパティ）と companion object を集約。
/// `in_companion` が真なら関連関数扱い（self 無し）。
fn collect_members(body: Node, source: &str, in_companion: bool, methods: &mut Vec<MethodInfo>) {
    let mut cur = body.walk();
    for member in body.children(&mut cur) {
        match member.kind() {
            "function_declaration" => {
                if let Some(m) = parse_function(member, source, in_companion) {
                    methods.push(m);
                }
            }
            "property_declaration" => {
                if let Some(m) = parse_property(member, source, in_companion) {
                    methods.push(m);
                }
            }
            "companion_object" => {
                if let Some(inner) = first_child_of_kind(member, "class_body") {
                    collect_members(inner, source, true, methods);
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
            if let Some(m) = parse_function(child, source, true) {
                out.push(m); // トップレベル関数は self 無し・関連関数扱い。
            }
        }
    }
    out
}

fn parse_function(n: Node, source: &str, assoc: bool) -> Option<MethodInfo> {
    let name_raw = {
        let mut cur = n.walk();
        n.children(&mut cur)
            .find(|c| c.kind() == "identifier")
            .map(|c| text_of(c, source).to_string())?
    };
    let name = normalize_op_name(&name_raw);
    let mut params = Vec::new();
    if let Some(vps) = first_child_of_kind(n, "function_value_parameters") {
        let mut cur = vps.walk();
        for p in vps.children(&mut cur) {
            if p.kind() == "parameter" {
                if let Some(t) = first_child_of_kind(p, "user_type") {
                    params.push(text_of(t, source).trim().to_string());
                }
            }
        }
    }
    // 戻り型: function_value_parameters の後の直下 user_type。
    let ret = {
        let mut cur = n.walk();
        let mut after_params = false;
        let mut r = None;
        for c in n.children(&mut cur) {
            if c.kind() == "function_value_parameters" {
                after_params = true;
            } else if after_params && matches!(c.kind(), "user_type" | "nullable_type" | "function_type") {
                r = Some(text_of(c, source).trim().to_string());
                break;
            }
        }
        r
    };
    Some(MethodInfo {
        name,
        self_param: if assoc { None } else { Some(SelfKind::Ref) },
        params,
        return_type: ret,
        is_assoc_fn: assoc,
    })
}

/// `companion object { val zero: T }` を単位元候補 MethodInfo に正規化。
/// companion 外のインスタンスプロパティは単位元ではない。
fn parse_property(n: Node, source: &str, in_companion: bool) -> Option<MethodInfo> {
    if !in_companion {
        return None;
    }
    let vd = first_child_of_kind(n, "variable_declaration")?;
    let name = first_child_of_kind(vd, "identifier").map(|c| text_of(c, source).to_string())?;
    let ret = property_type(n, vd, source)?;
    Some(MethodInfo {
        name,
        self_param: None,
        params: Vec::new(),
        return_type: Some(ret),
        is_assoc_fn: true,
    })
}

/// プロパティ型: `val x: T` の型注釈、無ければ初期化子の構築型 `= T(...)`。
fn property_type(n: Node, vd: Node, source: &str) -> Option<String> {
    if let Some(ut) = first_child_of_kind(vd, "user_type") {
        return Some(text_of(ut, source).trim().to_string());
    }
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

pub fn extract_type_infos(root: Node, source: &str) -> Vec<TypeInfo> {
    let mut out = Vec::new();
    recurse(root, &mut |n| {
        if n.kind() != "class_declaration" {
            return;
        }
        let Some(name) = type_name(n, source) else { return };
        // enum class → バリアント数。
        if has_modifier(n, source, "enum") {
            let count = count_enum_entries(n, source);
            out.push(TypeInfo { name, kind: TypeKind::Enum, variant_count: count, field_types: Vec::new() });
            return;
        }
        // data class などの primary constructor + class_body の val/var フィールド。
        let mut field_types = Vec::new();
        if let Some(pc) = first_child_of_kind(n, "primary_constructor") {
            if let Some(cps) = first_child_of_kind(pc, "class_parameters") {
                let mut cur = cps.walk();
                for cp in cps.children(&mut cur) {
                    if cp.kind() == "class_parameter" {
                        if let Some(t) = first_child_of_kind(cp, "user_type") {
                            field_types.push(normalize_type(text_of(t, source).trim()));
                        }
                    }
                }
            }
        }
        out.push(TypeInfo { name, kind: TypeKind::Struct, variant_count: 0, field_types });
    });
    out
}

fn count_enum_entries(n: Node, source: &str) -> usize {
    let mut count = 0;
    recurse(n, &mut |m| {
        if m.kind() == "enum_entry" {
            count += 1;
        }
    });
    let _ = source;
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
        let tree = parser::parse_with(src, parser::Language::Kotlin).unwrap();
        extract_impls(tree.root_node(), src)
    }

    fn method<'a>(imp: &'a ImplInfo, name: &str) -> &'a MethodInfo {
        imp.methods.iter().find(|m| m.name == name).unwrap()
    }

    #[test]
    fn operator_plus_maps_to_add_and_companion_zero_is_identity() {
        let impls = impls_of(
            "class Money(val amount: Int) {\n  operator fun plus(o: Money): Money { return this }\n  companion object { fun zero(): Money { return Money(0) } }\n}",
        );
        let m = impls.iter().find(|i| i.type_name == "Money").unwrap();
        let add = method(m, "add"); // plus -> add
        assert_eq!(add.self_param, Some(SelfKind::Ref));
        assert_eq!(add.params, vec!["Money".to_string()]);
        assert_eq!(add.return_type.as_deref(), Some("Money"));
        let zero = method(m, "zero");
        assert_eq!(zero.self_param, None);
        assert!(zero.is_assoc_fn);
        assert_eq!(zero.return_type.as_deref(), Some("Money"));
    }

    #[test]
    fn companion_val_property_is_identity() {
        let impls = impls_of(
            "class V {\n  fun combine(o: V): V { return this }\n  companion object { val zero: V = V() }\n}",
        );
        let zero = method(&impls[0], "zero");
        assert_eq!(zero.self_param, None);
        assert!(zero.params.is_empty());
        assert_eq!(zero.return_type.as_deref(), Some("V"));
    }

    #[test]
    fn comment_annotation_declares_monoid_and_law() {
        let src = "// konpu: monoid(op: combine, identity: zero)\nclass Money(val amount: Int)\nclass T {\n  // konpu: law(associativity)\n  fun testAssoc() { }\n}";
        let tree = parser::parse_with(src, parser::Language::Kotlin).unwrap();
        let decls = extract_declarations(tree.root_node(), src, Path::new("M.kt"));
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].type_name, "Money");
        assert_eq!(decls[0].target_structure, AlgebraicStructure::Monoid);
        assert_eq!(decls[0].operation_name, "combine");
        let laws = extract_law_tests(tree.root_node(), src, Path::new("M.kt"));
        assert_eq!(laws.len(), 1);
        assert_eq!(laws[0].test_fn.as_deref(), Some("testAssoc"));
    }

    #[test]
    fn data_class_field_types_normalized() {
        let tree = parser::parse_with(
            "data class Bag(val items: List<Money>, val n: Int)",
            parser::Language::Kotlin,
        )
        .unwrap();
        let infos = extract_type_infos(tree.root_node(), "data class Bag(val items: List<Money>, val n: Int)");
        let bag = infos.iter().find(|i| i.name == "Bag").unwrap();
        assert!(bag.field_types.contains(&"Vec<Money>".to_string()));
        assert!(bag.field_types.contains(&"Int".to_string()));
    }
}
