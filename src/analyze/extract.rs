use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::domain::konpu::{AlgebraicStructure, HigherKindedStructure, Law};

#[derive(Debug, Clone)]
pub struct AnalyzedDeclaration {
    pub target_structure: AlgebraicStructure,
    pub higher_kinded: Option<HigherKindedStructure>,
    pub type_name: String,
    pub operation_name: String,
    pub identity_name: Option<String>,
    pub inverse_name: Option<String>,
    pub path: std::path::PathBuf,
    pub line: usize,
    /// 文脈伝播度（Phase 1-B で算出）。未算出なら None。
    pub propagation: Option<crate::domain::konpu::PropagationSize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelfKind {
    Owned,
    Ref,
    MutRef,
    None,
}

#[derive(Debug, Clone)]
pub struct MethodInfo {
    pub name: String,
    pub self_param: Option<SelfKind>,
    pub params: Vec<String>,
    pub return_type: Option<String>,
    pub is_assoc_fn: bool,
}

#[derive(Debug, Clone)]
pub struct ImplInfo {
    pub type_name: String,
    pub methods: Vec<MethodInfo>,
}

fn text_of(node: Node, source: &str) -> String {
    node.utf8_text(source.as_bytes())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn child_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cur = node.walk();
    node.children(&mut cur).find(|child| child.kind() == kind)
}

fn first_child_by_field<'a>(node: Node<'a>, field: &str) -> Option<Node<'a>> {
    node.child_by_field_name(field)
}

fn item_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "struct_item" | "enum_item" | "function_item" | "trait_item" | "type_item"
        | "constant_item" | "static_item" | "macro_definition" => {}
        _ => return None,
    }
    let name = node.child_by_field_name("name")?;
    Some(text_of(name, source))
}

fn parse_konpu_args(attr_text: &str, structure: AlgebraicStructure) -> AnalyzedDeclaration {
    let mut op = None;
    let mut identity = None;
    let mut inverse = None;
    let mut higher = None;
    let inside = match attr_text.find('(') {
        Some(i) => {
            let close = attr_text.rfind(')').unwrap_or(attr_text.len());
            &attr_text[i + 1..close]
        }
        None => attr_text,
    };
    for pair_raw in split_args(inside) {
        let pair = pair_raw.trim();
        if pair.is_empty() {
            continue;
        }
        if let Some(eq) = pair.find('=') {
            let key = pair[..eq].trim();
            let val = pair[eq + 1..].trim();
            let val = strip_quotes(val);
            match key {
                "op" => op = val,
                "identity" => identity = val,
                "inverse" => inverse = val,
                "higher" => {
                    if let Some(v) = val {
                        higher = match v.as_str() {
                            "functor" => Some(HigherKindedStructure::Functor),
                            "applicative" => Some(HigherKindedStructure::Applicative),
                            "monad" => Some(HigherKindedStructure::MonadS),
                            _ => None,
                        };
                    }
                }
                _ => {}
            }
        }
    }
    AnalyzedDeclaration {
        target_structure: structure,
        higher_kinded: higher,
        type_name: String::new(),
        operation_name: op.unwrap_or_default(),
        identity_name: identity,
        inverse_name: inverse,
        path: std::path::PathBuf::new(),
        line: 0,
        propagation: None,
    }
}

fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    for c in s.chars() {
        if in_str {
            cur.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                cur.push(c);
            }
            ',' => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn strip_quotes(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        Some(s[1..s.len() - 1].to_string())
    } else if s.starts_with('"') || s.ends_with('"') {
        None
    } else {
        Some(s.to_string())
    }
}

fn structure_from_attr(attr_text: &str) -> Option<AlgebraicStructure> {
    if attr_text.contains("konpu::monoid") {
        Some(AlgebraicStructure::Monoid)
    } else if attr_text.contains("konpu::group") {
        Some(AlgebraicStructure::Group)
    } else if attr_text.contains("konpu::semigroup") {
        Some(AlgebraicStructure::Semigroup)
    } else if attr_text.contains("konpu::magma") {
        Some(AlgebraicStructure::Magma)
    } else {
        None
    }
}

pub fn extract_declarations(
    root: Node,
    source: &str,
    path: &Path,
) -> Vec<AnalyzedDeclaration> {
    let mut seen: std::collections::HashSet<(usize, String)> = std::collections::HashSet::new();
    let mut out = Vec::new();
    recurse_collect(root, source, path, &mut out, &mut seen);
    out
}

fn is_item_kind(kind: &str) -> bool {
    matches!(
        kind,
        "struct_item"
            | "enum_item"
            | "function_item"
            | "trait_item"
            | "type_item"
            | "constant_item"
            | "static_item"
            | "macro_definition"
    )
}

fn next_sibling_item(node: Node) -> Option<Node> {
    let parent = node.parent()?;
    let mut cur = parent.walk();
    let mut after = false;
    for sibling in parent.children(&mut cur) {
        if after && is_item_kind(sibling.kind()) {
            return Some(sibling);
        }
        if sibling == node {
            after = true;
        }
    }
    None
}

fn finalize(
    attr: Node,
    item: Node,
    source: &str,
    path: &Path,
) -> Option<AnalyzedDeclaration> {
    let attr_text = text_of(attr, source);
    let structure = structure_from_attr(&attr_text)?;
    let name = item_name(item, source)?;
    let mut decl = parse_konpu_args(&attr_text, structure);
    decl.type_name = name;
    decl.path = path.to_path_buf();
    decl.line = attr.start_position().row + 1;
    Some(decl)
}

fn recurse_collect(
    node: Node,
    source: &str,
    path: &Path,
    out: &mut Vec<AnalyzedDeclaration>,
    seen: &mut std::collections::HashSet<(usize, String)>,
) {
    let is_attr = matches!(node.kind(), "attribute" | "attribute_item");
    let parent_is_attr = node
        .parent()
        .is_some_and(|p| matches!(p.kind(), "attribute" | "attribute_item"));
    if is_attr && !parent_is_attr {
        let txt = text_of(node, source);
        if structure_from_attr(&txt).is_some() {
            let item = if let Some(parent) = node.parent() {
                if is_item_kind(parent.kind()) {
                    Some(parent)
                } else {
                    next_sibling_item(node)
                }
            } else {
                None
            };
            if let Some(item) = item {
                let line = node.start_position().row;
                let key = (line, txt.clone());
                if seen.insert(key) {
                    if let Some(decl) = finalize(node, item, source, path) {
                        out.push(decl);
                    }
                }
            }
        }
    }
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        recurse_collect(child, source, path, out, seen);
    }
}

#[derive(Debug, Clone)]
pub struct LawTestInfo {
    pub laws: Vec<Law>,
    pub enclosing_type: Option<String>,
    pub path: PathBuf,
    pub line: usize,
}

pub fn law_from_name(name: &str) -> Option<Law> {
    match name.trim() {
        "associativity" => Some(Law::Associativity),
        "left_identity" => Some(Law::LeftIdentity),
        "right_identity" => Some(Law::RightIdentity),
        "inverse_left" => Some(Law::InverseLeft),
        "inverse_right" => Some(Law::InverseRight),
        "functor_identity" => Some(Law::FunctorIdentity),
        "functor_composition" => Some(Law::FunctorComposition),
        "applicative_identity" => Some(Law::ApplicativeIdentity),
        "applicative_composition" => Some(Law::ApplicativeComposition),
        "monad_left_identity" => Some(Law::MonadLeftIdentity),
        "monad_right_identity" => Some(Law::MonadRightIdentity),
        "monad_associativity" => Some(Law::MonadAssociativity),
        _ => None,
    }
}

fn enclosing_impl_type(node: Node, source: &str) -> Option<String> {
    let mut cur = node;
    loop {
        cur = cur.parent()?;
        if cur.kind() == "impl_item" {
            return impl_type_name(cur, source);
        }
    }
}

pub fn extract_law_tests(root: Node, source: &str, path: &Path) -> Vec<LawTestInfo> {
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<(usize, String)> = std::collections::HashSet::new();
    recurse_collect_laws(root, source, path, &mut out, &mut seen);
    out
}

fn recurse_collect_laws(
    node: Node,
    source: &str,
    path: &Path,
    out: &mut Vec<LawTestInfo>,
    seen: &mut std::collections::HashSet<(usize, String)>,
) {
    if matches!(node.kind(), "attribute" | "attribute_item")
        && node
            .parent()
            .is_none_or(|p| !matches!(p.kind(), "attribute" | "attribute_item"))
    {
        let txt = text_of(node, source);
        if txt.contains("konpu::law") {
            let line = node.start_position().row;
            let key = (line, txt.clone());
            if seen.insert(key) {
                if let Some(info) = parse_law_attr(node, source, path) {
                    out.push(info);
                }
            }
        }
    }
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        recurse_collect_laws(child, source, path, out, seen);
    }
}

fn parse_law_attr(attr: Node, source: &str, path: &Path) -> Option<LawTestInfo> {
    let txt = text_of(attr, source);
    let inside = txt.find('(').map(|i| {
        let close = txt.rfind(')').unwrap_or(txt.len());
        &txt[i + 1..close]
    })?;
    let inside = inside.trim();
    let mut laws = Vec::new();
    for part in split_args(inside) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(law) = law_from_name(part) {
            laws.push(law);
        }
    }
    if laws.is_empty() {
        return None;
    }
    let enclosing_type = enclosing_impl_type(attr, source);
    Some(LawTestInfo {
        laws,
        enclosing_type,
        path: path.to_path_buf(),
        line: attr.start_position().row + 1,
    })
}

#[derive(Debug, Clone)]
pub struct IgnoreInfo {
    pub reason: crate::domain::konpu::IgnoreReason,
    pub note: Option<String>,
    pub type_name: Option<String>,
    pub path: std::path::PathBuf,
    pub line: usize,
}

pub fn ignore_reason_from_str(s: &str) -> Option<crate::domain::konpu::IgnoreReason> {
    use crate::domain::konpu::IgnoreReason;
    match s.trim() {
        "intentional" => Some(IgnoreReason::Intentional),
        "debt" => Some(IgnoreReason::Debt),
        "infeasible" => Some(IgnoreReason::Infeasible),
        _ => None,
    }
}

pub fn extract_ignores(root: Node, source: &str, path: &Path) -> Vec<IgnoreInfo> {
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<(usize, String)> = std::collections::HashSet::new();
    recurse_collect_ignores(root, source, path, &mut out, &mut seen);
    out
}

fn recurse_collect_ignores(
    node: Node,
    source: &str,
    path: &Path,
    out: &mut Vec<IgnoreInfo>,
    seen: &mut std::collections::HashSet<(usize, String)>,
) {
    if matches!(node.kind(), "attribute" | "attribute_item")
        && node
            .parent()
            .is_none_or(|p| !matches!(p.kind(), "attribute" | "attribute_item"))
    {
        let txt = text_of(node, source);
        if txt.contains("konpu::ignore") {
            let line = node.start_position().row;
            let key = (line, txt.clone());
            if seen.insert(key) {
                if let Some(info) = parse_ignore_attr(node, source, path) {
                    out.push(info);
                }
            }
        }
    }
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        recurse_collect_ignores(child, source, path, out, seen);
    }
}

fn parse_ignore_attr(attr: Node, source: &str, path: &Path) -> Option<IgnoreInfo> {
    let txt = text_of(attr, source);
    let inside = txt.find('(').map(|i| {
        let close = txt.rfind(')').unwrap_or(txt.len());
        &txt[i + 1..close]
    })?;
    let mut reason = None;
    let mut note: Option<String> = None;
    for part in split_args(inside) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(eq) = part.find('=') {
            let key = part[..eq].trim();
            let val = part[eq + 1..].trim();
            let val = strip_quotes(val);
            match key {
                "reason" => {
                    if let Some(v) = val {
                        reason = ignore_reason_from_str(&v);
                    }
                }
                "note" => note = val,
                _ => {}
            }
        }
    }
    let reason = reason?;
    let type_name = enclosing_impl_type(attr, source);
    Some(IgnoreInfo {
        reason,
        note,
        type_name,
        path: path.to_path_buf(),
        line: attr.start_position().row + 1,
    })
}

pub fn extract_impls(root: Node, source: &str) -> Vec<ImplInfo> {
    let mut out = Vec::new();
    recurse_impls(root, source, &mut out);
    out
}

fn recurse_impls(node: Node, source: &str, out: &mut Vec<ImplInfo>) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if child.kind() == "impl_item" {
            if let Some(info) = parse_impl(child, source) {
                out.push(info);
            }
        } else {
            recurse_impls(child, source, out);
        }
    }
}

fn parse_impl(node: Node, source: &str) -> Option<ImplInfo> {
    let type_name = impl_type_name(node, source)?;
    let body = child_by_kind(node, "declaration_list")?;
    let mut methods = Vec::new();
    let mut cur = body.walk();
    for child in body.children(&mut cur) {
        if child.kind() == "function_item" {
            if let Some(m) = parse_method(child, source) {
                methods.push(m);
            }
        }
    }
    Some(ImplInfo { type_name, methods })
}

fn impl_type_name(node: Node, source: &str) -> Option<String> {
    let type_id = first_child_by_field(node, "type")?;
    let raw = text_of(type_id, source);
    let raw = raw.trim();
    if let Some(i) = raw.find(|c: char| c == '<' || c.is_whitespace()) {
        let head = raw[..i].trim();
        if !head.is_empty() {
            return Some(head.to_string());
        }
    }
    Some(raw.to_string())
}

fn parse_method(node: Node, source: &str) -> Option<MethodInfo> {
    let name_node = node.child_by_field_name("name")?;
    let name = text_of(name_node, source);
    let params_node = node.child_by_field_name("parameters");
    let mut self_param: Option<SelfKind> = None;
    let mut params: Vec<String> = Vec::new();
    let mut is_assoc_fn = true;
    if let Some(params_node) = params_node {
        let mut cur = params_node.walk();
        for param in params_node.children(&mut cur) {
            match param.kind() {
                "self_parameter" => {
                    let txt = text_of(param, source);
                    if txt.contains("&mut self") {
                        self_param = Some(SelfKind::MutRef);
                    } else if txt.contains("&self") {
                        self_param = Some(SelfKind::Ref);
                    } else {
                        self_param = Some(SelfKind::Owned);
                    }
                    if let Some(tn) = param.child_by_field_name("type") {
                        let tt = text_of(tn, source);
                        if !tt.is_empty() {
                            params.push(tt);
                        }
                    }
                    is_assoc_fn = false;
                }
                "parameter" => {
                    let pat = param.child_by_field_name("pattern");
                    let ptype = param.child_by_field_name("type");
                    let pat_txt = pat.map(|n| text_of(n, source)).unwrap_or_default();
                    let type_txt = ptype.map(|n| text_of(n, source)).unwrap_or_default();
                    if !type_txt.is_empty() {
                        params.push(type_txt);
                    } else if !pat_txt.is_empty() {
                        params.push(pat_txt);
                    }
                }
                _ => {}
            }
        }
    }
    let return_type = node.child_by_field_name("return_type").map(|n| {
        text_of(n, source).trim().trim_start_matches("->").trim().to_string()
    });
    Some(MethodInfo {
        name,
        self_param,
        params,
        return_type,
        is_assoc_fn,
    })
}