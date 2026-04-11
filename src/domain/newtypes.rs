#[allow(unused_imports)]
use crate::domain::fixtures::*;
use crate::domain::konpu::diagnostic_rule::DiagnosticRule::*;
use crate::domain::konpu::propagation_size::PropagationSize::*;
use crate::domain::konpu::severity::Severity::*;
#[allow(unused_imports)]
use crate::domain::konpu::*;

/// Newtype wrapper: Diagnostic validated by AssociativityConfidenceIsInfo.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedDiagnostic(pub Diagnostic);

impl TryFrom<Diagnostic> for ValidatedDiagnostic {
    type Error = &'static str;

    fn try_from(value: Diagnostic) -> Result<Self, Self::Error> {
        if value.rule == MissingIdentity && !(value.severity == Error) {
            return Err("Diagnostic.rule = MissingIdentity implies Diagnostic.severity = Error");
        }
        if value.rule == MissingInverse && !(value.severity == Error) {
            return Err("Diagnostic.rule = MissingInverse implies Diagnostic.severity = Error");
        }
        if value.rule == ClosureViolation && !(value.severity == Error) {
            return Err("Diagnostic.rule = ClosureViolation implies Diagnostic.severity = Error");
        }
        if value.rule == MissingLawTest && !(value.severity == Warning) {
            return Err("Diagnostic.rule = MissingLawTest implies Diagnostic.severity = Warning");
        }
        if value.rule == FailingLawTest && !(value.severity == Error) {
            return Err("Diagnostic.rule = FailingLawTest implies Diagnostic.severity = Error");
        }
        if value.rule == PropagationExceeded && !(value.severity == Warning) {
            return Err(
                "Diagnostic.rule = PropagationExceeded implies Diagnostic.severity = Warning",
            );
        }
        if value.rule == AssociativityConfidence && !(value.severity == Info) {
            return Err(
                "Diagnostic.rule = AssociativityConfidence implies Diagnostic.severity = Info",
            );
        }
        let diagnostics: Vec<Diagnostic> = vec![value.clone()];
        if diagnostics.iter().all(|d| {
            let d = d.clone();
            !(d.rule == DiagnosticRule::AssociativityConfidence) || d.severity == Severity::Info
        }) {
            Ok(ValidatedDiagnostic(value))
        } else {
            Err("AssociativityConfidenceIsInfo invariant violated")
        }
    }
}

/// Newtype wrapper: ContextType validated by CountIsPositive.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedContextType(pub ContextType);

impl TryFrom<ContextType> for ValidatedContextType {
    type Error = &'static str;

    fn try_from(value: ContextType) -> Result<Self, Self::Error> {
        if value.propagation == Finite && !(value.variantCount.is_some()) {
            return Err("ContextType.propagation = Finite implies some ContextType.variantCount");
        }
        if value.propagation == Unbounded && !(value.variantCount.is_none()) {
            return Err("ContextType.propagation = Unbounded implies no ContextType.variantCount");
        }
        if value.variantCount.is_some() && !value.variantCount.is_none_or(|v| v > 0) {
            return Err("some ContextType.variantCount implies ContextType.variantCount > 0");
        }
        let context_types: Vec<ContextType> = vec![value.clone()];
        if context_types.iter().all(|c| {
            let c = c.clone();
            !c.variantCount.is_some() || c.variantCount.is_none_or(|v| v > 0)
        }) {
            Ok(ValidatedContextType(value))
        } else {
            Err("CountIsPositive invariant violated")
        }
    }
}

/// Newtype wrapper: LawRequirement validated by GroupInverseLeftLaw.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedLawRequirement(pub LawRequirement);

impl TryFrom<LawRequirement> for ValidatedLawRequirement {
    type Error = &'static str;

    fn try_from(value: LawRequirement) -> Result<Self, Self::Error> {
        let law_requirements: Vec<LawRequirement> = vec![value.clone()];
        if law_requirements.iter().any(|r| {
            let r = r.clone();
            r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseLeft
        }) {
            Ok(ValidatedLawRequirement(value))
        } else {
            Err("GroupInverseLeftLaw invariant violated")
        }
    }
}

/// Newtype wrapper: AlgebraicDeclaration validated by GroupRequiresInverse.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedAlgebraicDeclaration(pub AlgebraicDeclaration);

impl TryFrom<AlgebraicDeclaration> for ValidatedAlgebraicDeclaration {
    type Error = &'static str;

    fn try_from(value: AlgebraicDeclaration) -> Result<Self, Self::Error> {
        if value.targetStructure.rank() >= 2 && !(value.identityName.is_some()) {
            return Err(
                "AlgebraicDeclaration.targetStructure.rank() >= 2 implies some AlgebraicDeclaration.identityName",
            );
        }
        if value.targetStructure.rank() >= 3 && !(value.inverseName.is_some()) {
            return Err(
                "AlgebraicDeclaration.targetStructure.rank() >= 3 implies some AlgebraicDeclaration.inverseName",
            );
        }
        if value.identityName.is_some() && value.identityName.as_ref() == Some(&value.operationName)
        {
            return Err(
                "some AlgebraicDeclaration.identityName implies AlgebraicDeclaration.identityName != AlgebraicDeclaration.operationName",
            );
        }
        let algebraic_declarations: Vec<AlgebraicDeclaration> = vec![value.clone()];
        if algebraic_declarations.iter().all(|d| {
            let d = d.clone();
            !(d.targetStructure.rank() >= 3) || d.inverseName.is_some()
        }) {
            Ok(ValidatedAlgebraicDeclaration(value))
        } else {
            Err("GroupRequiresInverse invariant violated")
        }
    }
}

/// Newtype wrapper: IgnoreAnnotation validated by IgnoredSuppressesDiagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedIgnoreAnnotation(pub IgnoreAnnotation);

impl TryFrom<IgnoreAnnotation> for ValidatedIgnoreAnnotation {
    type Error = &'static str;

    fn try_from(value: IgnoreAnnotation) -> Result<Self, Self::Error> {
        let diagnostics: Vec<Diagnostic> = Vec::new();
        let ignore_annotations: Vec<IgnoreAnnotation> = vec![value.clone()];
        if ignore_annotations.iter().all(|i| {
            let i = i.clone();
            !diagnostics.iter().any(|d| {
                let d = d.clone();
                d.declaration == i.declaration
            })
        }) {
            Ok(ValidatedIgnoreAnnotation(value))
        } else {
            Err("IgnoredSuppressesDiagnostics invariant violated")
        }
    }
}

/// Newtype wrapper: LawTest validated by LawTestRelevance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedLawTest(pub LawTest);

impl TryFrom<LawTest> for ValidatedLawTest {
    type Error = &'static str;

    fn try_from(value: LawTest) -> Result<Self, Self::Error> {
        let law_requirements: Vec<LawRequirement> = Vec::new();
        let law_tests: Vec<LawTest> = vec![value.clone()];
        if law_tests.iter().all(|t| {
            let t = t.clone();
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == t.declaration.targetStructure && r.requiredLaw == t.law
            })
        }) {
            Ok(ValidatedLawTest(value))
        } else {
            Err("LawTestRelevance invariant violated")
        }
    }
}

/// Newtype wrapper: ComplianceReport validated by ValidComplianceCounts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedComplianceReport(pub ComplianceReport);

impl TryFrom<ComplianceReport> for ValidatedComplianceReport {
    type Error = &'static str;

    fn try_from(value: ComplianceReport) -> Result<Self, Self::Error> {
        if value.passingLaws > value.totalLaws {
            return Err("passingLaws must be <= totalLaws");
        }
        let compliance_reports: Vec<ComplianceReport> = vec![value.clone()];
        if compliance_reports.iter().all(|r| {
            let r = r.clone();
            r.passingLaws >= 0 && r.passingLaws <= r.totalLaws && r.totalLaws > 0
        }) {
            Ok(ValidatedComplianceReport(value))
        } else {
            Err("ValidComplianceCounts invariant violated")
        }
    }
}

/// Newtype wrapper: LayerExpectation validated by ValidMaxPropagation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedLayerExpectation(pub LayerExpectation);

impl TryFrom<LayerExpectation> for ValidatedLayerExpectation {
    type Error = &'static str;

    fn try_from(value: LayerExpectation) -> Result<Self, Self::Error> {
        if value.maxPropagation.is_some() && !value.maxPropagation.is_none_or(|v| v > 0 || v == -1)
        {
            return Err(
                "some LayerExpectation.maxPropagation implies LayerExpectation.maxPropagation > 0 or LayerExpectation.maxPropagation = -1",
            );
        }
        let layer_expectations: Vec<LayerExpectation> = vec![value.clone()];
        if layer_expectations.iter().all(|l| {
            let l = l.clone();
            !l.maxPropagation.is_some() || l.maxPropagation.is_none_or(|v| v > 0 || v == -1)
        }) {
            Ok(ValidatedLayerExpectation(value))
        } else {
            Err("ValidMaxPropagation invariant violated")
        }
    }
}
