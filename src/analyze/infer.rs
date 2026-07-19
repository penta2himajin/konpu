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

/// 演算の「族」: 二項演算と、その族に整合する単位元/逆元の慣用名。
/// op/identity/inverse を独立に拾うと min+zero+neg のような非整合な組み合わせを
/// 捏造してしまうため、identity/inverse は選んだ op の族からのみ拾う。
/// 非結合の `sub`/`div` は代数構造の演算ではないので含めない。
struct OpFamily {
    ops: &'static [&'static str],
    ids: &'static [&'static str],
    invs: &'static [&'static str],
}

/// 族の並びと各 `ops` の並びが優先順位。add/mul を汎用モノイドより優先。
const FAMILIES: &[OpFamily] = &[
    // 各族は正準名（add→zero）に加え、汎用名（identity/unit/inverse）も許す。
    OpFamily {
        ops: &["add"],
        ids: &["zero", "identity", "unit", "neutral"],
        invs: &["neg", "negate", "inverse", "inv"],
    },
    OpFamily {
        ops: &["mul"],
        ids: &["one", "identity", "unit", "neutral"],
        invs: &["recip", "inv", "inverse", "negate"],
    },
    OpFamily {
        ops: &["combine", "merge", "append", "concat", "mappend", "op", "compose", "then", "join", "meet", "and", "or", "bitand", "bitor", "bitxor"],
        ids: &["identity", "empty", "unit", "neutral", "zero", "one", "default"],
        invs: &["inverse", "inv", "negate", "neg"],
    },
    // 半束（min/max）: 結合的だが型内に慣用の単位元/逆元は無い → Semigroup 止まり。
    OpFamily { ops: &["min", "max"], ids: &[], invs: &[] },
];

/// 全 impl と型サイトから、代数構造を推論した宣言を返す。
/// `annotated` に既にある型はスキップ（アノテーション優先）。
/// `free_fns` は impl 外のモジュール関数。戻り型で型に帰属させ、単位元候補に加える
/// （oxidtr は receiver なし演算を自由関数として出すため）。
pub fn infer_declarations(
    impls: &[ImplInfo],
    free_fns: &[MethodInfo],
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
    // 自由関数を戻り型（基底名）で該当型のプールに足す。名前＋シグネチャが族に
    // 一致したものだけが拾われる（`fn zero() -> Money` は単位元、無関係な
    // `fn make() -> Money` はどの族名にも一致せず無視される）。
    // ponytail: is_inverse は引数を見ないので、逆元族名の無引数自由関数が
    // 逆元候補になりうる。oxidtr がそれを出す形は無く実害は見ていない。
    for f in free_fns {
        if let Some(base) = ret_base_name(f) {
            if let Some(bucket) = methods_by_type.get_mut(base.as_str()) {
                bucket.push(f);
            }
        }
    }
    let mut out = Vec::new();
    for (ty, methods) in methods_by_type {
        if annotated.contains(ty) {
            continue;
        }
        let Some((path, line)) = type_sites.get(ty) else {
            continue; // 宣言サイト不明（外部型など）はアンカーできないので skip。
        };
        out.extend(infer_all(ty, &methods, path.clone(), *line));
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
    out
}

/// 型 `ty` の各演算族について、閉じた二項演算を持つ族ごとに1宣言を出す。
/// リング型（加法 Group + 乗法 Monoid）のように、1つの型が複数の代数構造を
/// 持つ場合は複数の宣言を返す。ops が全族で互いに素なので二重計上は起きない。
fn infer_all(ty: &str, methods: &[&MethodInfo], path: PathBuf, line: usize) -> Vec<AnalyzedDeclaration> {
    FAMILIES
        .iter()
        .filter_map(|fam| {
            let op = fam.ops.iter().find_map(|&name| pick(methods, name, |m| is_binary_op(m, ty)))?;
            // identity / inverse は選んだ族の慣用名からのみ拾う（非整合な組み合わせを防ぐ）。
            let id = fam.ids.iter().find_map(|&name| pick(methods, name, |m| is_identity(m, ty)));
            let inv = fam.invs.iter().find_map(|&name| pick(methods, name, |m| is_inverse(m, ty)));

            let structure = match (id.is_some(), inv.is_some()) {
                (true, true) => AlgebraicStructure::Group,
                (true, false) => AlgebraicStructure::Monoid,
                _ => AlgebraicStructure::Semigroup, // 演算のみ（単位元無しの逆元は無視）
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
                path: path.clone(),
                line,
                propagation: None,
            })
        })
        .collect()
}

/// 名前が一致し述語を満たす最初のメソッドを返す。
fn pick<'a>(methods: &[&'a MethodInfo], name: &str, pred: impl Fn(&MethodInfo) -> bool) -> Option<&'a MethodInfo> {
    methods.iter().find(|m| m.name == name && pred(m)).copied()
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
    if s == "Self" {
        return true;
    }
    // 演算子トレイトの `Self::Output` は抽出時に実型へ解決済み（extract::parse_impl）。
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

/// メソッドの戻り型の基底名（参照・ジェネリクス・パスを剥がす）。
/// 自由関数を戻り型で型に帰属させる際のキー。`Self` は自由関数には現れない。
fn ret_base_name(m: &MethodInfo) -> Option<String> {
    let s = strip_refs(m.return_type.as_deref()?);
    let base = s.split('<').next().unwrap_or(s).trim();
    if base.is_empty() || base.contains(['[', '(', ',', ' ']) {
        return None;
    }
    Some(base.rsplit("::").next().unwrap_or(base).to_string())
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
            impure: false,
        }
    }

    fn infer_all_t(ty: &str, methods: Vec<MethodInfo>) -> Vec<AnalyzedDeclaration> {
        let refs: Vec<&MethodInfo> = methods.iter().collect();
        infer_all(ty, &refs, PathBuf::from("src/x.rs"), 1)
    }

    fn infer(ty: &str, methods: Vec<MethodInfo>) -> Option<AnalyzedDeclaration> {
        infer_all_t(ty, methods).into_iter().next()
    }

    #[test]
    fn ring_yields_additive_group_and_multiplicative_monoid() {
        // A ring-like type: additive Group (add/zero/neg) AND multiplicative
        // Monoid (mul/one). Both structures must be inferred, not just the first.
        let decls = infer_all_t("R", vec![
            method("add", Some(SelfKind::Owned), &["Self"], Some("Self"), false),
            method("zero", None, &[], Some("Self"), true),
            method("neg", Some(SelfKind::Owned), &[], Some("Self"), false),
            method("mul", Some(SelfKind::Owned), &["Self"], Some("Self"), false),
            method("one", None, &[], Some("Self"), true),
        ]);
        assert_eq!(decls.len(), 2);
        let add = decls.iter().find(|d| d.operation_name == "add").unwrap();
        assert_eq!(add.target_structure, AlgebraicStructure::Group);
        assert_eq!(add.inverse_name.as_deref(), Some("neg"));
        let mul = decls.iter().find(|d| d.operation_name == "mul").unwrap();
        assert_eq!(mul.target_structure, AlgebraicStructure::Monoid);
        assert_eq!(mul.identity_name.as_deref(), Some("one"));
        assert!(mul.inverse_name.is_none());
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
    fn generic_base_matching() {
        // After extraction resolves Self::Output, methods carry concrete generic
        // types; base-name matching (Vector<T,U> -> Vector) must still detect them.
        let d = infer("Vector", vec![
            method("add", Some(SelfKind::Owned), &["Vector<T, U>"], Some("Vector<T, U>"), false),
            method("neg", Some(SelfKind::Owned), &[], Some("Vector<T, U>"), false),
            method("zero", None, &[], Some("Vector<T, U>"), true),
        ]).unwrap();
        assert_eq!(d.target_structure, AlgebraicStructure::Group);
        assert_eq!(d.operation_name, "add");
        assert_eq!(d.inverse_name.as_deref(), Some("neg"));
    }

    #[test]
    fn min_op_does_not_borrow_zero_and_neg() {
        // Point-like: only min/max are closed (add returns via Vector etc.).
        // min belongs to the semilattice family which has no identity/inverse,
        // so zero/neg must NOT be attached -> Semigroup, not a bogus Group.
        let d = infer("Point", vec![
            method("min", Some(SelfKind::Owned), &["Self"], Some("Self"), false),
            method("max", Some(SelfKind::Owned), &["Self"], Some("Self"), false),
            method("zero", None, &[], Some("Self"), true),
            method("neg", Some(SelfKind::Owned), &[], Some("Self"), false),
        ]).unwrap();
        assert_eq!(d.target_structure, AlgebraicStructure::Semigroup);
        assert_eq!(d.operation_name, "min");
        assert!(d.identity_name.is_none());
        assert!(d.inverse_name.is_none());
    }

    #[test]
    fn sub_is_not_an_op() {
        // subtraction is non-associative -> not an algebraic-structure op.
        let d = infer("X", vec![
            method("sub", Some(SelfKind::Owned), &["Self"], Some("Self"), false),
        ]);
        assert!(d.is_none());
    }

    #[test]
    fn free_fn_identity_lifts_semigroup_to_monoid() {
        // oxidtr shape: combine is a receiver method in `impl Money`, but the
        // identity `zero() -> Money` is a module-level free fn (operations.rs).
        // infer_declarations must attribute the free fn by return type and
        // treat it as the identity -> Monoid, not Semigroup.
        use super::super::extract::ImplInfo;
        let impls = vec![ImplInfo {
            type_name: "Money".to_string(),
            methods: vec![method("combine", Some(SelfKind::Ref), &["&Money"], Some("Money"), false)],
        }];
        let free_fns = vec![method("zero", None, &[], Some("Money"), true)];
        let mut sites = HashMap::new();
        sites.insert("Money".to_string(), (PathBuf::from("src/x.rs"), 1));
        let decls = infer_declarations(&impls, &free_fns, &sites, &HashSet::new());
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].target_structure, AlgebraicStructure::Monoid);
        assert_eq!(decls[0].operation_name, "combine");
        assert_eq!(decls[0].identity_name.as_deref(), Some("zero"));
    }

    #[test]
    fn free_fn_of_unrelated_type_is_ignored() {
        // A free fn returning a type with no impl must NOT create a decl.
        use super::super::extract::ImplInfo;
        let impls = vec![ImplInfo {
            type_name: "Money".to_string(),
            methods: vec![method("combine", Some(SelfKind::Ref), &["&Money"], Some("Money"), false)],
        }];
        // `empty() -> Other` is irrelevant to Money; Money stays Semigroup.
        let free_fns = vec![method("empty", None, &[], Some("Other"), true)];
        let mut sites = HashMap::new();
        sites.insert("Money".to_string(), (PathBuf::from("src/x.rs"), 1));
        let decls = infer_declarations(&impls, &free_fns, &sites, &HashSet::new());
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].target_structure, AlgebraicStructure::Semigroup);
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

