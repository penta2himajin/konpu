use super::AlgebraicStructure;
use super::HigherKindedStructure;
use super::PathPattern;
use std::collections::BTreeSet;

/// Invariant: ValidMaxPropagation
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayerExpectation {
    pub pathPattern: PathPattern,
    pub expectedStructures: BTreeSet<AlgebraicStructure>,
    pub expectedHigherKinded: BTreeSet<HigherKindedStructure>,
    pub maxPropagation: Option<i64>,
}
