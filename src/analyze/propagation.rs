//! 文脈伝播度（Axis 4）の計測。
//!
//! 型定義から構造的サイズを機械的に算出する：
//! - enum → バリアント数
//! - struct → フィールド型の propagation の直積
//! - 再帰型・コレクション型 → Unbounded
//! - プリミティブ型 → 1
//!
//! 再帰検出は、その型が自分自身へのパスを持つか（直接的または他の型経由）で判定する。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use tree_sitter::Node;

use crate::domain::konpu::PropagationSize;

/// 抽出された型定義1件分。
#[derive(Debug, Clone)]
pub struct TypeInfo {
    pub name: String,
    pub kind: TypeKind,
    /// enum のバリアント数。struct は 0。
    pub variant_count: usize,
    /// struct のフィールド型のソース文字列。
    pub field_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    Enum,
    Struct,
    Other,
}

fn text_of(node: Node, source: &str) -> String {
    node.utf8_text(source.as_bytes())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// tree-sitter AST から struct/enum 定義を抽出する。
pub fn extract_type_infos(root: Node, source: &str) -> Vec<TypeInfo> {
    let mut out = Vec::new();
    recurse_types(root, source, &mut out);
    out
}

fn recurse_types(node: Node, source: &str, out: &mut Vec<TypeInfo>) {
    match node.kind() {
        "struct_item" => {
            if let Some(info) = parse_struct(node, source) {
                out.push(info);
            }
        }
        "enum_item" => {
            if let Some(info) = parse_enum(node, source) {
                out.push(info);
            }
        }
        _ => {}
    }
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        recurse_types(child, source, out);
    }
}

fn type_name_of(item: Node, source: &str) -> Option<String> {
    let name = item.child_by_field_name("name")?;
    Some(text_of(name, source))
}

fn parse_struct(node: Node, source: &str) -> Option<TypeInfo> {
    let name = type_name_of(node, source)?;
    let body = find_body(node)?;
    let mut field_types = Vec::new();
    let mut cur = body.walk();
    for child in body.children(&mut cur) {
        if child.kind() == "field_declaration" {
            if let Some(t) = child.child_by_field_name("type") {
                field_types.push(text_of(t, source));
            }
        }
    }
    Some(TypeInfo {
        name,
        kind: TypeKind::Struct,
        variant_count: 0,
        field_types,
    })
}

fn parse_enum(node: Node, source: &str) -> Option<TypeInfo> {
    let name = type_name_of(node, source)?;
    let body = find_body(node)?;
    let mut variant_count = 0;
    let mut cur = body.walk();
    for child in body.children(&mut cur) {
        if child.kind() == "enum_variant" {
            variant_count += 1;
        }
    }
    Some(TypeInfo {
        name,
        kind: TypeKind::Enum,
        variant_count,
        field_types: Vec::new(),
    })
}

fn find_body(node: Node) -> Option<Node> {
    let mut cur = node.walk();
    node.children(&mut cur).find(|c| {
        matches!(
            c.kind(),
            "declaration_list" | "field_declaration_list" | "variant_list"
        )
    })
}

const COLLECTION_TYPES: &[&str] = &[
    "Vec", "VecDeque", "LinkedList", "HashMap", "BTreeMap", "HashSet", "BTreeSet",
    "BinaryHeap", "Option", "Result", "Box", "Rc", "Arc", "RefCell", "Cell", "Mutex",
    "RwLock",
];

const PRIMITIVE_TYPES: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128",
    "usize", "f32", "f64", "bool", "char", "String", "str", "()",
];

/// 型名の head（generic 引数を取り除いたもの）を返す。
fn type_head(ty: &str) -> String {
    let ty = ty.trim();
    if let Some(i) = ty.find(|c: char| c == '<' || c == '(' || c.is_whitespace()) {
        ty[..i].trim().to_string()
    } else {
        ty.to_string()
    }
}

fn is_collection_type(ty: &str) -> bool {
    let head = type_head(ty);
    COLLECTION_TYPES.iter().any(|c| *c == head)
}

fn is_primitive(ty: &str) -> bool {
    let head = type_head(ty);
    PRIMITIVE_TYPES.iter().any(|c| *c == head || ty.trim() == *c)
}

/// 型テーブル（型名 → TypeInfo）を構築する。
type TypeTable<'a> = HashMap<String, &'a TypeInfo>;

fn build_type_table<'a>(infos: &'a [TypeInfo]) -> TypeTable<'a> {
    infos.iter().map(|t| (t.name.clone(), t)).collect()
}

/// 指定した型名の propagation を計算する。再帰型は Unbounded。
pub fn compute_propagation(
    type_name: &str,
    infos: &[TypeInfo],
) -> (PropagationSize, Option<i64>) {
    let table = build_type_table(infos);
    let mut visiting: HashSet<String> = HashSet::new();
    compute_inner(type_name, &table, &mut visiting)
}

fn compute_inner(
    type_name: &str,
    table: &TypeTable,
    visiting: &mut HashSet<String>,
) -> (PropagationSize, Option<i64>) {
    let type_name = type_name.trim();
    if is_collection_type(type_name) {
        return (PropagationSize::Unbounded, None);
    }
    if is_primitive(type_name) {
        return (PropagationSize::Finite, Some(1));
    }
    let head = type_head(type_name);
    if !visiting.insert(head.clone()) {
        return (PropagationSize::Unbounded, None);
    }
    let (size, count) = match table.get(&head) {
        None => (PropagationSize::Finite, Some(1)),
        Some(info) => match info.kind {
            TypeKind::Enum => (PropagationSize::Finite, Some(info.variant_count.max(1) as i64)),
            TypeKind::Struct => {
                let mut product: i64 = 1;
                let mut unbounded = false;
                for field_ty in &info.field_types {
                    let (fs, fc) = compute_inner(field_ty, table, visiting);
                    if fs == PropagationSize::Unbounded {
                        unbounded = true;
                        break;
                    }
                    if let Some(c) = fc {
                        product = product.saturating_mul(c);
                    }
                }
                if unbounded {
                    (PropagationSize::Unbounded, None)
                } else {
                    (PropagationSize::Finite, Some(product))
                }
            }
            TypeKind::Other => (PropagationSize::Finite, Some(1)),
        },
    };
    visiting.remove(&head);
    (size, count)
}

/// 複数ファイルから抽出した TypeInfo 群をマージするヘルパ。
pub fn merge_type_infos(fragments: Vec<Vec<TypeInfo>>) -> Vec<TypeInfo> {
    fragments.into_iter().flatten().collect()
}

#[allow(dead_code)]
fn _accepts_path(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::konpu::PropagationSize;

    fn info_enum(name: &str, n: usize) -> TypeInfo {
        TypeInfo {
            name: name.to_string(),
            kind: TypeKind::Enum,
            variant_count: n,
            field_types: Vec::new(),
        }
    }

    fn info_struct(name: &str, fields: &[&str]) -> TypeInfo {
        TypeInfo {
            name: name.to_string(),
            kind: TypeKind::Struct,
            variant_count: 0,
            field_types: fields.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn primitive_finite_one() {
        let (s, c) = compute_propagation("i32", &[]);
        assert_eq!(s, PropagationSize::Finite);
        assert_eq!(c, Some(1));
    }

    #[test]
    fn vec_unbounded() {
        let (s, c) = compute_propagation("Vec<u8>", &[]);
        assert_eq!(s, PropagationSize::Unbounded);
        assert_eq!(c, None);
    }

    #[test]
    fn enum_variant_count() {
        let infos = vec![info_enum("Color", 3)];
        let (s, c) = compute_propagation("Color", &infos);
        assert_eq!(s, PropagationSize::Finite);
        assert_eq!(c, Some(3));
    }

    #[test]
    fn struct_product_of_primitives() {
        let infos = vec![info_struct("Point", &["i32", "bool"])];
        let (s, c) = compute_propagation("Point", &infos);
        assert_eq!(s, PropagationSize::Finite);
        assert_eq!(c, Some(1));
    }

    #[test]
    fn struct_product_of_enums() {
        let infos = vec![
            info_enum("Color", 3),
            info_enum("Shape", 4),
            info_struct("Widget", &["Color", "Shape"]),
        ];
        let (s, c) = compute_propagation("Widget", &infos);
        assert_eq!(s, PropagationSize::Finite);
        assert_eq!(c, Some(12));
    }

    #[test]
    fn struct_with_vec_unbounded() {
        let infos = vec![info_struct("Log", &["Vec<String>"])];
        let (s, c) = compute_propagation("Log", &infos);
        assert_eq!(s, PropagationSize::Unbounded);
        assert_eq!(c, None);
    }

    #[test]
    fn recursive_struct_unbounded() {
        let infos = vec![info_struct("Node", &["i32", "Box<Node>"])];
        let (s, c) = compute_propagation("Node", &infos);
        assert_eq!(s, PropagationSize::Unbounded);
        assert_eq!(c, None);
    }

    #[test]
    fn mutually_recursive_unbounded() {
        let infos = vec![
            info_struct("A", &["B"]),
            info_struct("B", &["A"]),
        ];
        let (s, c) = compute_propagation("A", &infos);
        assert_eq!(s, PropagationSize::Unbounded);
        assert_eq!(c, None);
    }

    #[test]
    fn unknown_type_treated_as_one() {
        let (s, c) = compute_propagation("Mystery", &[]);
        assert_eq!(s, PropagationSize::Finite);
        assert_eq!(c, Some(1));
    }

    #[test]
    fn empty_enum_is_one() {
        let infos = vec![info_enum("Empty", 0)];
        let (s, c) = compute_propagation("Empty", &infos);
        assert_eq!(s, PropagationSize::Finite);
        assert_eq!(c, Some(1));
    }
}