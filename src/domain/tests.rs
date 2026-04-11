#[cfg(test)]
mod property_tests {
    #[allow(unused_imports)]
    use crate::domain::fixtures::*;
    use crate::domain::konpu::diagnostic_rule::DiagnosticRule::*;
    use crate::domain::konpu::propagation_size::PropagationSize::*;
    use crate::domain::konpu::severity::Severity::*;
    #[allow(unused_imports)]
    use crate::domain::konpu::*;

    #[test]
    fn monoid_integrity() {
        let decl = default_algebraic_declaration();
        assert!(decl.targetStructure.rank() < 2 || decl.identityName.is_some());
    }

    #[test]
    fn group_integrity() {
        let decl = default_algebraic_declaration();
        assert!(decl.targetStructure.rank() < 3 || decl.inverseName.is_some());
    }

    #[test]
    fn invariant_magma_rank() {
        assert!(AlgebraicStructure::Magma.rank() == 0);
    }

    #[test]
    fn invariant_semigroup_rank() {
        assert!(AlgebraicStructure::Semigroup.rank() == 1);
    }

    #[test]
    fn invariant_monoid_rank() {
        assert!(AlgebraicStructure::Monoid.rank() == 2);
    }

    #[test]
    fn invariant_group_rank() {
        assert!(AlgebraicStructure::Group.rank() == 3);
    }

    #[test]
    fn invariant_functor_rank() {
        assert!(HigherKindedStructure::Functor.hkRank() == 1);
    }

    #[test]
    fn invariant_applicative_rank() {
        assert!(HigherKindedStructure::Applicative.hkRank() == 2);
    }

    #[test]
    fn invariant_monad_s_rank() {
        assert!(HigherKindedStructure::MonadS.hkRank() == 3);
    }

    #[test]
    fn invariant_monoid_requires_identity() {
        let algebraic_declarations: Vec<AlgebraicDeclaration> =
            vec![default_algebraic_declaration()];
        assert!(algebraic_declarations.iter().all(|d| {
            let d = d.clone();
            !(d.targetStructure.rank() >= 2) || d.identityName.is_some()
        }));
    }

    #[test]
    fn invariant_group_requires_inverse() {
        let algebraic_declarations: Vec<AlgebraicDeclaration> =
            vec![default_algebraic_declaration()];
        assert!(algebraic_declarations.iter().all(|d| {
            let d = d.clone();
            !(d.targetStructure.rank() >= 3) || d.inverseName.is_some()
        }));
    }

    #[test]
    fn invariant_identity_distinct_from_op() {
        let algebraic_declarations: Vec<AlgebraicDeclaration> =
            vec![default_algebraic_declaration()];
        assert!(algebraic_declarations.iter().all(|d| {
            let d = d.clone();
            !d.identityName.is_some() || d.identityName.as_ref() != Some(&d.operationName)
        }));
    }

    #[test]
    fn invariant_semigroup_laws() {
        let law_requirements = all_law_requirements();
        assert!(law_requirements.iter().any(|r| {
            let r = r.clone();
            r.structure == AlgebraicStructure::Semigroup && r.requiredLaw == Law::Associativity
        }));
    }

    #[test]
    fn invariant_monoid_left_identity_law() {
        let law_requirements = all_law_requirements();
        assert!(law_requirements.iter().any(|r| {
            let r = r.clone();
            r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::LeftIdentity
        }));
    }

    #[test]
    fn invariant_monoid_right_identity_law() {
        let law_requirements = all_law_requirements();
        assert!(law_requirements.iter().any(|r| {
            let r = r.clone();
            r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::RightIdentity
        }));
    }

    #[test]
    fn invariant_group_inverse_left_law() {
        let law_requirements = all_law_requirements();
        assert!(law_requirements.iter().any(|r| {
            let r = r.clone();
            r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseLeft
        }));
    }

    #[test]
    fn invariant_group_inverse_right_law() {
        let law_requirements = all_law_requirements();
        assert!(law_requirements.iter().any(|r| {
            let r = r.clone();
            r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseRight
        }));
    }

    #[test]
    fn invariant_law_test_relevance() {
        let law_requirements = all_law_requirements();
        let law_tests: Vec<LawTest> = vec![default_law_test()];
        assert!(law_tests.iter().all(|t| {
            let t = t.clone();
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == t.declaration.targetStructure && r.requiredLaw == t.law
            })
        }));
    }

    #[test]
    fn invariant_ignored_suppresses_diagnostics() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        let ignore_annotations: Vec<IgnoreAnnotation> = Vec::new();
        assert!(ignore_annotations.iter().all(|i| {
            let i = i.clone();
            !diagnostics.iter().any(|d| {
                let d = d.clone();
                d.declaration == i.declaration
            })
        }));
    }

    #[test]
    fn invariant_missing_identity_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(diagnostics.iter().all(|d| {
            let d = d.clone();
            !(d.rule == DiagnosticRule::MissingIdentity) || d.severity == Severity::Error
        }));
    }

    #[test]
    fn invariant_missing_inverse_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(diagnostics.iter().all(|d| {
            let d = d.clone();
            !(d.rule == DiagnosticRule::MissingInverse) || d.severity == Severity::Error
        }));
    }

    #[test]
    fn invariant_closure_violation_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(diagnostics.iter().all(|d| {
            let d = d.clone();
            !(d.rule == DiagnosticRule::ClosureViolation) || d.severity == Severity::Error
        }));
    }

    #[test]
    fn invariant_missing_law_test_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(diagnostics.iter().all(|d| {
            let d = d.clone();
            !(d.rule == DiagnosticRule::MissingLawTest) || d.severity == Severity::Warning
        }));
    }

    #[test]
    fn invariant_failing_law_test_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(diagnostics.iter().all(|d| {
            let d = d.clone();
            !(d.rule == DiagnosticRule::FailingLawTest) || d.severity == Severity::Error
        }));
    }

    #[test]
    fn invariant_propagation_exceeded_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(diagnostics.iter().all(|d| {
            let d = d.clone();
            !(d.rule == DiagnosticRule::PropagationExceeded) || d.severity == Severity::Warning
        }));
    }

    #[test]
    fn invariant_associativity_confidence_is_info() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(diagnostics.iter().all(|d| {
            let d = d.clone();
            !(d.rule == DiagnosticRule::AssociativityConfidence) || d.severity == Severity::Info
        }));
    }

    #[test]
    fn invariant_finite_has_count() {
        let context_types: Vec<ContextType> = vec![default_context_type()];
        assert!(context_types.iter().all(|c| {
            let c = c.clone();
            !(c.propagation == PropagationSize::Finite) || c.variantCount.is_some()
        }));
    }

    #[test]
    fn invariant_unbounded_has_no_count() {
        let context_types: Vec<ContextType> = vec![default_context_type()];
        assert!(context_types.iter().all(|c| {
            let c = c.clone();
            !(c.propagation == PropagationSize::Unbounded) || c.variantCount.is_none()
        }));
    }

    #[test]
    fn invariant_count_is_positive() {
        let context_types: Vec<ContextType> = vec![default_context_type()];
        assert!(context_types.iter().all(|c| {
            let c = c.clone();
            !c.variantCount.is_some() || c.variantCount.is_none_or(|v| v > 0)
        }));
    }

    #[test]
    fn invariant_valid_max_propagation() {
        let layer_expectations: Vec<LayerExpectation> = vec![default_layer_expectation()];
        assert!(layer_expectations.iter().all(|l| {
            let l = l.clone();
            !l.maxPropagation.is_some() || l.maxPropagation.is_none_or(|v| v > 0 || v == -1)
        }));
    }

    #[test]
    fn invariant_valid_compliance_counts() {
        let compliance_reports: Vec<ComplianceReport> = vec![default_compliance_report()];
        assert!(compliance_reports.iter().all(|r| {
            let r = r.clone();
            r.passingLaws >= 0 && r.passingLaws <= r.totalLaws && r.totalLaws > 0
        }));
    }
}
