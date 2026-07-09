//! `konpu-cg` — コールグラフ provider とその構築エンジン。
//!
//! 構成 (docs/layer2-call-graph-design.md):
//! - `facts`: 抽出器が生成する言語中立な事実モデル。
//! - `graph`: CHA/RTA によるディスパッチ解釈と循環/ハブのクエリ。
//! - `CallGraphProvider`: konpu 本体が使う位置ベースの provider トレイト。

pub mod facts;
pub mod graph;
#[cfg(feature = "scip")]
pub mod scip_extract;

pub use facts::{CallSite, CallTargetKind, FuncDef, FuncId, Facts, ImplEntry, TraitMethod};
pub use graph::{CallGraph, Precision};
#[cfg(feature = "scip")]
pub use scip_extract::{facts_from_index, facts_from_project, facts_from_scip_file};

use std::path::{Path, PathBuf};

/// コールグラフ中の呼び出し先 1 件分。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallTarget {
    pub target_path: PathBuf,
    pub target_line: usize,
    pub target_name: String,
}

/// コールグラフ provider のトレイト。
///
/// `konpu` 本体はこれに依存し、provider なし (None) の場合は
/// 従来の近似検査 (同名型 rank 降格など) にフォールバックする。
pub trait CallGraphProvider {
    /// 指定位置からの outgoing call (呼び出し先) を返す。
    /// 空の Vec は「呼出なし」を意味する。
    /// provider が未実装の場合は空 Vec を返す。
    fn resolve_outgoing_calls(
        &self,
        _file_path: &Path,
        _line: usize,
        _column: usize,
    ) -> Vec<CallTarget> {
        Vec::new()
    }
}

/// ダミー provider。何も解決しない。
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyCallGraphProvider;

impl CallGraphProvider for EmptyCallGraphProvider {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_provider_returns_no_calls() {
        let p = EmptyCallGraphProvider;
        let calls = p.resolve_outgoing_calls(Path::new("src/lib.rs"), 0, 0);
        assert!(calls.is_empty());
    }
}
