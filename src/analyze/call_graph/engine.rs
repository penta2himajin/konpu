//! Swift/Kotlin/TS の tree-sitter CG 抽出器が共有する意味論コア。
//!
//! 3 言語の抽出器は「grammar walk（どのノードをどう辿るか）」だけが違い、
//! 「どう解決するか」— 索引の形・受け手型の解決順序・Static/Dynamic/External の
//! 落とし方・2 パス駆動・関数登録 — は同一モデルの複製だった。後者をここに一本化する。
//! grammar walk（collect_funcs/collect_calls の分岐、シグネチャ抽出）は各言語ファイル
//! に残す — そこは言語ごとに本当に違い、hook で抽象化すると grammar 差が読めなくなる
//! （kotlin.rs の旧 ponytail 注記の通り。3 言語目で複製が semantics まで及んだので
//! semantics だけ抽出した）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tree_sitter::Node;

use konpu_cg::{CallSite, CallTargetKind, Facts, FuncId, ImplEntry, TraitMethod};

use super::FnSig;
use crate::analyze::parser::{self, Language};
use crate::analyze::template::ResolvedConfig;

/// Pass 2 で不変な参照（node.id()→FuncId と精密解決索引）を束ねる。
pub(super) struct Resolver<'a> {
    pub(super) ids: &'a HashMap<usize, FuncId>,
    pub(super) index: &'a Index,
}

/// 精密な呼び出し解決のための索引。
#[derive(Default)]
pub(super) struct Index {
    /// (型, メソッド名) -> 候補 FuncId 群（同名オーバーロードは複数）。
    pub(super) type_methods: HashMap<(String, String), Vec<FuncId>>,
    /// 自由関数名 -> FuncId 群。
    pub(super) free_funcs: HashMap<String, Vec<FuncId>>,
    /// 型 -> (ストアドプロパティ名 -> 基底型名)。受け手が field のとき型解決に使う。
    pub(super) fields: HashMap<String, HashMap<String, String>>,
}

pub(super) enum Resolution {
    /// 型が解決でき、そのメソッドに厳密に結んだ（同名オーバーロードは複数）。
    Targets(Vec<FuncId>),
    /// 型未解決 → 同名メソッド全てに繋ぐ過大近似（偽陰性を出さない）。
    Dynamic(String),
    /// 受け手の型は具体解決できたが index に無い（外部ライブラリ型/継承）→ エッジ無し。
    /// Dynamic で全同名に繋ぐより精密（受け手型が判っている以上、他の自型メソッドではない）。
    External,
}

/// 型文字列の基底名（`A?`→A、`Foo<T>`→Foo、`a.b.C`→C）。
/// TS の型は `?`/`!` で終わらないので suffix 剥がしは無害に共通化できる。
pub(super) fn base_type_name(s: &str) -> String {
    let mut s = s.trim().trim_end_matches(['?', '!']).trim();
    if let Some(i) = s.find('<') {
        s = s[..i].trim();
    }
    s.rsplit(['.', ':']).next().unwrap_or(s).trim().to_string()
}

pub(super) fn text_of<'a>(n: Node, source: &'a str) -> &'a str {
    n.utf8_text(source.as_bytes()).unwrap_or("")
}

pub(super) fn is_pascal_case(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_uppercase())
}

pub(super) fn first_child_of_kind<'a>(n: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cur = n.walk();
    n.children(&mut cur).find(|c| c.kind() == kind)
}

pub(super) fn last_child_of_kind<'a>(n: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cur = n.walk();
    n.children(&mut cur).filter(|c| c.kind() == kind).last()
}

pub(super) fn first_named_child(n: Node) -> Option<Node> {
    let mut cur = n.walk();
    n.children(&mut cur).find(|c| c.is_named())
}

pub(super) fn lookup_method(index: &Index, ty: &str, method: &str) -> Resolution {
    match index.type_methods.get(&(ty.to_string(), method.to_string())) {
        Some(ids) => Resolution::Targets(ids.clone()),
        None => Resolution::External,
    }
}

/// 受け手なしの呼び出し: 内包型のメソッド（暗黙 self）→ 自由関数 → Dynamic。
pub(super) fn resolve_bare(index: &Index, enclosing: Option<&str>, name: &str) -> Resolution {
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

/// 平坦な受け手基底識別子の型解決（Swift/Kotlin）: self → 内包型、PascalCase → 型
/// そのもの（静的呼び）、それ以外 → ローカル → 内包型のフィールド。
/// TS はフィールドが常に `this.` 修飾でネストするため専用の再帰解決を持つ。
pub(super) fn recv_type(
    base: &str,
    enclosing: Option<&str>,
    locals: &HashMap<String, String>,
    index: &Index,
) -> Option<String> {
    if base == "self" {
        enclosing.map(str::to_string)
    } else if is_pascal_case(base) {
        Some(base.to_string())
    } else {
        locals
            .get(base)
            .cloned()
            .or_else(|| enclosing.and_then(|t| index.fields.get(t)).and_then(|f| f.get(base)).cloned())
    }
}

/// 解決結果をエッジ化して facts に足す。
pub(super) fn push_resolution(resolved: Resolution, caller: Option<FuncId>, facts: &mut Facts) {
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
        Resolution::External => {}
    }
}

/// 関数定義 1 件の登録: qualified 名で facts に追加し、impl（CHA/RTA 用）と
/// 精密解決索引（型メソッド or 自由関数）に載せる。
pub(super) fn register_func(
    bare: &str,
    node: Node,
    fpath: &Path,
    enclosing: Option<&str>,
    facts: &mut Facts,
    ids: &mut HashMap<usize, FuncId>,
    index: &mut Index,
) {
    let name = match enclosing {
        Some(t) => format!("{t}.{bare}"),
        None => bare.to_string(),
    };
    let id = facts.add_func(name, fpath.to_path_buf(), node.start_position().row + 1);
    ids.insert(node.id(), id);
    facts.impls.push(ImplEntry {
        trait_method: TraitMethod::new("", bare.to_string()),
        for_type: enclosing.unwrap_or("").to_string(),
        func: id,
    });
    match enclosing {
        Some(t) => index.type_methods.entry((t.to_string(), bare.to_string())).or_default().push(id),
        None => index.free_funcs.entry(bare.to_string()).or_default().push(id),
    }
}

/// collect_base_idents の言語差（ノード種別）を設定に出した共通 walker。
pub(super) struct IdentKinds {
    /// 参照ではない部分木（引数ラベル・navigation/member の末尾）。降りない。
    pub(super) skip: &'static [&'static str],
    /// self 参照ノード（"self" として集める）。
    pub(super) self_kind: &'static str,
    /// 識別子ノード。
    pub(super) ident: &'static str,
}

/// 式木の基底識別子（`a.x` → "a"、self/this）を重複なく集める。
pub(super) fn collect_base_idents(k: &IdentKinds, n: Node, source: &str, out: &mut Vec<String>) {
    let kind = n.kind();
    if k.skip.contains(&kind) {
        return;
    }
    if kind == k.self_kind {
        if !out.iter().any(|s| s == "self") {
            out.push("self".to_string());
        }
        return;
    }
    if kind == k.ident {
        let t = text_of(n, source).trim().to_string();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    let mut cur = n.walk();
    for child in n.children(&mut cur) {
        collect_base_idents(k, child, source, out);
    }
}

/// プロジェクトから対象言語のソースを集める（exclude 尊重・ルート相対パス）。
/// パスを相対にするのは preserve の `to`/`from` glob が SCIP 同様に相対前提のため。
pub(super) fn project_sources(path: &Path, config: &ResolvedConfig, lang: Language) -> Vec<(PathBuf, String)> {
    parser::collect_source_files(path)
        .into_iter()
        .filter(|(_, l)| *l == lang)
        .filter(|(f, _)| !config.is_excluded(f, path))
        .filter_map(|(f, _)| {
            let rel = f.strip_prefix(path).unwrap_or(&f).to_path_buf();
            std::fs::read_to_string(&f).ok().map(|s| (rel, s))
        })
        .collect()
}

/// 2 パス駆動: Pass 1 で関数定義を登録して索引を作り、Pass 2 で呼び出しをエッジ化する。
/// `collect_funcs`/`collect_calls` は各言語の grammar walk（root から）。
pub(super) fn facts_from_sources(
    lang: Language,
    sources: Vec<(PathBuf, String)>,
    collect_funcs: impl Fn(Node, &str, &Path, &mut Facts, &mut HashMap<usize, FuncId>, &mut Index),
    collect_calls: impl Fn(Node, &str, &Path, &Resolver, &mut Facts),
) -> Facts {
    let parsed: Vec<(PathBuf, String, tree_sitter::Tree)> = sources
        .into_iter()
        .filter_map(|(f, src)| {
            let tree = parser::parse_with(&src, lang)?;
            Some((f, src, tree))
        })
        .collect();

    let mut facts = Facts::default();
    // 自由関数（for_type ""）は RTA でも常に残す。
    facts.instantiated.insert(String::new());

    let mut index = Index::default();
    let mut fn_ids: Vec<HashMap<usize, FuncId>> = Vec::with_capacity(parsed.len());
    for (fpath, src, tree) in &parsed {
        let mut ids = HashMap::new();
        collect_funcs(tree.root_node(), src, fpath, &mut facts, &mut ids, &mut index);
        fn_ids.push(ids);
    }
    for (fi, (fpath, src, tree)) in parsed.iter().enumerate() {
        let r = Resolver { ids: &fn_ids[fi], index: &index };
        collect_calls(tree.root_node(), src, fpath, &r, &mut facts);
    }
    facts
}

/// 関数シグネチャ収集の駆動（preserve 検査 B/C 用）。`walk` は各言語の grammar walk。
pub(super) fn fn_signatures(
    path: &Path,
    config: &ResolvedConfig,
    lang: Language,
    walk: impl Fn(Node, &str, &Path, &mut Vec<FnSig>),
) -> Vec<FnSig> {
    let mut out = Vec::new();
    for (f, l) in parser::collect_source_files(path) {
        if l != lang || config.is_excluded(&f, path) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&f) else { continue };
        let Some(tree) = parser::parse_with(&src, lang) else { continue };
        walk(tree.root_node(), &src, &f, &mut out);
    }
    out
}
