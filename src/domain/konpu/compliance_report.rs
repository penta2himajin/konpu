use super::AlgebraicDeclaration;

/// Invariant: ValidComplianceCounts
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComplianceReport {
    pub declaration: AlgebraicDeclaration,
    pub totalLaws: i64,
    pub passingLaws: i64,
}
