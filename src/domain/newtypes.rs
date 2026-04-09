#[allow(unused_imports)]
use crate::domain::fixtures::*;
#[allow(unused_imports)]
use crate::domain::konpu::*;

/// Newtype wrapper: Diagnostic validated by severity rules.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedDiagnostic(pub Diagnostic);

impl TryFrom<Diagnostic> for ValidatedDiagnostic {
    type Error = &'static str;

    fn try_from(value: Diagnostic) -> Result<Self, Self::Error> {
        if value.rule == DiagnosticRule::MissingIdentity && !(value.severity == Severity::Error) {
            return Err("Diagnostic.rule = MissingIdentity implies Diagnostic.severity = Error");
        }
        if value.rule == DiagnosticRule::MissingInverse && !(value.severity == Severity::Error) {
            return Err("Diagnostic.rule = MissingInverse implies Diagnostic.severity = Error");
        }
        if value.rule == DiagnosticRule::ClosureViolation && !(value.severity == Severity::Error) {
            return Err("Diagnostic.rule = ClosureViolation implies Diagnostic.severity = Error");
        }
        if value.rule == DiagnosticRule::MissingLawTest && !(value.severity == Severity::Warning) {
            return Err("Diagnostic.rule = MissingLawTest implies Diagnostic.severity = Warning");
        }
        if value.rule == DiagnosticRule::FailingLawTest && !(value.severity == Severity::Error) {
            return Err("Diagnostic.rule = FailingLawTest implies Diagnostic.severity = Error");
        }
        if value.rule == DiagnosticRule::PropagationExceeded
            && !(value.severity == Severity::Warning)
        {
            return Err(
                "Diagnostic.rule = PropagationExceeded implies Diagnostic.severity = Warning",
            );
        }
        if value.rule == DiagnosticRule::AssociativityConfidence
            && !(value.severity == Severity::Info)
        {
            return Err(
                "Diagnostic.rule = AssociativityConfidence implies Diagnostic.severity = Info",
            );
        }
        Ok(ValidatedDiagnostic(value))
    }
}

/// Newtype wrapper: ContextType validated by CountIsPositive.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedContextType(pub ContextType);

impl TryFrom<ContextType> for ValidatedContextType {
    type Error = &'static str;

    fn try_from(value: ContextType) -> Result<Self, Self::Error> {
        if value.propagation == PropagationSize::Finite && value.variantCount.is_none() {
            return Err("ContextType.propagation = Finite implies some ContextType.variantCount");
        }
        if value.propagation == PropagationSize::Unbounded && value.variantCount.is_some() {
            return Err("ContextType.propagation = Unbounded implies no ContextType.variantCount");
        }
        if let Some(count) = value.variantCount {
            if count <= 0 {
                return Err("some ContextType.variantCount implies ContextType.variantCount > 0");
            }
        }
        Ok(ValidatedContextType(value))
    }
}

/// Newtype wrapper: AlgebraicDeclaration validated by structure requirements.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedAlgebraicDeclaration(pub AlgebraicDeclaration);

impl TryFrom<AlgebraicDeclaration> for ValidatedAlgebraicDeclaration {
    type Error = &'static str;

    fn try_from(value: AlgebraicDeclaration) -> Result<Self, Self::Error> {
        if value.targetStructure.rank() >= 2 && value.identityName.is_none() {
            return Err("Monoid+ requires identityName");
        }
        if value.targetStructure.rank() >= 3 && value.inverseName.is_none() {
            return Err("Group requires inverseName");
        }
        if let Some(ref id) = value.identityName {
            if *id == value.operationName {
                return Err("identityName must differ from operationName");
            }
        }
        Ok(ValidatedAlgebraicDeclaration(value))
    }
}

/// Newtype wrapper: ComplianceReport validated by ValidComplianceCounts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedComplianceReport(pub ComplianceReport);

impl TryFrom<ComplianceReport> for ValidatedComplianceReport {
    type Error = &'static str;

    fn try_from(value: ComplianceReport) -> Result<Self, Self::Error> {
        if value.passingLaws < 0 || value.passingLaws > value.totalLaws || value.totalLaws <= 0 {
            return Err("ValidComplianceCounts invariant violated");
        }
        Ok(ValidatedComplianceReport(value))
    }
}

/// Newtype wrapper: LayerExpectation validated by ValidMaxPropagation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedLayerExpectation(pub LayerExpectation);

impl TryFrom<LayerExpectation> for ValidatedLayerExpectation {
    type Error = &'static str;

    fn try_from(value: LayerExpectation) -> Result<Self, Self::Error> {
        if let Some(max) = value.maxPropagation {
            if max <= 0 && max != -1 {
                return Err("maxPropagation must be positive or -1");
            }
        }
        Ok(ValidatedLayerExpectation(value))
    }
}
