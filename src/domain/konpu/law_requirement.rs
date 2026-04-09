use super::AlgebraicStructure;
use super::Law;

/// Invariant: SemigroupLaws
/// Invariant: MonoidLeftIdentityLaw
/// Invariant: MonoidRightIdentityLaw
/// Invariant: GroupInverseLeftLaw
/// Invariant: GroupInverseRightLaw
/// Invariant: LawTestRelevance
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LawRequirement {
    pub structure: AlgebraicStructure,
    pub requiredLaw: Law,
}
