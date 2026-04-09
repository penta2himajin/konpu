use super::AlgebraicDeclaration;
use super::Law;
use super::TestStatus;

/// Invariant: LawTestRelevance
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LawTest {
    pub declaration: AlgebraicDeclaration,
    pub law: Law,
    pub status: TestStatus,
}
