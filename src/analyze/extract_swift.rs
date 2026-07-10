//! Swift 抽出器（tree-sitter-swift）。Swift の構文を konpu のコア語彙
//! （`ImplInfo` / `MethodInfo` / …）へ正規化する。下流の check/infer/template は
//! 言語非依存なので、この正規化さえ合えば Rust と同じ検査がそのまま効く。
//!
//! 正規化の要点:
//! - `struct`/`class`/`enum`/`extension` はいずれも `class_declaration`（先頭トークンで判別）。
//! - メソッドは `function_declaration`。Swift に self ノードは無く、`static` 修飾子の
//!   有無でインスタンス（≈ `&self`）／型メソッド（≈ 関連関数）を分ける。`mutating` は `&mut self`。
//! - `static let zero: T`（`property_declaration`）は Swift 慣用の単位元。引数0・self無し・
//!   戻り型 T の `MethodInfo` に正規化して `is_identity` に拾わせる。
//!
//! MVP: 推論経路（型・メソッド・静的単位元・自由関数）。コメント注釈・law test・
//! ignore・伝播度・演算子メソッドは後続で追加。

use std::path::{Path, PathBuf};

use tree_sitter::Node;

use super::extract::{ImplInfo, MethodInfo, SelfKind};

/// Swift ファイル 1 つからバンドルを返す（言語ディスパッチ用）。
pub fn extract_all_file(root: Node, source: &str, path: &Path) -> super::FileExtract {
    let mut fx = super::FileExtract::empty();
    fx.impls = extract_impls(root, source);
    fx.free_fns = extract_free_fns(root, source);
    fx.type_sites = extract_type_sites(root, source, path);
    fx
}

fn text_of<'a>(n: Node, source: &'a str) -> &'a str {
    n.utf8_text(source.as_bytes()).unwrap_or("")
}

fn first_child_of_kind<'a>(n: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cur = n.walk();
    n.children(&mut cur).find(|c| c.kind() == kind)
}

/// `class_declaration` の種別（先頭の非named トークン: struct/class/enum/extension）。
fn decl_keyword(n: Node, source: &str) -> Option<String> {
    let mut cur = n.walk();
    for c in n.children(&mut cur) {
        if !c.is_named() {
            let t = text_of(c, source).trim();
            if matches!(t, "struct" | "class" | "enum" | "extension") {
                return Some(t.to_string());
            }
        }
    }
    None
}

/// `class_declaration` の対象型名。struct/class/enum は `type_identifier`、
/// extension は `user_type > type_identifier`。
fn decl_type_name(n: Node, source: &str, keyword: &str) -> Option<String> {
    if keyword == "extension" {
        let ut = first_child_of_kind(n, "user_type")?;
        let ti = first_child_of_kind(ut, "type_identifier")?;
        Some(text_of(ti, source).to_string())
    } else {
        let ti = first_child_of_kind(n, "type_identifier")?;
        Some(text_of(ti, source).to_string())
    }
}

/// 型宣言サイト（struct/class/enum のみ、extension は除く）。推論の診断アンカー用。
pub fn extract_type_sites(root: Node, source: &str, path: &Path) -> Vec<(String, PathBuf, usize)> {
    let mut out = Vec::new();
    recurse(root, &mut |n| {
        if n.kind() == "class_declaration" {
            if let Some(kw) = decl_keyword(n, source) {
                if kw != "extension" {
                    if let Some(name) = decl_type_name(n, source, &kw) {
                        out.push((name, path.to_path_buf(), n.start_position().row + 1));
                    }
                }
            }
        }
    });
    out
}

/// 型ごとの impl 情報。struct/class/enum 本体と extension 本体のメソッド＋静的単位元。
pub fn extract_impls(root: Node, source: &str) -> Vec<ImplInfo> {
    let mut out = Vec::new();
    recurse(root, &mut |n| {
        if n.kind() != "class_declaration" {
            return;
        }
        let Some(kw) = decl_keyword(n, source) else { return };
        let Some(type_name) = decl_type_name(n, source, &kw) else { return };
        let body = first_child_of_kind(n, "class_body")
            .or_else(|| first_child_of_kind(n, "enum_class_body"));
        let Some(body) = body else { return };
        let mut methods = Vec::new();
        let mut bcur = body.walk();
        for member in body.children(&mut bcur) {
            match member.kind() {
                "function_declaration" => {
                    if let Some(m) = parse_function(member, source) {
                        methods.push(m);
                    }
                }
                "property_declaration" => {
                    if let Some(m) = parse_static_property(member, source) {
                        methods.push(m);
                    }
                }
                _ => {}
            }
        }
        if !methods.is_empty() {
            out.push(ImplInfo { type_name, methods });
        }
    });
    out
}

/// トップレベル（型/extension の外）の自由関数。戻り型で型に帰属させ単位元候補にする。
pub fn extract_free_fns(root: Node, source: &str) -> Vec<MethodInfo> {
    let mut out = Vec::new();
    let mut cur = root.walk();
    for child in root.children(&mut cur) {
        if child.kind() == "function_declaration" {
            if let Some(mut m) = parse_function(child, source) {
                // トップレベル関数に self は無い（型内メソッドと違い、修飾子だけでは
                // 区別できないのでここで正す）。関連関数扱い＝Rust の自由関数と同じ。
                m.self_param = None;
                m.is_assoc_fn = true;
                out.push(m);
            }
        }
    }
    out
}

/// `modifiers` に指定の修飾子があるか。
fn has_modifier(fn_node: Node, source: &str, want: &str) -> bool {
    let Some(mods) = first_child_of_kind(fn_node, "modifiers") else {
        return false;
    };
    let mut cur = mods.walk();
    mods.children(&mut cur)
        .any(|m| text_of(m, source).trim() == want)
}

/// `function_declaration` → `MethodInfo`。
fn parse_function(n: Node, source: &str) -> Option<MethodInfo> {
    // 名前: modifiers/parameter/型の外にある直下の simple_identifier。
    let name = {
        let mut cur = n.walk();
        n.children(&mut cur)
            .find(|c| c.kind() == "simple_identifier")
            .map(|c| text_of(c, source).to_string())?
    };
    let is_static = has_modifier(n, source, "static") || has_modifier(n, source, "class");
    let is_mutating = has_modifier(n, source, "mutating");
    let self_param = if is_static {
        None
    } else if is_mutating {
        Some(SelfKind::MutRef)
    } else {
        Some(SelfKind::Ref)
    };
    // 引数の型（各 parameter の型注釈）。
    let mut params = Vec::new();
    let mut ret: Option<String> = None;
    let mut cur = n.walk();
    for c in n.children(&mut cur) {
        match c.kind() {
            "parameter" => {
                if let Some(t) = param_type(c, source) {
                    params.push(t);
                }
            }
            // 直下の型ノード＝戻り型（`->` の後）。引数の型は parameter 内なので混ざらない。
            "user_type" | "optional_type" | "tuple_type" | "array_type" | "dictionary_type" => {
                ret = Some(text_of(c, source).trim().to_string());
            }
            _ => {}
        }
    }
    Some(MethodInfo {
        name,
        self_param,
        params,
        return_type: ret,
        is_assoc_fn: is_static,
    })
}

/// `parameter` の型テキスト（型注釈の型ノード）。
fn param_type(n: Node, source: &str) -> Option<String> {
    // parameter は `label name : Type`。型は user_type 等の型ノード。
    let mut cur = n.walk();
    for c in n.children(&mut cur) {
        match c.kind() {
            "user_type" | "optional_type" | "tuple_type" | "array_type" | "dictionary_type" => {
                return Some(text_of(c, source).trim().to_string());
            }
            _ => {}
        }
    }
    None
}

/// `static let/var zero: T`（または `= T(...)`）を単位元候補の `MethodInfo` に正規化。
/// static でないプロパティは単位元ではない（None）。
fn parse_static_property(n: Node, source: &str) -> Option<MethodInfo> {
    if !has_modifier(n, source, "static") && !has_modifier(n, source, "class") {
        return None;
    }
    // 名前: pattern > simple_identifier。
    let pattern = first_child_of_kind(n, "pattern")?;
    let name = first_child_of_kind(pattern, "simple_identifier")
        .map(|c| text_of(c, source).to_string())?;
    // 型: 明示 type_annotation、無ければイニシャライザの構築型（`= T(...)`）。
    let ret = property_type(n, source)?;
    Some(MethodInfo {
        name,
        self_param: None,
        params: Vec::new(),
        return_type: Some(ret),
        is_assoc_fn: true,
    })
}

/// プロパティの型: `type_annotation` の型、無ければ初期化子の構築型（`T(...)`）。
fn property_type(n: Node, source: &str) -> Option<String> {
    if let Some(ta) = first_child_of_kind(n, "type_annotation") {
        if let Some(ut) = first_child_of_kind(ta, "user_type") {
            return Some(text_of(ut, source).trim().to_string());
        }
    }
    // `static let unit = Money(...)` → call_expression の被呼び出し名。
    if let Some(call) = first_child_of_kind(n, "call_expression") {
        if let Some(id) = first_child_of_kind(call, "simple_identifier") {
            return Some(text_of(id, source).trim().to_string());
        }
    }
    None
}

/// 深さ優先で全ノードに `f` を適用。
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

    fn impls_of(src: &str) -> Vec<ImplInfo> {
        let tree = parser::parse_with(src, parser::Language::Swift).unwrap();
        extract_impls(tree.root_node(), src)
    }

    fn method<'a>(imp: &'a ImplInfo, name: &str) -> &'a MethodInfo {
        imp.methods.iter().find(|m| m.name == name).unwrap()
    }

    #[test]
    fn struct_combine_and_static_func_identity() {
        let impls = impls_of(
            "struct Money {\n  let amount: Int\n  func combine(_ o: Money) -> Money { o }\n  static func zero() -> Money { Money(amount: 0) }\n}",
        );
        let m = impls.iter().find(|i| i.type_name == "Money").unwrap();
        let combine = method(m, "combine");
        assert_eq!(combine.self_param, Some(SelfKind::Ref));
        assert_eq!(combine.params, vec!["Money".to_string()]);
        assert_eq!(combine.return_type.as_deref(), Some("Money"));
        let zero = method(m, "zero");
        assert_eq!(zero.self_param, None);
        assert!(zero.is_assoc_fn);
        assert_eq!(zero.return_type.as_deref(), Some("Money"));
    }

    #[test]
    fn static_let_property_is_normalized_to_identity() {
        // `static let zero = Money(...)` — a property, not a func, but Swift's
        // idiomatic identity. Must become a no-arg assoc fn returning Money.
        let impls = impls_of(
            "struct Money {\n  let amount: Int\n  func combine(_ o: Money) -> Money { o }\n  static let zero = Money(amount: 0)\n}",
        );
        let m = &impls[0];
        let zero = method(m, "zero");
        assert_eq!(zero.self_param, None);
        assert!(zero.params.is_empty());
        assert_eq!(zero.return_type.as_deref(), Some("Money"));
    }

    #[test]
    fn static_let_with_type_annotation() {
        let impls = impls_of(
            "struct V {\n  func combine(_ o: V) -> V { o }\n  static let unit: V = V()\n}",
        );
        let unit = method(&impls[0], "unit");
        assert_eq!(unit.return_type.as_deref(), Some("V"));
    }

    #[test]
    fn extension_methods_attributed_to_extended_type() {
        let impls = impls_of("extension Money {\n  func negate() -> Money { self }\n}");
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].type_name, "Money");
        assert_eq!(method(&impls[0], "negate").return_type.as_deref(), Some("Money"));
    }

    #[test]
    fn mutating_method_is_mutref() {
        let impls = impls_of("struct S {\n  mutating func add(_ o: S) -> S { o }\n}");
        assert_eq!(method(&impls[0], "add").self_param, Some(SelfKind::MutRef));
    }

    #[test]
    fn free_function_returning_type() {
        let src = "func zero() -> Money { Money(amount: 0) }";
        let tree = parser::parse_with(src, parser::Language::Swift).unwrap();
        let fns = extract_free_fns(tree.root_node(), src);
        assert_eq!(fns.len(), 1);
        assert_eq!(fns[0].name, "zero");
        assert_eq!(fns[0].return_type.as_deref(), Some("Money"));
        assert!(fns[0].self_param.is_none());
    }

    #[test]
    fn type_sites_skip_extensions() {
        let src = "struct Money {}\nextension Money {}";
        let tree = parser::parse_with(src, parser::Language::Swift).unwrap();
        let sites = extract_type_sites(tree.root_node(), src, Path::new("M.swift"));
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].0, "Money");
    }
}
