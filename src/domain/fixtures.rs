#[allow(unused_imports)]
use crate::domain::konpu::*;
#[allow(unused_imports)]
use std::collections::BTreeSet;

/// Factory: default value for enum AlgebraicStructure
#[allow(dead_code)]
pub fn default_algebraic_structure() -> AlgebraicStructure {
    AlgebraicStructure::Magma
}

/// Factory: default value for enum HigherKindedStructure
#[allow(dead_code)]
pub fn default_higher_kinded_structure() -> HigherKindedStructure {
    HigherKindedStructure::Functor
}

/// Factory: default value for enum Law
#[allow(dead_code)]
pub fn default_law() -> Law {
    Law::Associativity
}

/// Factory: default value for enum TestStatus
#[allow(dead_code)]
pub fn default_test_status() -> TestStatus {
    TestStatus::Pass
}

/// Factory: default value for enum IgnoreReason
#[allow(dead_code)]
pub fn default_ignore_reason() -> IgnoreReason {
    IgnoreReason::Intentional
}

/// Factory: default value for enum Severity
#[allow(dead_code)]
pub fn default_severity() -> Severity {
    Severity::Error
}

/// Factory: default value for enum DiagnosticRule
#[allow(dead_code)]
pub fn default_diagnostic_rule() -> DiagnosticRule {
    DiagnosticRule::MissingIdentity
}

/// Factory: default value for enum PropagationSize
#[allow(dead_code)]
pub fn default_propagation_size() -> PropagationSize {
    PropagationSize::Finite
}

/// Factory: default value for enum Preset
#[allow(dead_code)]
pub fn default_preset() -> Preset {
    Preset::DDD
}

/// Factory: default value for unit struct OperationName
#[allow(dead_code)]
pub fn default_operation_name() -> OperationName {
    OperationName
}

/// Factory: create a default valid AlgebraicDeclaration (Magma — no identity/inverse required)
#[allow(dead_code)]
pub fn default_algebraic_declaration() -> AlgebraicDeclaration {
    AlgebraicDeclaration {
        targetStructure: default_algebraic_structure(),
        higherKinded: None,
        operationName: default_operation_name(),
        identityName: None,
        inverseName: None,
    }
}

/// Factory: create a default valid LawRequirement
#[allow(dead_code)]
pub fn default_law_requirement() -> LawRequirement {
    LawRequirement {
        structure: AlgebraicStructure::Semigroup,
        requiredLaw: Law::Associativity,
    }
}

/// Factory: create all required law requirements
#[allow(dead_code)]
pub fn all_law_requirements() -> Vec<LawRequirement> {
    vec![
        LawRequirement {
            structure: AlgebraicStructure::Semigroup,
            requiredLaw: Law::Associativity,
        },
        LawRequirement {
            structure: AlgebraicStructure::Monoid,
            requiredLaw: Law::LeftIdentity,
        },
        LawRequirement {
            structure: AlgebraicStructure::Monoid,
            requiredLaw: Law::RightIdentity,
        },
        LawRequirement {
            structure: AlgebraicStructure::Group,
            requiredLaw: Law::InverseLeft,
        },
        LawRequirement {
            structure: AlgebraicStructure::Group,
            requiredLaw: Law::InverseRight,
        },
    ]
}

/// Factory: create a default valid LawTest (Semigroup + Associativity)
#[allow(dead_code)]
pub fn default_law_test() -> LawTest {
    LawTest {
        declaration: AlgebraicDeclaration {
            targetStructure: AlgebraicStructure::Semigroup,
            higherKinded: None,
            operationName: default_operation_name(),
            identityName: None,
            inverseName: None,
        },
        law: Law::Associativity,
        status: default_test_status(),
    }
}

/// Factory: create a default valid IgnoreAnnotation
#[allow(dead_code)]
pub fn default_ignore_annotation() -> IgnoreAnnotation {
    IgnoreAnnotation {
        reason: default_ignore_reason(),
        declaration: default_algebraic_declaration(),
    }
}

/// Factory: create a default valid Diagnostic (non-ignored declaration)
#[allow(dead_code)]
pub fn default_diagnostic() -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        declaration: AlgebraicDeclaration {
            targetStructure: AlgebraicStructure::Semigroup,
            higherKinded: None,
            operationName: OperationName,
            identityName: None,
            inverseName: None,
        },
        rule: DiagnosticRule::MissingIdentity,
    }
}

/// Factory: create a default valid ContextType (Finite with positive count)
#[allow(dead_code)]
pub fn default_context_type() -> ContextType {
    ContextType {
        propagation: PropagationSize::Finite,
        variantCount: Some(1),
    }
}

/// Factory: default value for unit struct PathPattern
#[allow(dead_code)]
pub fn default_path_pattern() -> PathPattern {
    PathPattern
}

/// Factory: create a default valid LayerExpectation
#[allow(dead_code)]
pub fn default_layer_expectation() -> LayerExpectation {
    LayerExpectation {
        pathPattern: default_path_pattern(),
        expectedStructures: BTreeSet::new(),
        expectedHigherKinded: BTreeSet::new(),
        maxPropagation: None,
    }
}

/// Factory: create a default valid ComplianceReport
#[allow(dead_code)]
pub fn default_compliance_report() -> ComplianceReport {
    ComplianceReport {
        declaration: default_algebraic_declaration(),
        totalLaws: 1,
        passingLaws: 0,
    }
}

/// Anomaly fixture: all set/seq fields empty (edge case for unbounded collections)
#[allow(dead_code)]
pub fn anomaly_empty_layer_expectation() -> LayerExpectation {
    LayerExpectation {
        pathPattern: default_path_pattern(),
        expectedStructures: BTreeSet::new(),
        expectedHigherKinded: BTreeSet::new(),
        maxPropagation: None,
    }
}
