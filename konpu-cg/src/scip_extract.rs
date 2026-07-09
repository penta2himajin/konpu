//! SCIP インデックス (`rust-analyzer scip`) から `Facts` を構築する抽出器。
//!
//! 設計 (docs/layer2-call-graph-design.md §3,§5): 事実抽出層。rust-analyzer が
//! 意味解析済みで吐く SCIP シンボルから、関数・呼び出しサイト・trait 実装・
//! インスタンス化型を取り出す。ディスパッチ解釈 (CHA/RTA) は `graph` に委ねる。
//!
//! SCIP シンボル符号化 (rust-analyzer 1.96 で確認):
//! - 自由関数:              `total().`
//! - 継承 impl メソッド:    `impl#[Type]method().`
//! - trait メソッド宣言:    `Trait#method().`   ← 動的ディスパッチのアンカー
//! - trait 実装メソッド:    `impl#[Type][Trait]method().`
//! - 型定義:                `Type#`
//! - 定義 occurrence は `symbol_roles & 1 == 1`、本体行範囲は `enclosing_range`。
//! - 動的呼び出し `x.m()` (x: dyn Trait) は trait 宣言シンボルを参照する。

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use protobuf::Message;
use scip::types::{Index, Occurrence};

use crate::facts::{CallSite, CallTargetKind, Facts, FuncId, ImplEntry, TraitMethod};

/// `SymbolRole::Definition` のビット。
const DEFINITION: i32 = 1;

/// SCIP index ファイルを読んで `Facts` を構築する。
pub fn facts_from_scip_file(path: &Path) -> io::Result<Facts> {
    let data = std::fs::read(path)?;
    let idx = Index::parse_from_bytes(&data)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(facts_from_index(&idx))
}

/// `rust-analyzer scip` をプロジェクトに対して実行し、`Facts` を構築する。
/// `rust-analyzer` が PATH 上に必要。インデックスは一時ファイルに書いて後で消す。
pub fn facts_from_project(project_dir: &Path) -> io::Result<Facts> {
    let out = std::env::temp_dir().join(format!("konpu-cg-{}.scip", std::process::id()));
    // rust-analyzer の進捗/内部 WARN は stderr に大量に出るので握り潰す。
    // 失敗は終了コードで検知する。
    let status = std::process::Command::new("rust-analyzer")
        .arg("scip")
        .arg(project_dir)
        .arg("--output")
        .arg(&out)
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(io::Error::other(
            "rust-analyzer scip failed (is rust-analyzer on PATH?)",
        ));
    }
    let facts = facts_from_scip_file(&out);
    let _ = std::fs::remove_file(&out);
    facts
}

/// パース済み SCIP index から `Facts` を構築する (ファイル I/O 非依存、テスト用)。
pub fn facts_from_index(idx: &Index) -> Facts {
    let mut facts = Facts::default();
    // シンボル文字列 -> FuncId (ローカル定義のみ)。
    let mut sym_to_func: HashMap<String, FuncId> = HashMap::new();
    // trait メソッド宣言シンボル -> TraitMethod (動的ディスパッチ判定用)。
    let mut trait_decl: HashMap<String, TraitMethod> = HashMap::new();
    // 型定義シンボル -> 型名 (RTA のインスタンス化判定用)。
    let mut type_sym: HashMap<String, String> = HashMap::new();
    // ドキュメントごとの (FuncId, 本体開始行, 本体終了行)。caller 特定に使う。
    let mut doc_defs: Vec<Vec<(FuncId, i32, i32)>> = vec![Vec::new(); idx.documents.len()];

    // Pass 1: ローカル定義を登録する。
    for (di, doc) in idx.documents.iter().enumerate() {
        for occ in &doc.occurrences {
            if occ.symbol_roles & DEFINITION == 0 {
                continue;
            }
            let Some(desc) = local_desc(&occ.symbol) else {
                continue;
            };
            // 型定義: RTA 用に記録。
            if let Some(ty) = as_type_name(desc) {
                type_sym.insert(occ.symbol.clone(), ty);
                continue;
            }
            if !desc.ends_with("().") {
                continue;
            }
            let (line, span_start, span_end) = def_span(occ);
            let id = facts.add_func(display_name(desc), PathBuf::from(&doc.relative_path), (line + 1) as usize);
            sym_to_func.insert(occ.symbol.clone(), id);
            doc_defs[di].push((id, span_start, span_end));

            if let Some((trait_name, for_type, method)) = as_impl_trait_method(desc) {
                facts.impls.push(ImplEntry {
                    trait_method: TraitMethod::new(trait_name, method),
                    for_type,
                    func: id,
                });
            } else if let Some(tm) = as_trait_method_decl(desc) {
                trait_decl.insert(occ.symbol.clone(), tm);
            }
        }
    }

    // Pass 2: 参照 occurrence をコールエッジ / インスタンス化型に変換する。
    for (di, doc) in idx.documents.iter().enumerate() {
        for occ in &doc.occurrences {
            if occ.symbol_roles & DEFINITION != 0 {
                continue; // 定義そのものは呼び出しではない。
            }
            // 型参照 -> RTA のインスタンス化集合 (使用を過大近似; 純粋な型注釈も含む)。
            if let Some(ty) = type_sym.get(&occ.symbol) {
                facts.instantiated.insert(ty.clone());
            }
            let ref_line = occ.range.first().copied().unwrap_or(0);
            let Some(caller) = enclosing_def(&doc_defs[di], ref_line) else {
                continue; // 関数本体の外 (import 等) は無視。
            };
            let target = if let Some(tm) = trait_decl.get(&occ.symbol) {
                CallTargetKind::Dynamic(tm.clone())
            } else if let Some(&callee) = sym_to_func.get(&occ.symbol) {
                CallTargetKind::Static(callee)
            } else {
                continue; // 外部関数への呼び出しは追跡しない。
            };
            facts.calls.push(CallSite { caller, target });
        }
    }

    // trait 名は具体型ではない (RTA の for_type には現れない) ので instantiated から除く。
    // dyn Trait / trait 境界としての参照が紛れ込むための掃除で、モデルの正確さのため。
    let trait_names: std::collections::HashSet<String> = facts
        .impls
        .iter()
        .map(|e| e.trait_method.trait_name.clone())
        .chain(trait_decl.values().map(|tm| tm.trait_name.clone()))
        .collect();
    facts.instantiated.retain(|t| !trait_names.contains(t));

    // RTA 精度の天井 (docs/layer2-call-graph-design.md §6):
    // rust-analyzer の SCIP は occurrence の value/type 位置も read/write role も
    // syntax_kind も出さない (実測 role=0, syntax_kind=Unspecified)。よって「構築」と
    // 「型言及」を区別できない。とくに `impl Trait for T` のヘッダが必ず `T#` 参照を
    // 生むため、trait 実装を持つ型は自身の impl ヘッダだけで instantiated 入りし、RTA は
    // 実 SCIP 上で CHA に縮退する。健全 (偽陰性なし) を保ったままの精緻化は不可能で、
    // 真の RTA には MIR 単相化レベルの構築事実 (StableMIR) が要る。現状は健全な過大近似。
    facts
}

// ---- SCIP シンボル/範囲パーサ (regex 非依存) ----

/// ローカル (ワークスペース) シンボルの descriptor 部分を返す。
/// 形式 `rust-analyzer cargo <pkg> <ver> <descriptors>`。ローカルの descriptor は
/// 空白を含まない (外部 URL パッケージやジェネリクスは空白を含み弾かれる)。
fn local_desc(sym: &str) -> Option<&str> {
    let rest = sym.strip_prefix("rust-analyzer cargo ")?;
    let mut it = rest.splitn(3, ' ');
    let _pkg = it.next()?;
    let _ver = it.next()?;
    let desc = it.next()?;
    if desc.contains(' ') {
        return None;
    }
    Some(desc)
}

/// `Type#` -> "Type" (型定義)。namespace 接頭辞は落とす。impl/メソッドは除外。
fn as_type_name(desc: &str) -> Option<String> {
    let body = desc.strip_suffix('#')?;
    if body.contains('#') || body.contains('[') || body.contains('(') {
        return None;
    }
    let name = body.rsplit('/').next()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// `Trait#method().` -> TraitMethod (trait メソッド宣言; `impl#` を含まない)。
fn as_trait_method_decl(desc: &str) -> Option<TraitMethod> {
    let body = desc.strip_suffix("().")?;
    if body.contains("impl#") {
        return None;
    }
    let (before, method) = body.rsplit_once('#')?;
    let trait_name = before.rsplit('/').next()?;
    if trait_name.is_empty()
        || method.is_empty()
        || method.contains(['#', '[', ']', '/'])
    {
        return None;
    }
    Some(TraitMethod::new(trait_name, method))
}

/// `impl#[Type][Trait]method().` -> (trait, for_type, method)。
/// 継承 impl (`impl#[Type]method().`, ブラケット 1 つ) は None。
fn as_impl_trait_method(desc: &str) -> Option<(String, String, String)> {
    let body = desc.strip_suffix("().")?;
    let idx = body.find("impl#[")?;
    let after = &body[idx + "impl#[".len()..];
    let (for_type, rest) = after.split_once(']')?;
    let rest = rest.strip_prefix('[')?;
    let (trait_name, method) = rest.split_once(']')?;
    if for_type.is_empty()
        || trait_name.is_empty()
        || method.is_empty()
        || method.contains(['[', ']'])
    {
        return None;
    }
    Some((trait_name.to_string(), for_type.to_string(), method.to_string()))
}

/// 表示名: descriptor から末尾 `().` を落としたもの。
fn display_name(desc: &str) -> String {
    desc.strip_suffix("().").unwrap_or(desc).to_string()
}

/// SCIP range の (開始行, 終了行)。3 要素=単一行 [line,sc,ec]、4 要素=複数行 [sl,sc,el,ec]。
fn range_lines(r: &[i32]) -> (i32, i32) {
    match r.len() {
        3 => (r[0], r[0]),
        n if n >= 4 => (r[0], r[2]),
        _ => (0, 0),
    }
}

/// 定義 occurrence の (表示行, 本体開始行, 本体終了行)。
fn def_span(occ: &Occurrence) -> (i32, i32, i32) {
    let line = occ.range.first().copied().unwrap_or(0);
    if occ.enclosing_range.is_empty() {
        (line, line, line)
    } else {
        let (s, e) = range_lines(&occ.enclosing_range);
        (line, s, e)
    }
}

/// ある行を本体に含む定義のうち、最も範囲の狭いものを caller とする。
fn enclosing_def(defs: &[(FuncId, i32, i32)], line: i32) -> Option<FuncId> {
    defs.iter()
        .filter(|(_, s, e)| *s <= line && line <= *e)
        .min_by_key(|(_, s, e)| e - s)
        .map(|(f, _, _)| *f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{CallGraph, Precision};
    use scip::types::Document;

    fn occ(symbol: &str, role: i32, range: Vec<i32>, encl: Vec<i32>) -> Occurrence {
        Occurrence {
            range,
            symbol: symbol.to_string(),
            symbol_roles: role,
            enclosing_range: encl,
            ..Default::default()
        }
    }

    fn sym(desc: &str) -> String {
        format!("rust-analyzer cargo fix 0.1.0 {desc}")
    }

    // Rebuilds the Shape/Circle/Square fixture as a hand-made SCIP index:
    //   trait Shape { fn area(); }  impl Shape for Circle/Square
    //   fn total(shapes) { for sh { sh.area() } }   // dyn dispatch -> Shape#area()
    //   fn make() -> Vec<Box<dyn Shape>> { Circle; Square }
    //   fn main() { total(make()) }
    fn shape_index() -> Index {
        let doc = Document {
            relative_path: "src/main.rs".to_string(),
            occurrences: vec![
                // type defs (Shape is a trait -> gets a `Shape#` symbol too)
                occ(&sym("Shape#"), DEFINITION, vec![0, 6, 11], vec![]),
                occ(&sym("Circle#"), DEFINITION, vec![1, 7, 13], vec![]),
                occ(&sym("Square#"), DEFINITION, vec![2, 7, 13], vec![]),
                // trait method decl + impls
                occ(&sym("Shape#area()."), DEFINITION, vec![0, 17, 21], vec![0, 14, 36]),
                // `&[Box<dyn Shape>]` in total()'s signature references the trait type
                occ(&sym("Shape#"), 0, vec![5, 20, 25], vec![]),
                occ(&sym("impl#[Circle][Shape]area()."), DEFINITION, vec![3, 27, 31], vec![3, 24, 54]),
                occ(&sym("impl#[Square][Shape]area()."), DEFINITION, vec![4, 27, 31], vec![4, 24, 53]),
                // total(): body lines 5..9, calls sh.area() at line 7 -> Shape#area() (dyn)
                occ(&sym("total()."), DEFINITION, vec![5, 3, 8], vec![5, 0, 9, 1]),
                occ(&sym("Shape#area()."), 0, vec![7, 31, 35], vec![]),
                // make(): body line 10, instantiates Circle + Square
                occ(&sym("make()."), DEFINITION, vec![10, 3, 7], vec![10, 0, 10, 40]),
                occ(&sym("Circle#"), 0, vec![10, 20, 26], vec![]),
                occ(&sym("Square#"), 0, vec![10, 32, 38], vec![]),
                // main(): body line 11, calls total() + make() (static)
                occ(&sym("main()."), DEFINITION, vec![11, 3, 7], vec![11, 0, 11, 40]),
                occ(&sym("total()."), 0, vec![11, 27, 32], vec![]),
                occ(&sym("make()."), 0, vec![11, 34, 38], vec![]),
            ],
            ..Default::default()
        };
        Index {
            documents: vec![doc],
            ..Default::default()
        }
    }

    fn func_named(facts: &Facts, name: &str) -> FuncId {
        facts.funcs.iter().position(|f| f.name == name).unwrap_or_else(|| panic!("no func {name}"))
    }

    #[test]
    fn extracts_funcs_impls_and_instantiated() {
        let facts = facts_from_index(&shape_index());
        // 6 functions: Shape#area (decl), 2 impls, total, make, main
        assert_eq!(facts.funcs.len(), 6);
        // two trait impls of Shape::area
        assert_eq!(facts.impls.len(), 2);
        assert!(facts.impls.iter().any(|e| e.for_type == "Circle"));
        assert!(facts.impls.iter().any(|e| e.for_type == "Square"));
        // both concrete types instantiated
        assert!(facts.instantiated.contains("Circle"));
        assert!(facts.instantiated.contains("Square"));
        // the trait name is not a concrete type -> excluded from the RTA set
        assert!(!facts.instantiated.contains("Shape"));
    }

    #[test]
    fn static_calls_resolved() {
        let facts = facts_from_index(&shape_index());
        let g = CallGraph::build(&facts, Precision::Cha);
        let main = func_named(&facts, "main");
        let total = func_named(&facts, "total");
        let make = func_named(&facts, "make");
        assert!(g.edges[main].contains(&total));
        assert!(g.edges[main].contains(&make));
    }

    #[test]
    fn dyn_dispatch_expands_via_cha() {
        let facts = facts_from_index(&shape_index());
        let g = CallGraph::build(&facts, Precision::Cha);
        let total = func_named(&facts, "total");
        // total() dyn-calls Shape::area -> both Circle and Square impls
        let circle = func_named(&facts, "impl#[Circle][Shape]area");
        let square = func_named(&facts, "impl#[Square][Shape]area");
        assert!(g.edges[total].contains(&circle));
        assert!(g.edges[total].contains(&square));
    }

    #[test]
    fn rta_keeps_instantiated_impls() {
        // both Circle and Square are instantiated in make(), so RTA keeps both.
        let facts = facts_from_index(&shape_index());
        let g = CallGraph::build(&facts, Precision::Rta);
        let total = func_named(&facts, "total");
        assert_eq!(g.out_degree(total), 2);
    }

    #[test]
    fn rta_prunes_uninstantiated_impl() {
        // Drop the Square instantiation: RTA must prune the Square edge but keep Circle.
        let mut idx = shape_index();
        idx.documents[0]
            .occurrences
            .retain(|o| !(o.symbol.ends_with("Square#") && o.symbol_roles == 0));
        let facts = facts_from_index(&idx);
        assert!(facts.instantiated.contains("Circle"));
        assert!(!facts.instantiated.contains("Square"));
        let g = CallGraph::build(&facts, Precision::Rta);
        let total = func_named(&facts, "total");
        let circle = func_named(&facts, "impl#[Circle][Shape]area");
        let square = func_named(&facts, "impl#[Square][Shape]area");
        assert!(g.edges[total].contains(&circle));
        assert!(!g.edges[total].contains(&square)); // pruned by RTA
    }
}
