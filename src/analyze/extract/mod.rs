//! 抽出層の共有語彙 + 言語別抽出器のディスパッチ。
//!
//! ここ（`mod.rs`）は言語非依存のコア構造体（`AnalyzedDeclaration` / `MethodInfo` /
//! `ImplInfo` / `LawTestInfo` / `UseStatement` / `IgnoreInfo`）と、全言語で共通の
//! 小ヘルパ（`law_from_name` / `ignore_reason_from_str`）だけを持つ。tree-sitter の
//! grammar に依存する実際の抽出は各言語ファイル（`rust` / `swift` / `kotlin` / `ts`）。
//! 下流の check/infer/template/compliance/propagation はこの共有語彙のみに依存する。

pub mod kotlin;
pub mod rust;
pub mod swift;
pub mod ts;

use std::path::PathBuf;

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
    /// 演算本体が非純粋（外部可変状態の読み書き・非決定的呼び出し）と判定されたか。
    /// 非純粋な演算は結合律を破りうるので confidence を withhold する。今は TS の
    /// object-literal encoding でのみ算出（他言語・他経路は false）。
    pub impure: bool,
}

#[derive(Debug, Clone)]
pub struct ImplInfo {
    pub type_name: String,
    pub methods: Vec<MethodInfo>,
}

#[derive(Debug, Clone)]
pub struct LawTestInfo {
    pub laws: Vec<Law>,
    pub enclosing_type: Option<String>,
    /// 直後の `fn` 名。テスト結果（`cargo test` 出力）と突き合わせて
    /// 通過/不通過を判定するためのキー。抽出できなければ `None`。
    pub test_fn: Option<String>,
    pub path: PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct UseStatement {
    pub path: std::path::PathBuf,
    /// Rust: `use` のパス（`crate::domain::Money`）。Swift/Kotlin/TS: import 指定子。
    pub imported_path: String,
    pub line: usize,
    /// import 元言語。境界検査の照合方式を切り替える（Rust=パスキー、他=モジュール名）。
    pub language: super::parser::Language,
}

#[derive(Debug, Clone)]
pub struct IgnoreInfo {
    pub reason: crate::domain::konpu::IgnoreReason,
    pub note: Option<String>,
    pub type_name: Option<String>,
    pub path: std::path::PathBuf,
    pub line: usize,
}

/// law ディレクティブ名 → `Law`。全言語共通（`// konpu: law(...)` / `#[konpu::law(...)]`）。
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

/// ignore 理由名 → `IgnoreReason`。全言語共通。
pub fn ignore_reason_from_str(s: &str) -> Option<crate::domain::konpu::IgnoreReason> {
    use crate::domain::konpu::IgnoreReason;
    match s.trim() {
        "intentional" => Some(IgnoreReason::Intentional),
        "debt" => Some(IgnoreReason::Debt),
        "infeasible" => Some(IgnoreReason::Infeasible),
        _ => None,
    }
}
