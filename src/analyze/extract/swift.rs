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

use super::{ImplInfo, MethodInfo, SelfKind};

use crate::analyze::propagation::{TypeInfo, TypeKind};

use crate::analyze::directive::{higher_from, parse_directive, structure_from, Directive};
use super::{
    ignore_reason_from_str, law_from_name, AnalyzedDeclaration, IgnoreInfo, LawTestInfo, UseStatement,
};
use crate::analyze::parser::Language;

/// Swift ファイル 1 つからバンドルを返す（言語ディスパッチ用）。
pub fn extract_all_file(root: Node, source: &str, path: &Path) -> crate::analyze::FileExtract {
    crate::analyze::FileExtract {
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

/// コメント直後の named 宣言（他のコメントは飛ばす）。Swift のコメントは `comment`。
fn following_decl(comment: Node) -> Option<Node> {
    let mut sib = comment.next_named_sibling();
    while let Some(n) = sib {
        if n.kind() == "comment" {
            sib = n.next_named_sibling();
        } else {
            return Some(n);
        }
    }
    None
}

/// 全 konpu ディレクティブコメントを (Directive, 直後宣言, 行) で列挙。
fn directives<'a>(root: Node<'a>, source: &'a str) -> Vec<(Directive, Option<Node<'a>>, usize)> {
    let mut comments = Vec::new();
    collect_comments(root, &mut comments);
    comments
        .into_iter()
        .filter_map(|n| {
            parse_directive(text_of(n, source))
                .map(|d| (d, following_decl(n), n.start_position().row + 1))
        })
        .collect()
}

fn collect_comments<'a>(n: Node<'a>, out: &mut Vec<Node<'a>>) {
    if n.kind() == "comment" {
        out.push(n);
    }
    let mut cur = n.walk();
    for c in n.children(&mut cur) {
        collect_comments(c, out);
    }
}

/// `// konpu: monoid(op: ..., identity: ...)` 等 → 直後の型への宣言。
pub fn extract_declarations(root: Node, source: &str, path: &Path) -> Vec<AnalyzedDeclaration> {
    let mut out = Vec::new();
    for (d, decl, line) in directives(root, source) {
        let Some(structure) = structure_from(&d.head) else { continue };
        let Some(decl) = decl else { continue };
        if decl.kind() != "class_declaration" {
            continue;
        }
        let Some(kw) = decl_keyword(decl, source) else { continue };
        let Some(type_name) = decl_type_name(decl, source, &kw) else { continue };
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

/// `// konpu: law(associativity, ...)` → 直後のテスト関数の LawTestInfo。
/// Swift のテストは XCTest クラス内で対象型と別なので enclosing_type=None（全型に一致）。
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
                .then(|| first_child_of_kind(n, "simple_identifier").map(|c| text_of(c, source).to_string()))
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
        let type_name = decl.and_then(|n| {
            (n.kind() == "class_declaration")
                .then(|| decl_keyword(n, source).and_then(|kw| decl_type_name(n, source, &kw)))
                .flatten()
        });
        out.push(IgnoreInfo { reason, note: d.kwargs.get("note").cloned(), type_name, path: path.to_path_buf(), line });
    }
    out
}

/// 文脈伝播度（Axis 4）用の型情報。struct のフィールド型と enum のバリアント数。
/// ponytail: Swift の `[T]`/`T?` はコレクション/optional だが propagation の
/// Unbounded 判定は Rust の `Vec`/`Option` 前提。Swift 構文の非有界判定は未対応
/// （過小評価）。実需が出たら propagation 側に Swift 構文を足す。
pub fn extract_type_infos(root: Node, source: &str) -> Vec<TypeInfo> {
    let mut out = Vec::new();
    recurse(root, &mut |n| {
        if n.kind() != "class_declaration" {
            return;
        }
        let Some(kw) = decl_keyword(n, source) else { return };
        let Some(name) = decl_type_name(n, source, &kw) else { return };
        match kw.as_str() {
            "enum" => {
                let Some(body) = first_child_of_kind(n, "enum_class_body") else { return };
                let mut variant_count = 0;
                let mut bcur = body.walk();
                for entry in body.children(&mut bcur) {
                    if entry.kind() == "enum_entry" {
                        let mut ecur = entry.walk();
                        variant_count += entry
                            .children(&mut ecur)
                            .filter(|c| c.kind() == "simple_identifier")
                            .count();
                    }
                }
                out.push(TypeInfo { name, kind: TypeKind::Enum, variant_count, field_types: Vec::new() });
            }
            "struct" | "class" => {
                let Some(body) = first_child_of_kind(n, "class_body") else { return };
                let mut field_types = Vec::new();
                let mut bcur = body.walk();
                for member in body.children(&mut bcur) {
                    if member.kind() == "property_declaration"
                        && !has_modifier(member, source, "static")
                        && !has_modifier(member, source, "class")
                    {
                        if let Some(t) = property_type(member, source) {
                            field_types.push(normalize_swift_type(&t));
                        }
                    }
                }
                out.push(TypeInfo { name, kind: TypeKind::Struct, variant_count: 0, field_types });
            }
            _ => {}
        }
    });
    out
}

fn text_of<'a>(n: Node, source: &'a str) -> &'a str {
    n.utf8_text(source.as_bytes()).unwrap_or("")
}

/// Swift のコレクション/optional 構文を konpu 正準名へ正規化（propagation は
/// head でコレクションを判定するため、head が Vec/HashMap/HashSet/Option に
/// なれば `[T]`/`T?`/`Set<T>` を Unbounded と見なせる）。propagation.rs は
/// Rust 名前前提なので、言語差はここ（Swift 境界）で吸収する。
fn normalize_swift_type(s: &str) -> String {
    let s = s.trim();
    if let Some(inner) = s.strip_suffix('?') {
        return format!("Option<{}>", normalize_swift_type(inner));
    }
    if let Some(mid) = s.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
        // `[K: V]` は辞書、`[T]` は配列。どちらも非有界。
        return if mid.contains(':') {
            format!("HashMap<{}>", mid.trim())
        } else {
            format!("Vec<{}>", normalize_swift_type(mid.trim()))
        };
    }
    for (swift, rust) in [
        ("Array", "Vec"),
        ("Dictionary", "HashMap"),
        ("Set", "HashSet"),
        ("Optional", "Option"),
    ] {
        if let Some(rest) = s.strip_prefix(swift) {
            if rest.is_empty() || rest.starts_with('<') {
                return format!("{rust}{rest}");
            }
        }
    }
    s.to_string()
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
        // プロトコル準拠が保証する代数サーフェスを合成メソッドとして足す。
        methods.extend(synthetic_conformance_methods(&conformances(n, source), &type_name));
        if !methods.is_empty() {
            out.push(ImplInfo { type_name, methods });
        }
    });
    out
}

/// `class_declaration` が準拠する named プロトコル一覧。
fn conformances(n: Node, source: &str) -> Vec<String> {
    let mut cur = n.walk();
    n.children(&mut cur)
        .filter(|c| c.kind() == "inheritance_specifier")
        .filter_map(|c| first_child_of_kind(c, "user_type"))
        .filter_map(|ut| first_child_of_kind(ut, "type_identifier"))
        .map(|ti| text_of(ti, source).to_string())
        .collect()
}

/// プロトコル準拠から保証される代数サーフェスを合成メソッドで返す。
/// `AdditiveArithmetic` はコンパイラ強制で `+` と `static var zero` を保証する
/// （閉じた加法＋単位元）→ add+zero に正規化し、明示メソッドが無くても Monoid を
/// 推論できるようにする（準拠はコンパイラ検証済みなので honest）。
/// ponytail: Group まで上げるには逆元（negate 等）の明示が要る。準拠だけでは
/// 構造的に確認できる床＝Monoid に留める。
fn synthetic_conformance_methods(protos: &[String], ty: &str) -> Vec<MethodInfo> {
    let mut out = Vec::new();
    if protos.iter().any(|p| p == "AdditiveArithmetic") {
        out.push(MethodInfo {
            name: "add".to_string(),
            self_param: None,
            params: vec![ty.to_string(), ty.to_string()],
            return_type: Some(ty.to_string()),
            is_assoc_fn: true,
        });
        out.push(MethodInfo {
            name: "zero".to_string(),
            self_param: None,
            params: Vec::new(),
            return_type: Some(ty.to_string()),
            is_assoc_fn: true,
        });
    }
    out
}

/// `import <Module>` を UseStatement として集める。imported_path = モジュール名。
/// 境界検査は Swift import を `from_modules`（モジュール名リスト）と照合する。
pub fn extract_use_statements(root: Node, source: &str, path: &Path) -> Vec<UseStatement> {
    let mut out = Vec::new();
    recurse(root, &mut |n| {
        if n.kind() == "import_declaration" {
            if let Some(id) = first_child_of_kind(n, "identifier") {
                let module = text_of(id, source).trim().to_string();
                if !module.is_empty() {
                    out.push(UseStatement {
                        path: path.to_path_buf(),
                        imported_path: module,
                        line: n.start_position().row + 1,
                        language: Language::Swift,
                    });
                }
            }
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

/// `function_declaration` の名前（`func` トークンの次の子）を konpu 用に正規化。
/// 演算子は族名にマップ（`+`→add, `*`→mul）。非結合の `-`/`/` 等は生のまま
/// （どの族にも一致せず推論では無視される＝正しい）。
fn fn_name(n: Node, source: &str) -> Option<String> {
    let mut cur = n.walk();
    let mut seen_func = false;
    for c in n.children(&mut cur) {
        if seen_func {
            let raw = text_of(c, source).trim();
            return Some(match raw {
                "+" => "add".to_string(),
                "*" => "mul".to_string(),
                other => other.to_string(),
            });
        }
        if !c.is_named() && text_of(c, source).trim() == "func" {
            seen_func = true;
        }
    }
    None
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
    // 名前は `func` トークンの次の子。通常メソッドは `simple_identifier`、
    // 演算子メソッドは無名トークン（`+`/`*` 等）。演算子は konpu の族名へ正規化。
    let name = fn_name(n, source)?;
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

/// プロパティの型: `type_annotation` 直下の型ノード（user_type/array_type/…）、
/// 無ければ初期化子の構築型（`= T(...)`）。
fn property_type(n: Node, source: &str) -> Option<String> {
    if let Some(ta) = first_child_of_kind(n, "type_annotation") {
        let mut cur = ta.walk();
        if let Some(t) = ta.children(&mut cur).find(|c| c.is_named()) {
            return Some(text_of(t, source).trim().to_string());
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
    use crate::domain::konpu::{AlgebraicStructure, HigherKindedStructure};

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

    fn type_infos_of(src: &str) -> Vec<TypeInfo> {
        let tree = parser::parse_with(src, parser::Language::Swift).unwrap();
        extract_type_infos(tree.root_node(), src)
    }

    #[test]
    fn enum_variant_count() {
        let t = type_infos_of("enum E { case a, b\n case c(Int)\n case d }");
        let e = t.iter().find(|i| i.name == "E").unwrap();
        assert_eq!(e.kind, TypeKind::Enum);
        assert_eq!(e.variant_count, 4); // a, b, c, d
    }

    #[test]
    fn struct_field_types_exclude_static() {
        let t = type_infos_of(
            "struct S {\n  let x: Int\n  var y: Money\n  let z: [Money]\n  static let zero = S()\n}",
        );
        let s = t.iter().find(|i| i.name == "S").unwrap();
        assert_eq!(s.kind, TypeKind::Struct);
        // static `zero` excluded; array field normalized (`[Money]` -> `Vec<Money>`).
        assert_eq!(s.field_types, vec!["Int".to_string(), "Money".to_string(), "Vec<Money>".to_string()]);
    }

    #[test]
    fn plus_operator_maps_to_add_binary_op() {
        let impls = impls_of(
            "struct Vec2 {\n  static func + (lhs: Vec2, rhs: Vec2) -> Vec2 { lhs }\n}",
        );
        let add = method(&impls[0], "add"); // `+` normalized to `add`
        assert!(add.is_assoc_fn);
        assert_eq!(add.params, vec!["Vec2".to_string(), "Vec2".to_string()]);
        assert_eq!(add.return_type.as_deref(), Some("Vec2"));
    }

    #[test]
    fn star_operator_maps_to_mul() {
        let impls = impls_of("struct M {\n  static func * (a: M, b: M) -> M { a }\n}");
        assert!(impls[0].methods.iter().any(|m| m.name == "mul"));
    }

    #[test]
    fn additive_arithmetic_conformance_synthesizes_add_and_zero() {
        // No explicit +/zero in source; conformance guarantees them.
        let impls = impls_of("struct Money: AdditiveArithmetic, Equatable {\n  let amount: Int\n}");
        let m = impls.iter().find(|i| i.type_name == "Money").unwrap();
        assert!(m.methods.iter().any(|x| x.name == "add" && x.params.len() == 2));
        assert!(m.methods.iter().any(|x| x.name == "zero" && x.params.is_empty()));
    }

    #[test]
    fn non_algebraic_conformance_synthesizes_nothing() {
        let impls = impls_of("struct P: Equatable {\n  static func + (a: P, b: P) -> P { a }\n}");
        // only the real `+`->add, no synthetic zero from Equatable.
        let p = &impls[0];
        assert!(p.methods.iter().any(|m| m.name == "add"));
        assert!(!p.methods.iter().any(|m| m.name == "zero"));
    }

    #[test]
    fn comment_annotation_declares_monoid() {
        let src = "// konpu: monoid(op: combine, identity: zero)\nstruct Money { let amount: Int }";
        let tree = parser::parse_with(src, parser::Language::Swift).unwrap();
        let decls = extract_declarations(tree.root_node(), src, Path::new("M.swift"));
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].type_name, "Money");
        assert_eq!(decls[0].target_structure, AlgebraicStructure::Monoid);
        assert_eq!(decls[0].operation_name, "combine");
        assert_eq!(decls[0].identity_name.as_deref(), Some("zero"));
    }

    #[test]
    fn swift_collection_types_normalize_to_unbounded_heads() {
        assert_eq!(normalize_swift_type("[Money]"), "Vec<Money>");
        assert_eq!(normalize_swift_type("Money?"), "Option<Money>");
        assert_eq!(normalize_swift_type("[String: Int]"), "HashMap<String: Int>");
        assert_eq!(normalize_swift_type("Set<Money>"), "HashSet<Money>");
        assert_eq!(normalize_swift_type("Int"), "Int"); // primitive unchanged
    }

    #[test]
    fn struct_array_field_is_normalized() {
        let t = type_infos_of("struct S { let xs: [Money] }");
        assert_eq!(t[0].field_types, vec!["Vec<Money>".to_string()]);
    }

    #[test]
    fn comment_annotation_with_higher_kinded() {
        let src = "// konpu: monoid(op: compose, higher: functor)\nstruct Parser { let x: Int }";
        let tree = parser::parse_with(src, parser::Language::Swift).unwrap();
        let decls = extract_declarations(tree.root_node(), src, Path::new("P.swift"));
        assert_eq!(decls[0].higher_kinded, Some(HigherKindedStructure::Functor));
    }

    #[test]
    fn comment_law_annotation_on_test_func() {
        let src = "class T {\n  // konpu: law(associativity)\n  func testAssoc() {}\n}";
        let tree = parser::parse_with(src, parser::Language::Swift).unwrap();
        let laws = extract_law_tests(tree.root_node(), src, Path::new("T.swift"));
        assert_eq!(laws.len(), 1);
        assert_eq!(laws[0].test_fn.as_deref(), Some("testAssoc"));
        assert!(laws[0].enclosing_type.is_none());
    }

    #[test]
    fn comment_ignore_annotation() {
        let src = "// konpu: ignore(reason: intentional, note: \"order matters\")\nstruct Discounts {}";
        let tree = parser::parse_with(src, parser::Language::Swift).unwrap();
        let ig = extract_ignores(tree.root_node(), src, Path::new("D.swift"));
        assert_eq!(ig.len(), 1);
        assert_eq!(ig[0].note.as_deref(), Some("order matters"));
        assert_eq!(ig[0].type_name.as_deref(), Some("Discounts"));
    }

    #[test]
    fn imports_extracted_as_module_uses() {
        let src = "import Foundation\nimport DomainKit\n@testable import InfraKit\nstruct S {}";
        let tree = parser::parse_with(src, parser::Language::Swift).unwrap();
        let uses = extract_use_statements(tree.root_node(), src, Path::new("S.swift"));
        let mods: Vec<&str> = uses.iter().map(|u| u.imported_path.as_str()).collect();
        assert_eq!(mods, vec!["Foundation", "DomainKit", "InfraKit"]);
        assert!(uses.iter().all(|u| u.language == parser::Language::Swift));
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
