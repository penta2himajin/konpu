//! コールグラフ構築のための「事実」モデル。
//!
//! 設計 (docs/layer2-call-graph-design.md §3): 事実抽出とディスパッチ解釈を
//! 分離する。このモジュールは抽出器 (rust-analyzer / SCIP など) が生成する
//! 言語中立な事実の型だけを定義し、CHA/RTA による解釈は `graph` モジュールが
//! 担う。抽出器を差し替えても解釈エンジンは共有できる。

use std::collections::HashSet;
use std::path::PathBuf;

/// `Facts::funcs` 内の添字を関数の識別子とする。隣接リストの添字空間と一致。
pub type FuncId = usize;

/// trait メソッドの参照 (trait 名, メソッド名)。動的ディスパッチ候補集合の鍵。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitMethod {
    pub trait_name: String,
    pub method: String,
}

impl TraitMethod {
    pub fn new(trait_name: impl Into<String>, method: impl Into<String>) -> Self {
        TraitMethod {
            trait_name: trait_name.into(),
            method: method.into(),
        }
    }
}

/// 関数/メソッド 1 件の定義。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncDef {
    pub path: PathBuf,
    pub line: usize,
    /// 修飾名 (例 "domain::Money::combine")。表示・照合用。
    pub name: String,
}

/// 呼び出しサイトの静的に判る呼び出し先。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallTargetKind {
    /// 具体関数に解決済み (静的ディスパッチ / 単相化後)。
    Static(FuncId),
    /// 動的ディスパッチ。trait メソッドまでしか判らず CHA/RTA で展開する。
    Dynamic(TraitMethod),
}

/// caller の本体内に現れる呼び出し 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    pub caller: FuncId,
    pub target: CallTargetKind,
}

/// trait 実装メソッド 1 件。CHA の候補集合を構成する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplEntry {
    pub trait_method: TraitMethod,
    /// 実装先の型名 (例 "Money")。RTA の絞り込み鍵。
    pub for_type: String,
    /// この実装メソッド本体を指す FuncId。
    pub func: FuncId,
}

/// 抽出器が生成する事実一式。
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub funcs: Vec<FuncDef>,
    pub calls: Vec<CallSite>,
    pub impls: Vec<ImplEntry>,
    /// RTA 用: プログラム中で実際にインスタンス化された型名の集合。
    pub instantiated: HashSet<String>,
}

impl Facts {
    /// 関数を登録し FuncId を返す。抽出器/テスト用の小さなビルダ。
    pub fn add_func(&mut self, name: impl Into<String>, path: impl Into<PathBuf>, line: usize) -> FuncId {
        let id = self.funcs.len();
        self.funcs.push(FuncDef {
            path: path.into(),
            line,
            name: name.into(),
        });
        id
    }
}
