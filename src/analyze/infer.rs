//! アノテーション無しでの代数構造の推論。
//!
//! 正直さ（RTA/preserve と同じ姿勢）: 推論できるのは構造の「形」だけ
//! （閉じた二項演算があるか / 単位元があるか / 逆元があるか）。結合律・単位律
//! そのものは静的には証明できない。しかし konpu の既存モデルと整合する —
//! 割り当てた構造には既存検査が `AssociativityConfidence`(Info) と
//! `MissingLawTest`(Warning) を出し「未証明。law test を書け」とヘッジする。
//! つまり推論は「宣言の出所」をアノテーション→コード信号に替えるだけ。
//!
//! 検出（impl のメソッド名 + シグネチャから、型 `T` について）:
//! - 二項演算: 閉じた `fn f(self|&self, other: T|Self) -> T|Self`（演算子トレイト
//!   `add/mul/...` + 慣用名 `combine/merge/...`）。両オペランドが同型なのが鍵で、
//!   ビルダー `fn with_x(self, x: U) -> Self` は除外される。
//! - 単位元: 引数0の関連関数 `zero/one/empty/identity/unit/default` → `T|Self`。
//! - 逆元: `neg/inv/inverse/negate` → `T|Self`。
//!
//! 構造割当: 演算+単位元+逆元=Group / 演算+単位元=Monoid / 演算のみ=Semigroup。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::domain::konpu::AlgebraicStructure;

use super::extract::{AnalyzedDeclaration, ImplInfo, MethodInfo, SelfKind};

/// 二項演算とみなすメソッド名（演算子トレイト + 慣用名）。優先順位でもある。
const OP_NAMES: &[&str] = &[
    "combine", "merge", "append", "concat", "op", "mappend", "add", "mul", "join", "meet", "and",
    "or", "min", "max", "sub", "div", "bitand", "bitor", "bitxor",
];
/// 単位元とみなす引数0の関連関数名。
const ID_NAMES: &[&str] = &["identity", "empty", "zero", "unit", "neutral", "one", "default"];
/// 逆元とみなすメソッド名。
const INV_NAMES: &[&str] = &["inverse", "inv", "neg", "negate"];

/// 全 impl と型サイトから、代数構造を推論した宣言を返す。
/// `annotated` に既にある型はスキップ（アノテーション優先）。
pub fn infer_declarations(
    impls: &[ImplInfo],
    type_sites: &HashMap<String, (PathBuf, usize)>,
    annotated: &HashSet<String>,
) -> Vec<AnalyzedDeclaration> {
    // 型ごとにメソッドを集約。
    let mut methods_by_type: HashMap<&str, Vec<&MethodInfo>> = HashMap::new();
    for imp in impls {
        methods_by_type
            .entry(imp.type_name.as_str())
            .or_default()
            .extend(imp.methods.iter());
    }
    let mut out = Vec::new();
    for (ty, methods) in methods_by_type {
        if annotated.contains(ty) {
            continue;
        }
        let Some((path, line)) = type_sites.get(ty) else {
            continue; // 宣言サイト不明（外部型など）はアンカーできないので skip。
        };
        if let Some(decl) = infer_one(ty, &methods, path.clone(), *line) {
            out.push(decl);
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
    out
}

fn infer_one(ty: &str, methods: &[&MethodInfo], path: PathBuf, line: usize) -> Option<AnalyzedDeclaration> {
    let op = pick(methods, OP_NAMES, |m| is_binary_op(m, ty))?;
    let id = pick(methods, ID_NAMES, |m| is_identity(m, ty));
    let inv = pick(methods, INV_NAMES, |m| is_inverse(m, ty));

    let structure = match (id.is_some(), inv.is_some()) {
        (true, true) => AlgebraicStructure::Group,
        (true, false) => AlgebraicStructure::Monoid,
        _ => AlgebraicStructure::Semigroup, // 演算のみ（逆元だけで単位元無しも Semigroup 扱い）
    };
    let identity_name = (structure.rank() >= 2).then(|| id.unwrap().name.clone());
    let inverse_name = (structure.rank() >= 3).then(|| inv.unwrap().name.clone());

    Some(AnalyzedDeclaration {
        target_structure: structure,
        higher_kinded: None,
        type_name: ty.to_string(),
        operation_name: op.name.clone(),
        identity_name,
        inverse_name,
        path,
        line,
        propagation: None,
    })
}

/// 名前優先リスト順に、述語を満たす最初のメソッドを選ぶ。無ければ述語を満たす任意の先頭。
fn pick<'a>(
    methods: &[&'a MethodInfo],
    priority: &[&str],
    pred: impl Fn(&MethodInfo) -> bool,
) -> Option<&'a MethodInfo> {
    for &name in priority {
        if let Some(m) = methods.iter().find(|m| m.name == name && pred(m)) {
            return Some(m);
        }
    }
    None
}

/// 閉じた二項演算か: `fn f(self|&self, other: T) -> T` または `fn f(a: T, b: T) -> T`。
fn is_binary_op(m: &MethodInfo, ty: &str) -> bool {
    if m.self_param == Some(SelfKind::MutRef) {
        return false; // 破壊的演算は値の演算ではない。
    }
    if !ret_is(m, ty) {
        return false;
    }
    if m.self_param.is_some() {
        // self + 1 引数（引数が T）。
        m.params.len() == 1 && type_is(&m.params[0], ty)
    } else if m.is_assoc_fn {
        // 関連関数 op(T, T) -> T。
        m.params.len() == 2 && m.params.iter().all(|p| type_is(p, ty))
    } else {
        false
    }
}

/// 単位元か: 引数0で `-> T` を返す関連関数。
fn is_identity(m: &MethodInfo, ty: &str) -> bool {
    m.self_param.is_none() && m.params.is_empty() && ret_is(m, ty)
}

/// 逆元か: `-> T` を返す（self ありでも関連関数でも可）。
fn is_inverse(m: &MethodInfo, ty: &str) -> bool {
    ret_is(m, ty) && m.self_param != Some(SelfKind::MutRef)
}

fn strip_refs(s: &str) -> &str {
    let mut s = s.trim();
    loop {
        if let Some(r) = s.strip_prefix('&') {
            s = r.trim_start();
        } else if let Some(r) = s.strip_prefix("mut ") {
            s = r.trim_start();
        } else {
            break;
        }
    }
    s
}

fn type_is(s: &str, ty: &str) -> bool {
    let s = strip_refs(s).trim();
    // `Self` と、演算子トレイトが返す `Self::Output` は当該型とみなす。
    if s == "Self" || s == "Self::Output" {
        return true;
    }
    // ジェネリック引数を落として基底型名で照合する（`Vector2D<T, U>` → `Vector2D`）。
    // konpu は型を基底名で扱う（impl_type_name も同様）ので整合する。
    let base = s.split('<').next().unwrap_or(s).trim();
    if base.contains(['[', '(', ',', ' ']) {
        return false;
    }
    base.rsplit("::").next().unwrap_or(base) == ty
}

fn ret_is(m: &MethodInfo, ty: &str) -> bool {
    m.return_type.as_deref().is_some_and(|r| type_is(r, ty))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn method(name: &str, self_param: Option<SelfKind>, params: &[&str], ret: Option<&str>, is_assoc_fn: bool) -> MethodInfo {
        MethodInfo {
            name: name.to_string(),
            self_param,
            params: params.iter().map(|s| s.to_string()).collect(),
            return_type: ret.map(str::to_string),
            is_assoc_fn,
        }
    }

    fn infer(ty: &str, methods: Vec<MethodInfo>) -> Option<AnalyzedDeclaration> {
        let refs: Vec<&MethodInfo> = methods.iter().collect();
        infer_one(ty, &refs, PathBuf::from("src/x.rs"), 1)
    }

    #[test]
    fn monoid_from_combine_and_empty() {
        let d = infer("Money", vec![
            method("combine", Some(SelfKind::Owned), &["Self"], Some("Self"), false),
            method("empty", None, &[], Some("Self"), true),
        ]).unwrap();
        assert_eq!(d.target_structure, AlgebraicStructure::Monoid);
        assert_eq!(d.operation_name, "combine");
        assert_eq!(d.identity_name.as_deref(), Some("empty"));
        assert!(d.inverse_name.is_none());
    }

    #[test]
    fn semigroup_from_op_only() {
        let d = infer("S", vec![
            method("merge", Some(SelfKind::Ref), &["&Self"], Some("Self"), false),
        ]).unwrap();
        assert_eq!(d.target_structure, AlgebraicStructure::Semigroup);
        assert_eq!(d.operation_name, "merge");
        assert!(d.identity_name.is_none());
    }

    #[test]
    fn group_from_add_zero_neg() {
        let d = infer("V", vec![
            method("add", Some(SelfKind::Owned), &["Self"], Some("Self"), false),
            method("zero", None, &[], Some("Self"), true),
            method("neg", Some(SelfKind::Owned), &[], Some("Self"), false),
        ]).unwrap();
        assert_eq!(d.target_structure, AlgebraicStructure::Group);
        assert_eq!(d.inverse_name.as_deref(), Some("neg"));
    }

    #[test]
    fn builder_not_an_op() {
        // fn with_name(self, name: String) -> Self — closed? no, param isn't Self.
        let d = infer("Builder", vec![
            method("with_name", Some(SelfKind::Owned), &["String"], Some("Self"), false),
        ]);
        assert!(d.is_none());
    }

    #[test]
    fn no_op_no_decl() {
        let d = infer("Plain", vec![
            method("len", Some(SelfKind::Ref), &[], Some("usize"), false),
        ]);
        assert!(d.is_none());
    }

    #[test]
    fn operator_trait_self_output_and_generics() {
        // impl Add/Neg for Vector<T,U> { fn add(self, other: Self) -> Self::Output; fn neg(self) -> Self::Output }
        // plus a generic-typed param and a `zero()` -> Vector<T,U>.
        let d = infer("Vector", vec![
            method("add", Some(SelfKind::Owned), &["Self"], Some("Self::Output"), false),
            method("neg", Some(SelfKind::Owned), &[], Some("Self::Output"), false),
            method("zero", None, &[], Some("Vector<T, U>"), true),
        ]).unwrap();
        assert_eq!(d.target_structure, AlgebraicStructure::Group);
        assert_eq!(d.operation_name, "add");
        assert_eq!(d.inverse_name.as_deref(), Some("neg"));
    }

    #[test]
    fn assoc_fn_binary_op() {
        // fn op(a: Self, b: Self) -> Self
        let d = infer("M", vec![
            method("op", None, &["Self", "Self"], Some("Self"), true),
        ]).unwrap();
        assert_eq!(d.operation_name, "op");
        assert_eq!(d.target_structure, AlgebraicStructure::Semigroup);
    }
}
