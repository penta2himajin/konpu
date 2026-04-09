#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticRule {
    MissingIdentity,
    MissingInverse,
    ClosureViolation,
    MapSignatureViolation,
    MissingLawTest,
    FailingLawTest,
    PropagationExceeded,
    AssociativityConfidence,
}
