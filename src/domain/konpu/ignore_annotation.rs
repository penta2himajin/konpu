use super::AlgebraicDeclaration;
use super::IgnoreReason;

/// Invariant: IgnoredSuppressesDiagnostics
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IgnoreAnnotation {
    pub reason: IgnoreReason,
    pub declaration: AlgebraicDeclaration,
}
