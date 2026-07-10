//! Layer 2a: モジュール（ディレクトリ）依存グラフ。
//!
//! 純構文解析（`use`/`import` 抽出）でディレクトリ間の import 関係を組み、循環依存
//! （SCC）と結合ハブ（fan-in/out）を報告する。call graph(2b) と違い意味解析は不要なので
//! feature flag なしで常に使える。
//!
//! 粒度はディレクトリ（root 相対）。エッジは「ディレクトリ A のファイルが、ディレクトリ B
//! のファイルへ import する」。同一ディレクトリ内 import は自己ループになるので落とす。
//!
//! MVP は **Rust（crate パス）と TS（相対 import）** を解決する。両者はパスから解決できる。
//! Swift/Kotlin はモジュール名 import でディレクトリに素直に落ちないので、別途言語別の
//! 方法で対応予定（現状は未解決＝エッジ化しない。存在ファイル数だけ注記する）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::extract;
use super::parser::{self, Language};
use super::template::ResolvedConfig;

/// ディレクトリ依存グラフ。`edges[i]` = モジュール i が依存するモジュール index 集合。
pub struct ModuleGraph {
    pub modules: Vec<String>,
    pub edges: Vec<BTreeSet<usize>>,
    /// 未解決のまま残った言語別ファイル数（Swift/Kotlin など）。報告用。
    pub unresolved: BTreeMap<&'static str, usize>,
}

impl ModuleGraph {
    pub fn out_degree(&self, i: usize) -> usize {
        self.edges[i].len()
    }

    pub fn in_degree(&self, i: usize) -> usize {
        self.edges.iter().filter(|e| e.contains(&i)).count()
    }

    /// fan-out（依存先が多い＝多くを巻き込む）ハブ。
    pub fn fan_out_hubs(&self, min: usize) -> Vec<usize> {
        (0..self.modules.len()).filter(|&i| self.out_degree(i) >= min).collect()
    }

    /// fan-in（依存される＝変更の波及元）ハブ。
    pub fn fan_in_hubs(&self, min: usize) -> Vec<usize> {
        (0..self.modules.len()).filter(|&i| self.in_degree(i) >= min).collect()
    }

    /// 循環依存＝サイズ2以上の SCC（自己ループは同一ディレクトリで落としてあるので出ない）。
    pub fn cycles(&self) -> Vec<Vec<usize>> {
        let mut sccs = tarjan(&self.edges);
        sccs.retain(|s| s.len() > 1);
        for s in &mut sccs {
            s.sort_unstable();
        }
        sccs.sort_by_key(|s| s[0]);
        sccs
    }
}

/// プロジェクトからディレクトリ依存グラフを構築する。`config.exclude` は尊重する。
pub fn build(path: &Path, config: &ResolvedConfig) -> ModuleGraph {
    let files: Vec<(PathBuf, Language)> = parser::collect_source_files(path)
        .into_iter()
        .filter(|(f, _)| !config.is_excluded(f, path))
        .collect();

    // root 相対パス（POSIX 区切り）へ正規化して集める。
    let rels: Vec<(String, Language)> = files
        .iter()
        .map(|(f, l)| (rel_str(f, path), *l))
        .collect();
    let known: BTreeSet<String> = rels.iter().map(|(r, _)| r.clone()).collect();

    // ノード = ソースを含むディレクトリ（解決対象言語 Rust/TS のみ）。
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut modules: Vec<String> = Vec::new();
    let node = |dir: String, index: &mut BTreeMap<String, usize>, modules: &mut Vec<String>| -> usize {
        if let Some(&i) = index.get(&dir) {
            return i;
        }
        let i = modules.len();
        index.insert(dir.clone(), i);
        modules.push(dir);
        i
    };

    let mut unresolved: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (rel, lang) in &rels {
        if !matches!(lang, Language::Rust | Language::Ts) {
            *unresolved.entry(lang_name(*lang)).or_insert(0) += 1;
            continue;
        }
        node(dir_of(rel), &mut index, &mut modules);
    }

    // 各ファイルの import を解決してエッジ化。
    let mut raw_edges: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (rel, lang) in &rels {
        let (Language::Rust | Language::Ts) = lang else { continue };
        let Ok(src) = std::fs::read_to_string(path.join(rel)) else { continue };
        let Some(tree) = parser::parse_with(&src, *lang) else { continue };
        let root = tree.root_node();
        let rel_path = Path::new(rel);
        let uses = match lang {
            Language::Rust => extract::rust::extract_use_statements(root, &src, rel_path),
            Language::Ts => extract::ts::extract_use_statements(root, &src, rel_path),
            _ => continue,
        };
        let importer_dir = dir_of(rel);
        let Some(&from) = index.get(&importer_dir) else { continue };
        for u in &uses {
            let target = match lang {
                Language::Rust => resolve_rust(&u.imported_path, rel, &known),
                Language::Ts => resolve_ts(&u.imported_path, &importer_dir, &known),
                _ => None,
            };
            if let Some(target_file) = target {
                let to_dir = dir_of(&target_file);
                if let Some(&to) = index.get(&to_dir) {
                    if from != to {
                        raw_edges.insert((from, to));
                    }
                }
            }
        }
    }

    let mut edges = vec![BTreeSet::new(); modules.len()];
    for (a, b) in raw_edges {
        edges[a].insert(b);
    }
    ModuleGraph { modules, edges, unresolved }
}

fn lang_name(l: Language) -> &'static str {
    match l {
        Language::Rust => "Rust",
        Language::Swift => "Swift",
        Language::Kotlin => "Kotlin",
        Language::Ts => "TS",
    }
}

/// ファイルパスの root 相対 POSIX 文字列。
fn rel_str(f: &Path, root: &Path) -> String {
    let r = f.strip_prefix(root).unwrap_or(f);
    r.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// root 相対ファイルパスの親ディレクトリ（root 直下は ""）。
fn dir_of(rel: &str) -> String {
    match rel.rfind('/') {
        Some(i) => rel[..i].to_string(),
        None => String::new(),
    }
}

/// TS 相対 import を対象ファイルへ解決する。相対（`.`/`..`）のみ。bare（`zod` 等）は外部。
fn resolve_ts(spec: &str, importer_dir: &str, known: &BTreeSet<String>) -> Option<String> {
    if !spec.starts_with('.') {
        return None; // bare specifier = 外部パッケージ。
    }
    let joined = normalize_join(importer_dir, spec);
    // ESM/TS 慣習: import 指定子は `.js`（実ファイルは `.ts`）。末尾拡張子を剥がして再照合。
    let joined = strip_ext(&joined);
    // `./x` → x.ts / x.tsx / x/index.ts …
    for ext in ["ts", "tsx", "mts", "cts"] {
        let cand = format!("{joined}.{ext}");
        if known.contains(&cand) {
            return Some(cand);
        }
    }
    for idx in ["index.ts", "index.tsx"] {
        let cand = if joined.is_empty() { idx.to_string() } else { format!("{joined}/{idx}") };
        if known.contains(&cand) {
            return Some(cand);
        }
    }
    None
}

/// 末尾の JS/TS 拡張子を剥がす（`./x.js` → `./x`）。ESM 指定子は拡張子付きで書かれる。
fn strip_ext(p: &str) -> String {
    for ext in [".js", ".mjs", ".cjs", ".jsx", ".ts", ".tsx", ".mts", ".cts"] {
        if let Some(stem) = p.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    p.to_string()
}

/// `importer_dir` から相対 `spec`（`./a`, `../b/c`）を解決した POSIX パス（拡張子なし）。
fn normalize_join(importer_dir: &str, spec: &str) -> String {
    let mut parts: Vec<&str> = if importer_dir.is_empty() {
        Vec::new()
    } else {
        importer_dir.split('/').collect()
    };
    for seg in spec.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Rust の `use` パスを対象ファイルへ解決する。`importer_file` は import 元の root 相対パス。
/// `crate::a::b::Item` はサフィックス照合で `.../a/b.rs`（or `a/b/mod.rs`）を探す。
/// `self::`/`super::` は importer のモジュールからの相対、外部クレート（`std`/`serde`…）は None。
///
/// Rust の `super` 意味論の要点: `foo.rs` の module は `<dir>::foo` なので `super` は
/// `<dir>` ＝**自分のディレクトリ**。`mod.rs` の module は `<dir>` なので `super` は**親**。
fn resolve_rust(imported: &str, importer_file: &str, known: &BTreeSet<String>) -> Option<String> {
    // グループ `{...}` と `as` エイリアスの手前でモジュールパスを切る。
    let head = imported.split('{').next().unwrap_or(imported);
    let head = head.split(" as ").next().unwrap_or(head).trim();
    let mut segs: Vec<&str> = head.split("::").map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return None;
    }
    let importer_dir = dir_of(importer_file);
    let is_mod = is_module_root_file(importer_file);
    match segs[0] {
        "crate" => {
            segs.remove(0);
            resolve_rust_segments(&segs, known)
        }
        "self" => {
            segs.remove(0);
            resolve_rel_rust(&segs, &importer_dir, known)
        }
        "super" => {
            // 最初の super: mod.rs は親ディレクトリ、それ以外は自分のディレクトリ。
            segs.remove(0);
            let mut base = if is_mod { dir_of(&importer_dir) } else { importer_dir };
            while segs.first() == Some(&"super") {
                segs.remove(0);
                base = dir_of(&base);
            }
            resolve_rel_rust(&segs, &base, known)
        }
        // std / core / alloc / 外部クレート。
        _ => None,
    }
}

/// `mod.rs` / `lib.rs` / `main.rs` はディレクトリ自身がモジュール（`super` は親を指す）。
fn is_module_root_file(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    matches!(name, "mod.rs" | "lib.rs" | "main.rs")
}

/// crate ルート相対のセグメント列を、既知ファイルへサフィックス照合で解決する。
/// 末尾（＝import する item 名）を落としながら最長一致を探す。
fn resolve_rust_segments(segs: &[&str], known: &BTreeSet<String>) -> Option<String> {
    for k in (1..=segs.len()).rev() {
        let cand = segs[..k].join("/");
        if let Some(f) = match_rust_file(&cand, known) {
            return Some(f);
        }
    }
    None
}

/// self/super からの相対セグメントを、base ディレクトリ配下で解決する。
fn resolve_rel_rust(segs: &[&str], base: &str, known: &BTreeSet<String>) -> Option<String> {
    for k in (1..=segs.len()).rev() {
        let joined = segs[..k].join("/");
        let cand = if base.is_empty() { joined } else { format!("{base}/{joined}") };
        // 相対は先頭一致でよいが、実装統一のためサフィックス照合を流用する。
        if let Some(f) = match_rust_file(&cand, known) {
            return Some(f);
        }
    }
    None
}

/// `cand`（`a/b` のようなモジュールパス）に対応する既知 .rs ファイルを探す。
/// `cand.rs` / `cand/mod.rs` を末尾一致で照合（crate ルートの接頭辞差を吸収）。
fn match_rust_file(cand: &str, known: &BTreeSet<String>) -> Option<String> {
    let file = format!("{cand}.rs");
    let modrs = format!("{cand}/mod.rs");
    known
        .iter()
        .find(|f| {
            **f == file
                || f.ends_with(&format!("/{file}"))
                || **f == modrs
                || f.ends_with(&format!("/{modrs}"))
        })
        .cloned()
}

/// Tarjan の強連結成分分解。`adj[i]` = i の後続 index 集合。
fn tarjan(adj: &[BTreeSet<usize>]) -> Vec<Vec<usize>> {
    struct State<'a> {
        adj: &'a [BTreeSet<usize>],
        index: usize,
        indices: Vec<Option<usize>>,
        low: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        out: Vec<Vec<usize>>,
    }
    fn strongconnect(s: &mut State, v: usize) {
        s.indices[v] = Some(s.index);
        s.low[v] = s.index;
        s.index += 1;
        s.stack.push(v);
        s.on_stack[v] = true;
        for &w in s.adj[v].iter() {
            if s.indices[w].is_none() {
                strongconnect(s, w);
                s.low[v] = s.low[v].min(s.low[w]);
            } else if s.on_stack[w] {
                s.low[v] = s.low[v].min(s.indices[w].unwrap());
            }
        }
        if s.low[v] == s.indices[v].unwrap() {
            let mut scc = Vec::new();
            loop {
                let w = s.stack.pop().unwrap();
                s.on_stack[w] = false;
                scc.push(w);
                if w == v {
                    break;
                }
            }
            s.out.push(scc);
        }
    }
    let n = adj.len();
    let mut s = State {
        adj,
        index: 0,
        indices: vec![None; n],
        low: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        out: Vec::new(),
    };
    for v in 0..n {
        if s.indices[v].is_none() {
            strongconnect(&mut s, v);
        }
    }
    s.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(fs: &[&str]) -> BTreeSet<String> {
        fs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ts_relative_import_resolves() {
        let k = known(&["src/domain/money.ts", "src/infra/db.ts"]);
        assert_eq!(resolve_ts("../infra/db", "src/domain", &k).as_deref(), Some("src/infra/db.ts"));
        assert_eq!(resolve_ts("./money", "src/domain", &k).as_deref(), Some("src/domain/money.ts"));
        assert_eq!(resolve_ts("zod", "src/domain", &k), None); // 外部
        // ESM 指定子（`.js` 付き、実体は `.ts`）。
        assert_eq!(resolve_ts("../infra/db.js", "src/domain", &k).as_deref(), Some("src/infra/db.ts"));
    }

    #[test]
    fn ts_index_import_resolves() {
        let k = known(&["src/infra/index.ts"]);
        assert_eq!(resolve_ts("../infra", "src/domain", &k).as_deref(), Some("src/infra/index.ts"));
    }

    #[test]
    fn rust_crate_path_resolves_by_suffix() {
        let k = known(&["src/analyze/extract/rust.rs", "src/analyze/mod.rs"]);
        // item 名 `Foo` は落として module `analyze/extract/rust` に一致。
        assert_eq!(
            resolve_rust("crate::analyze::extract::rust::Foo", "src/whatever", &k).as_deref(),
            Some("src/analyze/extract/rust.rs")
        );
        // mod.rs も拾う。
        assert_eq!(
            resolve_rust("crate::analyze::Thing", "src/x", &k).as_deref(),
            Some("src/analyze/mod.rs")
        );
        assert_eq!(resolve_rust("std::fmt::Display", "src/x", &k), None); // 外部
    }

    #[test]
    fn rust_super_semantics_mod_vs_regular() {
        let k = known(&["src/analyze/check.rs", "src/analyze/extract/sub.rs"]);
        // mod.rs の super は親ディレクトリ: extract/mod.rs から super::check → src/analyze/check.rs。
        assert_eq!(
            resolve_rust("super::check::Foo", "src/analyze/extract/mod.rs", &k).as_deref(),
            Some("src/analyze/check.rs")
        );
        // 非 mod ファイルの super は自分のディレクトリ: extract/rust.rs から super::sub → extract/sub.rs。
        assert_eq!(
            resolve_rust("super::sub::Bar", "src/analyze/extract/rust.rs", &k).as_deref(),
            Some("src/analyze/extract/sub.rs")
        );
    }

    #[test]
    fn cycles_detects_two_dir_scc() {
        // 0->1, 1->0 の循環。
        let edges = vec![
            BTreeSet::from([1usize]),
            BTreeSet::from([0usize]),
            BTreeSet::new(),
        ];
        let sccs = tarjan(&edges);
        assert_eq!(sccs.iter().filter(|s| s.len() == 2).count(), 1);
    }
}
