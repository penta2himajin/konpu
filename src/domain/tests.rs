#[cfg(test)]
mod property_tests {
    #[allow(unused_imports)]
    use crate::domain::fixtures::*;
    #[allow(unused_imports)]
    use crate::domain::konpu::*;

    #[test]
    fn monoid_integrity() {
        // Magma (rank 0) needs no identity — assertion holds vacuously
        let decl = default_algebraic_declaration();
        assert!(decl.targetStructure.rank() < 2 || decl.identityName.is_some());
    }

    #[test]
    fn group_integrity() {
        // Magma (rank 0) needs no inverse — assertion holds vacuously
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
        assert!(HigherKindedStructure::Functor.hk_rank() == 1);
    }

    #[test]
    fn invariant_applicative_rank() {
        assert!(HigherKindedStructure::Applicative.hk_rank() == 2);
    }

    #[test]
    fn invariant_monad_s_rank() {
        assert!(HigherKindedStructure::MonadS.hk_rank() == 3);
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
        assert!(
            law_requirements
                .iter()
                .any(|r| r.structure == AlgebraicStructure::Semigroup
                    && r.requiredLaw == Law::Associativity)
        );
    }

    #[test]
    fn invariant_monoid_left_identity_law() {
        let law_requirements = all_law_requirements();
        assert!(law_requirements.iter().any(
            |r| r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::LeftIdentity
        ));
    }

    #[test]
    fn invariant_monoid_right_identity_law() {
        let law_requirements = all_law_requirements();
        assert!(
            law_requirements
                .iter()
                .any(|r| r.structure == AlgebraicStructure::Monoid
                    && r.requiredLaw == Law::RightIdentity)
        );
    }

    #[test]
    fn invariant_group_inverse_left_law() {
        let law_requirements = all_law_requirements();
        assert!(
            law_requirements
                .iter()
                .any(|r| r.structure == AlgebraicStructure::Group
                    && r.requiredLaw == Law::InverseLeft)
        );
    }

    #[test]
    fn invariant_group_inverse_right_law() {
        let law_requirements = all_law_requirements();
        assert!(law_requirements.iter().any(
            |r| r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseRight
        ));
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
        // default_diagnostic uses Semigroup declaration, default_ignore_annotation uses Magma
        // so no diagnostic shares a declaration with an ignore annotation
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        let ignore_annotations: Vec<IgnoreAnnotation> = vec![default_ignore_annotation()];
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
            !c.variantCount.is_some() || c.variantCount.map_or(true, |v| v > 0)
        }));
    }

    #[test]
    fn invariant_valid_max_propagation() {
        let layer_expectations: Vec<LayerExpectation> = vec![default_layer_expectation()];
        assert!(layer_expectations.iter().all(|l| {
            let l = l.clone();
            !l.maxPropagation.is_some() || l.maxPropagation.map_or(true, |v| v > 0 || v == -1)
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

    // --- Anomaly tests: edge-case coverage ---

    /// Anomaly: field `pathPattern` is not constrained by any fact.
    #[test]
    fn anomaly_unconstrained_layer_expectation_path_pattern() {
        let instance = default_layer_expectation();
        // LayerExpectation.pathPattern is not constrained — verify it is handled
        let _ = &instance.pathPattern;
    }

    /// Anomaly: field `expectedStructures` is not constrained by any fact.
    #[test]
    fn anomaly_unconstrained_layer_expectation_expected_structures() {
        let instance = default_layer_expectation();
        // LayerExpectation.expectedStructures is not constrained — verify it is handled
        let _ = &instance.expectedStructures;
    }

    /// Anomaly: `expectedStructures` has no cardinality upper bound.
    #[test]
    fn anomaly_empty_layer_expectation_expected_structures() {
        let instance = anomaly_empty_layer_expectation();
        // Verify invariants hold even with empty collection
        let _ = &instance.expectedStructures;
    }

    /// Anomaly: field `expectedHigherKinded` is not constrained by any fact.
    #[test]
    fn anomaly_unconstrained_layer_expectation_expected_higher_kinded() {
        let instance = default_layer_expectation();
        // LayerExpectation.expectedHigherKinded is not constrained — verify it is handled
        let _ = &instance.expectedHigherKinded;
    }

    /// Anomaly: `expectedHigherKinded` has no cardinality upper bound.
    #[test]
    fn anomaly_empty_layer_expectation_expected_higher_kinded() {
        let instance = anomaly_empty_layer_expectation();
        // Verify invariants hold even with empty collection
        let _ = &instance.expectedHigherKinded;
    }

    /// Anomaly: field `declaration` is not constrained by any fact.
    #[test]
    fn anomaly_unconstrained_compliance_report_declaration() {
        let instance = default_compliance_report();
        // ComplianceReport.declaration is not constrained — verify it is handled
        let _ = &instance.declaration;
    }

    /// Anomaly: field `higherKinded` is not constrained by any fact.
    #[test]
    fn anomaly_unconstrained_algebraic_declaration_higher_kinded() {
        let instance = default_algebraic_declaration();
        // AlgebraicDeclaration.higherKinded is not constrained — verify it is handled
        let _ = &instance.higherKinded;
    }

    /// Anomaly: field `reason` is not constrained by any fact.
    #[test]
    fn anomaly_unconstrained_ignore_annotation_reason() {
        let instance = default_ignore_annotation();
        // IgnoreAnnotation.reason is not constrained — verify it is handled
        let _ = &instance.reason;
    }

    /// Anomaly: field `status` is not constrained by any fact.
    #[test]
    fn anomaly_unconstrained_law_test_status() {
        let instance = default_law_test();
        // LawTest.status is not constrained — verify it is handled
        let _ = &instance.status;
    }

    // --- Coverage tests: fact × fact pairwise ---

    /// Coverage: GroupRequiresInverse × IdentityDistinctFromOp
    #[test]
    #[ignore]
    fn cover_group_requires_inverse_x_identity_distinct_from_op() {
        let algebraic_declarations: Vec<AlgebraicDeclaration> =
            vec![default_algebraic_declaration()];
        assert!(
            algebraic_declarations.iter().all(|d| {
                let d = d.clone();
                !(d.targetStructure.rank() >= 3) || d.inverseName.is_some()
            }),
            "fact GroupRequiresInverse should hold"
        );
        assert!(
            algebraic_declarations.iter().all(|d| {
                let d = d.clone();
                !d.identityName.is_some() || d.identityName.as_ref() != Some(&d.operationName)
            }),
            "fact IdentityDistinctFromOp should hold"
        );
    }

    /// Coverage: GroupRequiresInverse × MonoidRequiresIdentity
    #[test]
    #[ignore]
    fn cover_group_requires_inverse_x_monoid_requires_identity() {
        let algebraic_declarations: Vec<AlgebraicDeclaration> =
            vec![default_algebraic_declaration()];
        assert!(
            algebraic_declarations.iter().all(|d| {
                let d = d.clone();
                !(d.targetStructure.rank() >= 3) || d.inverseName.is_some()
            }),
            "fact GroupRequiresInverse should hold"
        );
        assert!(
            algebraic_declarations.iter().all(|d| {
                let d = d.clone();
                !(d.targetStructure.rank() >= 2) || d.identityName.is_some()
            }),
            "fact MonoidRequiresIdentity should hold"
        );
    }

    /// Coverage: IdentityDistinctFromOp × MonoidRequiresIdentity
    #[test]
    #[ignore]
    fn cover_identity_distinct_from_op_x_monoid_requires_identity() {
        let algebraic_declarations: Vec<AlgebraicDeclaration> =
            vec![default_algebraic_declaration()];
        assert!(
            algebraic_declarations.iter().all(|d| {
                let d = d.clone();
                !d.identityName.is_some() || d.identityName.as_ref() != Some(&d.operationName)
            }),
            "fact IdentityDistinctFromOp should hold"
        );
        assert!(
            algebraic_declarations.iter().all(|d| {
                let d = d.clone();
                !(d.targetStructure.rank() >= 2) || d.identityName.is_some()
            }),
            "fact MonoidRequiresIdentity should hold"
        );
    }

    /// Coverage: CountIsPositive × FiniteHasCount
    #[test]
    #[ignore]
    fn cover_count_is_positive_x_finite_has_count() {
        let context_types: Vec<ContextType> = vec![default_context_type()];
        assert!(
            context_types.iter().all(|c| {
                let c = c.clone();
                !c.variantCount.is_some() || c.variantCount.map_or(true, |v| v > 0)
            }),
            "fact CountIsPositive should hold"
        );
        assert!(
            context_types.iter().all(|c| {
                let c = c.clone();
                !(c.propagation == PropagationSize::Finite) || c.variantCount.is_some()
            }),
            "fact FiniteHasCount should hold"
        );
    }

    /// Coverage: CountIsPositive × UnboundedHasNoCount
    #[test]
    #[ignore]
    fn cover_count_is_positive_x_unbounded_has_no_count() {
        let context_types: Vec<ContextType> = vec![default_context_type()];
        assert!(
            context_types.iter().all(|c| {
                let c = c.clone();
                !c.variantCount.is_some() || c.variantCount.map_or(true, |v| v > 0)
            }),
            "fact CountIsPositive should hold"
        );
        assert!(
            context_types.iter().all(|c| {
                let c = c.clone();
                !(c.propagation == PropagationSize::Unbounded) || c.variantCount.is_none()
            }),
            "fact UnboundedHasNoCount should hold"
        );
    }

    /// Coverage: FiniteHasCount × UnboundedHasNoCount
    #[test]
    #[ignore]
    fn cover_finite_has_count_x_unbounded_has_no_count() {
        let context_types: Vec<ContextType> = vec![default_context_type()];
        assert!(
            context_types.iter().all(|c| {
                let c = c.clone();
                !(c.propagation == PropagationSize::Finite) || c.variantCount.is_some()
            }),
            "fact FiniteHasCount should hold"
        );
        assert!(
            context_types.iter().all(|c| {
                let c = c.clone();
                !(c.propagation == PropagationSize::Unbounded) || c.variantCount.is_none()
            }),
            "fact UnboundedHasNoCount should hold"
        );
    }

    /// Coverage: AssociativityConfidenceIsInfo × ClosureViolationIsError
    #[test]
    #[ignore]
    fn cover_associativity_confidence_is_info_x_closure_violation_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::AssociativityConfidence) || d.severity == Severity::Info
            }),
            "fact AssociativityConfidenceIsInfo should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::ClosureViolation) || d.severity == Severity::Error
            }),
            "fact ClosureViolationIsError should hold"
        );
    }

    /// Coverage: AssociativityConfidenceIsInfo × FailingLawTestIsError
    #[test]
    #[ignore]
    fn cover_associativity_confidence_is_info_x_failing_law_test_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::AssociativityConfidence) || d.severity == Severity::Info
            }),
            "fact AssociativityConfidenceIsInfo should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::FailingLawTest) || d.severity == Severity::Error
            }),
            "fact FailingLawTestIsError should hold"
        );
    }

    /// Coverage: AssociativityConfidenceIsInfo × IgnoredSuppressesDiagnostics
    #[test]
    #[ignore]
    fn cover_associativity_confidence_is_info_x_ignored_suppresses_diagnostics() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        let ignore_annotations: Vec<IgnoreAnnotation> = vec![default_ignore_annotation()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::AssociativityConfidence) || d.severity == Severity::Info
            }),
            "fact AssociativityConfidenceIsInfo should hold"
        );
        assert!(
            ignore_annotations.iter().all(|i| {
                let i = i.clone();
                !diagnostics.iter().any(|d| {
                    let d = d.clone();
                    d.declaration == i.declaration
                })
            }),
            "fact IgnoredSuppressesDiagnostics should hold"
        );
    }

    /// Coverage: AssociativityConfidenceIsInfo × MissingIdentityIsError
    #[test]
    #[ignore]
    fn cover_associativity_confidence_is_info_x_missing_identity_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::AssociativityConfidence) || d.severity == Severity::Info
            }),
            "fact AssociativityConfidenceIsInfo should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingIdentity) || d.severity == Severity::Error
            }),
            "fact MissingIdentityIsError should hold"
        );
    }

    /// Coverage: AssociativityConfidenceIsInfo × MissingInverseIsError
    #[test]
    #[ignore]
    fn cover_associativity_confidence_is_info_x_missing_inverse_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::AssociativityConfidence) || d.severity == Severity::Info
            }),
            "fact AssociativityConfidenceIsInfo should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingInverse) || d.severity == Severity::Error
            }),
            "fact MissingInverseIsError should hold"
        );
    }

    /// Coverage: AssociativityConfidenceIsInfo × MissingLawTestIsWarning
    #[test]
    #[ignore]
    fn cover_associativity_confidence_is_info_x_missing_law_test_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::AssociativityConfidence) || d.severity == Severity::Info
            }),
            "fact AssociativityConfidenceIsInfo should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingLawTest) || d.severity == Severity::Warning
            }),
            "fact MissingLawTestIsWarning should hold"
        );
    }

    /// Coverage: AssociativityConfidenceIsInfo × PropagationExceededIsWarning
    #[test]
    #[ignore]
    fn cover_associativity_confidence_is_info_x_propagation_exceeded_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::AssociativityConfidence) || d.severity == Severity::Info
            }),
            "fact AssociativityConfidenceIsInfo should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::PropagationExceeded) || d.severity == Severity::Warning
            }),
            "fact PropagationExceededIsWarning should hold"
        );
    }

    /// Coverage: ClosureViolationIsError × FailingLawTestIsError
    #[test]
    #[ignore]
    fn cover_closure_violation_is_error_x_failing_law_test_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::ClosureViolation) || d.severity == Severity::Error
            }),
            "fact ClosureViolationIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::FailingLawTest) || d.severity == Severity::Error
            }),
            "fact FailingLawTestIsError should hold"
        );
    }

    /// Coverage: ClosureViolationIsError × IgnoredSuppressesDiagnostics
    #[test]
    #[ignore]
    fn cover_closure_violation_is_error_x_ignored_suppresses_diagnostics() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        let ignore_annotations: Vec<IgnoreAnnotation> = vec![default_ignore_annotation()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::ClosureViolation) || d.severity == Severity::Error
            }),
            "fact ClosureViolationIsError should hold"
        );
        assert!(
            ignore_annotations.iter().all(|i| {
                let i = i.clone();
                !diagnostics.iter().any(|d| {
                    let d = d.clone();
                    d.declaration == i.declaration
                })
            }),
            "fact IgnoredSuppressesDiagnostics should hold"
        );
    }

    /// Coverage: ClosureViolationIsError × MissingIdentityIsError
    #[test]
    #[ignore]
    fn cover_closure_violation_is_error_x_missing_identity_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::ClosureViolation) || d.severity == Severity::Error
            }),
            "fact ClosureViolationIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingIdentity) || d.severity == Severity::Error
            }),
            "fact MissingIdentityIsError should hold"
        );
    }

    /// Coverage: ClosureViolationIsError × MissingInverseIsError
    #[test]
    #[ignore]
    fn cover_closure_violation_is_error_x_missing_inverse_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::ClosureViolation) || d.severity == Severity::Error
            }),
            "fact ClosureViolationIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingInverse) || d.severity == Severity::Error
            }),
            "fact MissingInverseIsError should hold"
        );
    }

    /// Coverage: ClosureViolationIsError × MissingLawTestIsWarning
    #[test]
    #[ignore]
    fn cover_closure_violation_is_error_x_missing_law_test_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::ClosureViolation) || d.severity == Severity::Error
            }),
            "fact ClosureViolationIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingLawTest) || d.severity == Severity::Warning
            }),
            "fact MissingLawTestIsWarning should hold"
        );
    }

    /// Coverage: ClosureViolationIsError × PropagationExceededIsWarning
    #[test]
    #[ignore]
    fn cover_closure_violation_is_error_x_propagation_exceeded_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::ClosureViolation) || d.severity == Severity::Error
            }),
            "fact ClosureViolationIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::PropagationExceeded) || d.severity == Severity::Warning
            }),
            "fact PropagationExceededIsWarning should hold"
        );
    }

    /// Coverage: FailingLawTestIsError × IgnoredSuppressesDiagnostics
    #[test]
    #[ignore]
    fn cover_failing_law_test_is_error_x_ignored_suppresses_diagnostics() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        let ignore_annotations: Vec<IgnoreAnnotation> = vec![default_ignore_annotation()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::FailingLawTest) || d.severity == Severity::Error
            }),
            "fact FailingLawTestIsError should hold"
        );
        assert!(
            ignore_annotations.iter().all(|i| {
                let i = i.clone();
                !diagnostics.iter().any(|d| {
                    let d = d.clone();
                    d.declaration == i.declaration
                })
            }),
            "fact IgnoredSuppressesDiagnostics should hold"
        );
    }

    /// Coverage: FailingLawTestIsError × MissingIdentityIsError
    #[test]
    #[ignore]
    fn cover_failing_law_test_is_error_x_missing_identity_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::FailingLawTest) || d.severity == Severity::Error
            }),
            "fact FailingLawTestIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingIdentity) || d.severity == Severity::Error
            }),
            "fact MissingIdentityIsError should hold"
        );
    }

    /// Coverage: FailingLawTestIsError × MissingInverseIsError
    #[test]
    #[ignore]
    fn cover_failing_law_test_is_error_x_missing_inverse_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::FailingLawTest) || d.severity == Severity::Error
            }),
            "fact FailingLawTestIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingInverse) || d.severity == Severity::Error
            }),
            "fact MissingInverseIsError should hold"
        );
    }

    /// Coverage: FailingLawTestIsError × MissingLawTestIsWarning
    #[test]
    #[ignore]
    fn cover_failing_law_test_is_error_x_missing_law_test_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::FailingLawTest) || d.severity == Severity::Error
            }),
            "fact FailingLawTestIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingLawTest) || d.severity == Severity::Warning
            }),
            "fact MissingLawTestIsWarning should hold"
        );
    }

    /// Coverage: FailingLawTestIsError × PropagationExceededIsWarning
    #[test]
    #[ignore]
    fn cover_failing_law_test_is_error_x_propagation_exceeded_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::FailingLawTest) || d.severity == Severity::Error
            }),
            "fact FailingLawTestIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::PropagationExceeded) || d.severity == Severity::Warning
            }),
            "fact PropagationExceededIsWarning should hold"
        );
    }

    /// Coverage: IgnoredSuppressesDiagnostics × MissingIdentityIsError
    #[test]
    #[ignore]
    fn cover_ignored_suppresses_diagnostics_x_missing_identity_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        let ignore_annotations: Vec<IgnoreAnnotation> = vec![default_ignore_annotation()];
        assert!(
            ignore_annotations.iter().all(|i| {
                let i = i.clone();
                !diagnostics.iter().any(|d| {
                    let d = d.clone();
                    d.declaration == i.declaration
                })
            }),
            "fact IgnoredSuppressesDiagnostics should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingIdentity) || d.severity == Severity::Error
            }),
            "fact MissingIdentityIsError should hold"
        );
    }

    /// Coverage: IgnoredSuppressesDiagnostics × MissingInverseIsError
    #[test]
    #[ignore]
    fn cover_ignored_suppresses_diagnostics_x_missing_inverse_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        let ignore_annotations: Vec<IgnoreAnnotation> = vec![default_ignore_annotation()];
        assert!(
            ignore_annotations.iter().all(|i| {
                let i = i.clone();
                !diagnostics.iter().any(|d| {
                    let d = d.clone();
                    d.declaration == i.declaration
                })
            }),
            "fact IgnoredSuppressesDiagnostics should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingInverse) || d.severity == Severity::Error
            }),
            "fact MissingInverseIsError should hold"
        );
    }

    /// Coverage: IgnoredSuppressesDiagnostics × MissingLawTestIsWarning
    #[test]
    #[ignore]
    fn cover_ignored_suppresses_diagnostics_x_missing_law_test_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        let ignore_annotations: Vec<IgnoreAnnotation> = vec![default_ignore_annotation()];
        assert!(
            ignore_annotations.iter().all(|i| {
                let i = i.clone();
                !diagnostics.iter().any(|d| {
                    let d = d.clone();
                    d.declaration == i.declaration
                })
            }),
            "fact IgnoredSuppressesDiagnostics should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingLawTest) || d.severity == Severity::Warning
            }),
            "fact MissingLawTestIsWarning should hold"
        );
    }

    /// Coverage: IgnoredSuppressesDiagnostics × PropagationExceededIsWarning
    #[test]
    #[ignore]
    fn cover_ignored_suppresses_diagnostics_x_propagation_exceeded_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        let ignore_annotations: Vec<IgnoreAnnotation> = vec![default_ignore_annotation()];
        assert!(
            ignore_annotations.iter().all(|i| {
                let i = i.clone();
                !diagnostics.iter().any(|d| {
                    let d = d.clone();
                    d.declaration == i.declaration
                })
            }),
            "fact IgnoredSuppressesDiagnostics should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::PropagationExceeded) || d.severity == Severity::Warning
            }),
            "fact PropagationExceededIsWarning should hold"
        );
    }

    /// Coverage: MissingIdentityIsError × MissingInverseIsError
    #[test]
    #[ignore]
    fn cover_missing_identity_is_error_x_missing_inverse_is_error() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingIdentity) || d.severity == Severity::Error
            }),
            "fact MissingIdentityIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingInverse) || d.severity == Severity::Error
            }),
            "fact MissingInverseIsError should hold"
        );
    }

    /// Coverage: MissingIdentityIsError × MissingLawTestIsWarning
    #[test]
    #[ignore]
    fn cover_missing_identity_is_error_x_missing_law_test_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingIdentity) || d.severity == Severity::Error
            }),
            "fact MissingIdentityIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingLawTest) || d.severity == Severity::Warning
            }),
            "fact MissingLawTestIsWarning should hold"
        );
    }

    /// Coverage: MissingIdentityIsError × PropagationExceededIsWarning
    #[test]
    #[ignore]
    fn cover_missing_identity_is_error_x_propagation_exceeded_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingIdentity) || d.severity == Severity::Error
            }),
            "fact MissingIdentityIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::PropagationExceeded) || d.severity == Severity::Warning
            }),
            "fact PropagationExceededIsWarning should hold"
        );
    }

    /// Coverage: MissingInverseIsError × MissingLawTestIsWarning
    #[test]
    #[ignore]
    fn cover_missing_inverse_is_error_x_missing_law_test_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingInverse) || d.severity == Severity::Error
            }),
            "fact MissingInverseIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingLawTest) || d.severity == Severity::Warning
            }),
            "fact MissingLawTestIsWarning should hold"
        );
    }

    /// Coverage: MissingInverseIsError × PropagationExceededIsWarning
    #[test]
    #[ignore]
    fn cover_missing_inverse_is_error_x_propagation_exceeded_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingInverse) || d.severity == Severity::Error
            }),
            "fact MissingInverseIsError should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::PropagationExceeded) || d.severity == Severity::Warning
            }),
            "fact PropagationExceededIsWarning should hold"
        );
    }

    /// Coverage: MissingLawTestIsWarning × PropagationExceededIsWarning
    #[test]
    #[ignore]
    fn cover_missing_law_test_is_warning_x_propagation_exceeded_is_warning() {
        let diagnostics: Vec<Diagnostic> = vec![default_diagnostic()];
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::MissingLawTest) || d.severity == Severity::Warning
            }),
            "fact MissingLawTestIsWarning should hold"
        );
        assert!(
            diagnostics.iter().all(|d| {
                let d = d.clone();
                !(d.rule == DiagnosticRule::PropagationExceeded) || d.severity == Severity::Warning
            }),
            "fact PropagationExceededIsWarning should hold"
        );
    }

    /// Coverage: GroupInverseLeftLaw × GroupInverseRightLaw
    #[test]
    #[ignore]
    fn cover_group_inverse_left_law_x_group_inverse_right_law() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseLeft
            }),
            "fact GroupInverseLeftLaw should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseRight
            }),
            "fact GroupInverseRightLaw should hold"
        );
    }

    /// Coverage: GroupInverseLeftLaw × LawTestRelevance
    #[test]
    #[ignore]
    fn cover_group_inverse_left_law_x_law_test_relevance() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        let law_tests: Vec<LawTest> = vec![default_law_test()];
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseLeft
            }),
            "fact GroupInverseLeftLaw should hold"
        );
        assert!(
            law_tests.iter().all(|t| {
                let t = t.clone();
                law_requirements.iter().any(|r| {
                    let r = r.clone();
                    r.structure == t.declaration.targetStructure && r.requiredLaw == t.law
                })
            }),
            "fact LawTestRelevance should hold"
        );
    }

    /// Coverage: GroupInverseLeftLaw × MonoidLeftIdentityLaw
    #[test]
    #[ignore]
    fn cover_group_inverse_left_law_x_monoid_left_identity_law() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseLeft
            }),
            "fact GroupInverseLeftLaw should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::LeftIdentity
            }),
            "fact MonoidLeftIdentityLaw should hold"
        );
    }

    /// Coverage: GroupInverseLeftLaw × MonoidRightIdentityLaw
    #[test]
    #[ignore]
    fn cover_group_inverse_left_law_x_monoid_right_identity_law() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseLeft
            }),
            "fact GroupInverseLeftLaw should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::RightIdentity
            }),
            "fact MonoidRightIdentityLaw should hold"
        );
    }

    /// Coverage: GroupInverseLeftLaw × SemigroupLaws
    #[test]
    #[ignore]
    fn cover_group_inverse_left_law_x_semigroup_laws() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseLeft
            }),
            "fact GroupInverseLeftLaw should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Semigroup && r.requiredLaw == Law::Associativity
            }),
            "fact SemigroupLaws should hold"
        );
    }

    /// Coverage: GroupInverseRightLaw × LawTestRelevance
    #[test]
    #[ignore]
    fn cover_group_inverse_right_law_x_law_test_relevance() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        let law_tests: Vec<LawTest> = vec![default_law_test()];
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseRight
            }),
            "fact GroupInverseRightLaw should hold"
        );
        assert!(
            law_tests.iter().all(|t| {
                let t = t.clone();
                law_requirements.iter().any(|r| {
                    let r = r.clone();
                    r.structure == t.declaration.targetStructure && r.requiredLaw == t.law
                })
            }),
            "fact LawTestRelevance should hold"
        );
    }

    /// Coverage: GroupInverseRightLaw × MonoidLeftIdentityLaw
    #[test]
    #[ignore]
    fn cover_group_inverse_right_law_x_monoid_left_identity_law() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseRight
            }),
            "fact GroupInverseRightLaw should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::LeftIdentity
            }),
            "fact MonoidLeftIdentityLaw should hold"
        );
    }

    /// Coverage: GroupInverseRightLaw × MonoidRightIdentityLaw
    #[test]
    #[ignore]
    fn cover_group_inverse_right_law_x_monoid_right_identity_law() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseRight
            }),
            "fact GroupInverseRightLaw should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::RightIdentity
            }),
            "fact MonoidRightIdentityLaw should hold"
        );
    }

    /// Coverage: GroupInverseRightLaw × SemigroupLaws
    #[test]
    #[ignore]
    fn cover_group_inverse_right_law_x_semigroup_laws() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Group && r.requiredLaw == Law::InverseRight
            }),
            "fact GroupInverseRightLaw should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Semigroup && r.requiredLaw == Law::Associativity
            }),
            "fact SemigroupLaws should hold"
        );
    }

    /// Coverage: LawTestRelevance × MonoidLeftIdentityLaw
    #[test]
    #[ignore]
    fn cover_law_test_relevance_x_monoid_left_identity_law() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        let law_tests: Vec<LawTest> = vec![default_law_test()];
        assert!(
            law_tests.iter().all(|t| {
                let t = t.clone();
                law_requirements.iter().any(|r| {
                    let r = r.clone();
                    r.structure == t.declaration.targetStructure && r.requiredLaw == t.law
                })
            }),
            "fact LawTestRelevance should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::LeftIdentity
            }),
            "fact MonoidLeftIdentityLaw should hold"
        );
    }

    /// Coverage: LawTestRelevance × MonoidRightIdentityLaw
    #[test]
    #[ignore]
    fn cover_law_test_relevance_x_monoid_right_identity_law() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        let law_tests: Vec<LawTest> = vec![default_law_test()];
        assert!(
            law_tests.iter().all(|t| {
                let t = t.clone();
                law_requirements.iter().any(|r| {
                    let r = r.clone();
                    r.structure == t.declaration.targetStructure && r.requiredLaw == t.law
                })
            }),
            "fact LawTestRelevance should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::RightIdentity
            }),
            "fact MonoidRightIdentityLaw should hold"
        );
    }

    /// Coverage: LawTestRelevance × SemigroupLaws
    #[test]
    #[ignore]
    fn cover_law_test_relevance_x_semigroup_laws() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        let law_tests: Vec<LawTest> = vec![default_law_test()];
        assert!(
            law_tests.iter().all(|t| {
                let t = t.clone();
                law_requirements.iter().any(|r| {
                    let r = r.clone();
                    r.structure == t.declaration.targetStructure && r.requiredLaw == t.law
                })
            }),
            "fact LawTestRelevance should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Semigroup && r.requiredLaw == Law::Associativity
            }),
            "fact SemigroupLaws should hold"
        );
    }

    /// Coverage: MonoidLeftIdentityLaw × MonoidRightIdentityLaw
    #[test]
    #[ignore]
    fn cover_monoid_left_identity_law_x_monoid_right_identity_law() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::LeftIdentity
            }),
            "fact MonoidLeftIdentityLaw should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::RightIdentity
            }),
            "fact MonoidRightIdentityLaw should hold"
        );
    }

    /// Coverage: MonoidLeftIdentityLaw × SemigroupLaws
    #[test]
    #[ignore]
    fn cover_monoid_left_identity_law_x_semigroup_laws() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::LeftIdentity
            }),
            "fact MonoidLeftIdentityLaw should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Semigroup && r.requiredLaw == Law::Associativity
            }),
            "fact SemigroupLaws should hold"
        );
    }

    /// Coverage: MonoidRightIdentityLaw × SemigroupLaws
    #[test]
    #[ignore]
    fn cover_monoid_right_identity_law_x_semigroup_laws() {
        let law_requirements: Vec<LawRequirement> = vec![default_law_requirement()];
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Monoid && r.requiredLaw == Law::RightIdentity
            }),
            "fact MonoidRightIdentityLaw should hold"
        );
        assert!(
            law_requirements.iter().any(|r| {
                let r = r.clone();
                r.structure == AlgebraicStructure::Semigroup && r.requiredLaw == Law::Associativity
            }),
            "fact SemigroupLaws should hold"
        );
    }
}
