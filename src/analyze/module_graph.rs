//! Layer 2a: モジュール（ディレクトリ）依存グラフ。
//!
//! 純構文解析（`use`/`import` 抽出）でディレクトリ間の import 関係を組み、循環依存
//! （SCC）と結合ハブ（fan-in/out）を報告する。call graph(2b) と違い意味解析は不要なので
//! feature flag なしで常に使える。
//!
//! 粒度はディレクトリ（root 相対）。エッジは「ディレクトリ A のファイルが、ディレクトリ B
//! のファイルへ import する」。同一ディレクトリ内 import は自己ループになるので落とす。
//!
//! 言語別の解決方式:
//! - **Rust**: crate パス（`crate::a::b`）をファイルへサフィックス照合。
//! - **TS**: 相対 import（`./`/`../`）を importer dir から join。
//! - **Kotlin**: `package a.b.c` 宣言を索引化し、import の完全修飾名を宣言済み package へ
//!   最長一致（ディレクトリ慣習に依存せず、宣言が SSoT）。
//! - **Swift**: module 内は import 文が存在しないため2本立て —
//!   (1) `import M` → swift ソースを含む basename==M の dir（SwiftPM `Sources/M/` 慣習、一意時のみ）、
//!   (2) 型参照 → 他 dir に一意宣言された型名の使用をエッジ化（best-effort）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::extract;
use super::parser::{self, Language};
use super::template::ResolvedConfig;

/// ディレクトリ依存グラフ。`edges[i]` = モジュール i が依存するモジュール index 集合。
pub struct ModuleGraph {
    pub modules: Vec<String>,
    pub edges: Vec<BTreeSet<usize>>,
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

/// 1ファイル分の解析結果（parse は1回で済ませ、索引→エッジ化を後段で行う）。
struct FileFacts {
    rel: String,
    lang: Language,
    /// import / use 文の指定子。
    imports: Vec<String>,
    /// Kotlin: `package a.b.c` 宣言。
    kt_package: Option<String>,
    /// Swift: このファイルで宣言される型名。
    /// Kotlin: import 可能なトップレベル symbol（型 + トップレベル関数）。package と組で FQCN 索引になる。
    type_decls: Vec<String>,
    /// Swift: このファイルが参照する型名。
    swift_refs: BTreeSet<String>,
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

    // ノード = ソースを含むディレクトリ（全対応言語）。
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
    for (rel, _) in &rels {
        node(dir_of(rel), &mut index, &mut modules);
    }

    // パス1: 各ファイルを一度だけ parse して素材を集める。
    let mut facts: Vec<FileFacts> = Vec::new();
    for (rel, lang) in &rels {
        let Ok(src) = std::fs::read_to_string(path.join(rel)) else { continue };
        let Some(tree) = parser::parse_with(&src, *lang) else { continue };
        let root = tree.root_node();
        let rel_path = Path::new(rel);
        let uses = match lang {
            Language::Rust => extract::rust::extract_use_statements(root, &src, rel_path),
            Language::Ts => extract::ts::extract_use_statements(root, &src, rel_path),
            Language::Kotlin => extract::kotlin::extract_use_statements(root, &src, rel_path),
            Language::Swift => extract::swift::extract_use_statements(root, &src, rel_path),
        };
        let kt_package = matches!(lang, Language::Kotlin)
            .then(|| extract::kotlin::extract_package(root, &src))
            .flatten();
        let (type_decls, swift_refs) = match lang {
            Language::Swift => {
                let decls = extract::swift::extract_type_sites(root, &src, rel_path)
                    .into_iter()
                    .map(|(name, _, _)| name)
                    .collect();
                (decls, extract::swift::extract_type_refs(root, &src))
            }
            Language::Kotlin => {
                let mut decls: Vec<String> = extract::kotlin::extract_type_sites(root, &src, rel_path)
                    .into_iter()
                    .map(|(name, _, _)| name)
                    .collect();
                // トップレベル関数も import 対象（拡張関数含む）なので symbol 索引に足す。
                decls.extend(extract::kotlin::extract_free_fns(root, &src).into_iter().map(|m| m.name));
                (decls, BTreeSet::new())
            }
            _ => (Vec::new(), BTreeSet::new()),
        };
        facts.push(FileFacts {
            rel: rel.clone(),
            lang: *lang,
            imports: uses.into_iter().map(|u| u.imported_path).collect(),
            kt_package,
            type_decls,
            swift_refs,
        });
    }

    // パス2: 言語別索引。
    // Kotlin: 宣言済み package → その package を宣言する dir 集合。
    let mut kt_packages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // Kotlin: FQCN（package.Type）→ 宣言 dir 集合。同一 package が複数 dir に
    // 跨る場合（KMP の commonMain/commonTest 等）に型 import を宣言 dir へ絞る。
    let mut kt_types: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // Swift: 型名 → 宣言 dir 集合（一意なもののみ参照解決に使う）。
    let mut swift_decl_dirs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // Swift: dir basename → swift ソースを含む dir 集合（import 解決用）。
    let mut swift_basenames: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in &facts {
        let dir = dir_of(&f.rel);
        if let Some(p) = &f.kt_package {
            kt_packages.entry(p.clone()).or_default().insert(dir.clone());
            for t in &f.type_decls {
                kt_types.entry(format!("{p}.{t}")).or_default().insert(dir.clone());
            }
        }
        if matches!(f.lang, Language::Swift) {
            for t in &f.type_decls {
                swift_decl_dirs.entry(t.clone()).or_default().insert(dir.clone());
            }
            if let Some(base) = dir.rsplit('/').next().filter(|b| !b.is_empty()) {
                swift_basenames.entry(base.to_string()).or_default().insert(dir.clone());
            }
        }
    }

    // パス3: 解決してエッジ化。
    let mut raw_edges: BTreeSet<(usize, usize)> = BTreeSet::new();
    for f in &facts {
        let importer_dir = dir_of(&f.rel);
        let Some(&from) = index.get(&importer_dir) else { continue };
        let add = |to_dir: &str, raw_edges: &mut BTreeSet<(usize, usize)>| {
            if let Some(&to) = index.get(to_dir) {
                if from != to {
                    raw_edges.insert((from, to));
                }
            }
        };
        for spec in &f.imports {
            match f.lang {
                Language::Rust => {
                    if let Some(file) = resolve_rust(spec, &f.rel, &known) {
                        add(&dir_of(&file), &mut raw_edges);
                    }
                }
                Language::Ts => {
                    if let Some(file) = resolve_ts(spec, &importer_dir, &known) {
                        add(&dir_of(&file), &mut raw_edges);
                    }
                }
                Language::Kotlin => {
                    for dir in resolve_kotlin(spec, &kt_packages, &kt_types) {
                        add(&dir, &mut raw_edges);
                    }
                }
                Language::Swift => {
                    if let Some(dir) = resolve_swift_import(spec, &swift_basenames) {
                        add(&dir, &mut raw_edges);
                    }
                }
            }
        }
        // Swift: 型参照 → 一意宣言 dir へのエッジ。
        for r in &f.swift_refs {
            if let Some(dirs) = swift_decl_dirs.get(r) {
                if dirs.len() == 1 {
                    add(dirs.iter().next().unwrap(), &mut raw_edges);
                }
            }
        }
    }

    let mut edges = vec![BTreeSet::new(); modules.len()];
    for (a, b) in raw_edges {
        edges[a].insert(b);
    }
    ModuleGraph { modules, edges }
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
/// call_graph::ts の import 束縛解決からも再利用する（pub(crate) はそのため）。
pub(crate) fn resolve_ts(spec: &str, importer_dir: &str, known: &BTreeSet<String>) -> Option<String> {
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

/// Kotlin import（完全修飾名）を宣言済み package への最長一致で解決する。
/// `import a.b.c.Db` は `a.b.c.Db`（object/nested import）→ `a.b.c` → `a.b` … の順。
/// wildcard `import a.b.*` は抽出時点で `a.b` になっているのでそのまま一致する。
/// 外部パッケージ（kotlinx 等）は索引に無く空を返す。
///
/// 同一 package が複数 dir に跨る場合（KMP の commonMain/commonTest/jvmTest が同じ
/// package を宣言する）は、import した symbol（次セグメント）を FQCN 索引 `symbols`
/// （型 + トップレベル関数）で宣言 dir に絞る。絞れない残余（wildcard・未索引 symbol）
/// を全 dir に張ると main→test の偽エッジ・偽循環を量産する（arrow-kt 実測）ので
/// エッジ化しない — 誤エッジより miss。
fn resolve_kotlin(
    spec: &str,
    packages: &BTreeMap<String, BTreeSet<String>>,
    symbols: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<String> {
    let segs: Vec<&str> = spec.split('.').filter(|s| !s.is_empty()).collect();
    for k in (1..=segs.len()).rev() {
        let cand = segs[..k].join(".");
        if let Some(dirs) = packages.get(&cand) {
            if dirs.len() == 1 {
                return dirs.iter().cloned().collect();
            }
            if k < segs.len() {
                let fq = segs[..=k].join(".");
                if let Some(sdirs) = symbols.get(&fq) {
                    return sdirs.iter().cloned().collect();
                }
            }
            return Vec::new();
        }
    }
    Vec::new()
}

/// Swift `import M` を「swift ソースを含む basename==M の dir」へ解決（一意時のみ）。
/// SwiftPM の `Sources/<Target>/` 慣習に乗る。Foundation 等の外部は索引に無く None。
fn resolve_swift_import(module: &str, basenames: &BTreeMap<String, BTreeSet<String>>) -> Option<String> {
    let dirs = basenames.get(module)?;
    if dirs.len() == 1 {
        dirs.iter().next().cloned()
    } else {
        None // 同名 dir が複数 → 曖昧。誤エッジより miss を選ぶ。
    }
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
    fn kotlin_import_resolves_by_declared_package() {
        let mut pkgs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        pkgs.entry("com.x.domain".into()).or_default().insert("src/domain".into());
        pkgs.entry("com.x.infra".into()).or_default().insert("src/infra".into());
        let types = BTreeMap::new();
        // クラス import は最長一致で package に落ちる。
        assert_eq!(resolve_kotlin("com.x.infra.Db", &pkgs, &types), vec!["src/infra".to_string()]);
        // ネスト（inner class / companion）も落ちる。
        assert_eq!(resolve_kotlin("com.x.infra.Db.Tx", &pkgs, &types), vec!["src/infra".to_string()]);
        // wildcard は抽出時点で package 名そのもの。
        assert_eq!(resolve_kotlin("com.x.domain", &pkgs, &types), vec!["src/domain".to_string()]);
        // 外部パッケージ。
        assert!(resolve_kotlin("kotlinx.coroutines.flow.Flow", &pkgs, &types).is_empty());
    }

    #[test]
    fn kotlin_multi_dir_package_narrows_by_declared_symbol() {
        // KMP: commonMain と commonTest が同じ package を宣言。symbol import は宣言 dir へ絞る。
        let main = "src/commonMain/kotlin/arrow/core";
        let test = "src/commonTest/kotlin/arrow/core";
        let mut pkgs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for d in [main, test] {
            pkgs.entry("arrow.core".into()).or_default().insert(d.into());
        }
        let mut symbols: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        symbols.entry("arrow.core.NonEmptyList".into()).or_default().insert(main.into());
        symbols.entry("arrow.core.flatMap".into()).or_default().insert(main.into());
        // 型 import → 宣言 dir のみ（test dir への偽エッジを張らない）。
        assert_eq!(resolve_kotlin("arrow.core.NonEmptyList", &pkgs, &symbols), vec![main.to_string()]);
        // トップレベル関数 import も同様に絞れる。
        assert_eq!(resolve_kotlin("arrow.core.flatMap", &pkgs, &symbols), vec![main.to_string()]);
        // 絞れない（wildcard / 未索引 symbol）→ 全 dir に張ると偽循環を量産するので miss。
        assert!(resolve_kotlin("arrow.core", &pkgs, &symbols).is_empty());
        assert!(resolve_kotlin("arrow.core.unknownVal", &pkgs, &symbols).is_empty());
    }

    #[test]
    fn swift_import_resolves_unique_dir_basename() {
        let mut base: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        base.entry("DomainKit".into()).or_default().insert("Sources/DomainKit".into());
        base.entry("Dup".into()).or_default().insert("a/Dup".into());
        base.entry("Dup".into()).or_default().insert("b/Dup".into());
        assert_eq!(
            resolve_swift_import("DomainKit", &base).as_deref(),
            Some("Sources/DomainKit")
        );
        assert_eq!(resolve_swift_import("Dup", &base), None); // 曖昧
        assert_eq!(resolve_swift_import("Foundation", &base), None); // 外部
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
