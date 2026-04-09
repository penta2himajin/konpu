use super::AlgebraicDeclaration;
use super::DiagnosticRule;
use super::Severity;

/// Invariant: IgnoredSuppressesDiagnostics
/// Invariant: MissingIdentityIsError
/// Invariant: MissingInverseIsError
/// Invariant: ClosureViolationIsError
/// Invariant: MissingLawTestIsWarning
/// Invariant: FailingLawTestIsError
/// Invariant: PropagationExceededIsWarning
/// Invariant: AssociativityConfidenceIsInfo
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Diagnostic {
    pub severity: Severity,
    pub declaration: AlgebraicDeclaration,
    pub rule: DiagnosticRule,
}
