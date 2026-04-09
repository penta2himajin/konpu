-- konpu.als
-- Domain model for Konpu: algebraic complexity linter.
-- Source of truth for algebraic structure hierarchy, law requirements,
-- diagnostic rules, and template configuration.

module konpu

-------------------------------------------------------------------------------
-- Algebraic Structure Hierarchy
-------------------------------------------------------------------------------

abstract sig AlgebraicStructure {
  rank: one Int
}
one sig Magma     extends AlgebraicStructure {}
one sig Semigroup extends AlgebraicStructure {}
one sig Monoid    extends AlgebraicStructure {}
one sig Group     extends AlgebraicStructure {}

fact MagmaRank     { Magma.rank = 0 }
fact SemigroupRank { Semigroup.rank = 1 }
fact MonoidRank    { Monoid.rank = 2 }
fact GroupRank     { Group.rank = 3 }

abstract sig HigherKindedStructure {
  hkRank: one Int
}
one sig Functor     extends HigherKindedStructure {}
one sig Applicative extends HigherKindedStructure {}
one sig MonadS      extends HigherKindedStructure {}

fact FunctorRank     { Functor.hkRank = 1 }
fact ApplicativeRank { Applicative.hkRank = 2 }
fact MonadSRank      { MonadS.hkRank = 3 }

-------------------------------------------------------------------------------
-- Algebraic Declaration (type-operation pair)
-------------------------------------------------------------------------------

sig OperationName {}

sig AlgebraicDeclaration {
  targetStructure: one AlgebraicStructure,
  higherKinded: lone HigherKindedStructure,
  operationName: one OperationName,
  identityName: lone OperationName,
  inverseName: lone OperationName
}

fact MonoidRequiresIdentity {
  all d: AlgebraicDeclaration |
    d.targetStructure.rank >= 2 implies some d.identityName
}

fact GroupRequiresInverse {
  all d: AlgebraicDeclaration |
    d.targetStructure.rank >= 3 implies some d.inverseName
}

fact IdentityDistinctFromOp {
  all d: AlgebraicDeclaration |
    some d.identityName implies d.identityName != d.operationName
}

-------------------------------------------------------------------------------
-- Law Requirements
-------------------------------------------------------------------------------

abstract sig Law {}
one sig Associativity      extends Law {}
one sig LeftIdentity       extends Law {}
one sig RightIdentity      extends Law {}
one sig InverseLeft        extends Law {}
one sig InverseRight       extends Law {}
one sig FunctorIdentity    extends Law {}
one sig FunctorComposition extends Law {}
one sig ApplicativeIdentity    extends Law {}
one sig ApplicativeComposition extends Law {}
one sig MonadLeftIdentity  extends Law {}
one sig MonadRightIdentity extends Law {}
one sig MonadAssociativity extends Law {}

sig LawRequirement {
  structure: one AlgebraicStructure,
  requiredLaw: one Law
}

fact SemigroupLaws {
  some r: LawRequirement | r.structure = Semigroup and r.requiredLaw = Associativity
}

fact MonoidLeftIdentityLaw {
  some r: LawRequirement | r.structure = Monoid and r.requiredLaw = LeftIdentity
}

fact MonoidRightIdentityLaw {
  some r: LawRequirement | r.structure = Monoid and r.requiredLaw = RightIdentity
}

fact GroupInverseLeftLaw {
  some r: LawRequirement | r.structure = Group and r.requiredLaw = InverseLeft
}

fact GroupInverseRightLaw {
  some r: LawRequirement | r.structure = Group and r.requiredLaw = InverseRight
}

-------------------------------------------------------------------------------
-- Law Test Status
-------------------------------------------------------------------------------

abstract sig TestStatus {}
one sig Pass    extends TestStatus {}
one sig Fail    extends TestStatus {}
one sig Missing extends TestStatus {}

sig LawTest {
  declaration: one AlgebraicDeclaration,
  law: one Law,
  status: one TestStatus
}

fact LawTestRelevance {
  all t: LawTest |
    some r: LawRequirement |
      r.structure = t.declaration.targetStructure and r.requiredLaw = t.law
}

-------------------------------------------------------------------------------
-- Ignore Annotations
-------------------------------------------------------------------------------

abstract sig IgnoreReason {}
one sig Intentional extends IgnoreReason {}
one sig Debt        extends IgnoreReason {}
one sig Infeasible  extends IgnoreReason {}

sig IgnoreAnnotation {
  reason: one IgnoreReason,
  declaration: one AlgebraicDeclaration
}

fact IgnoredSuppressesDiagnostics {
  all i: IgnoreAnnotation |
    no d: Diagnostic | d.declaration = i.declaration
}

-------------------------------------------------------------------------------
-- Diagnostics
-------------------------------------------------------------------------------

abstract sig Severity {}
one sig Error   extends Severity {}
one sig Warning extends Severity {}
one sig Info    extends Severity {}

abstract sig DiagnosticRule {}
one sig MissingIdentity         extends DiagnosticRule {}
one sig MissingInverse          extends DiagnosticRule {}
one sig ClosureViolation        extends DiagnosticRule {}
one sig MapSignatureViolation   extends DiagnosticRule {}
one sig MissingLawTest          extends DiagnosticRule {}
one sig FailingLawTest          extends DiagnosticRule {}
one sig PropagationExceeded     extends DiagnosticRule {}
one sig AssociativityConfidence extends DiagnosticRule {}

sig Diagnostic {
  severity: one Severity,
  declaration: one AlgebraicDeclaration,
  rule: one DiagnosticRule
}

fact MissingIdentityIsError {
  all d: Diagnostic | d.rule = MissingIdentity implies d.severity = Error
}

fact MissingInverseIsError {
  all d: Diagnostic | d.rule = MissingInverse implies d.severity = Error
}

fact ClosureViolationIsError {
  all d: Diagnostic | d.rule = ClosureViolation implies d.severity = Error
}

fact MissingLawTestIsWarning {
  all d: Diagnostic | d.rule = MissingLawTest implies d.severity = Warning
}

fact FailingLawTestIsError {
  all d: Diagnostic | d.rule = FailingLawTest implies d.severity = Error
}

fact PropagationExceededIsWarning {
  all d: Diagnostic | d.rule = PropagationExceeded implies d.severity = Warning
}

fact AssociativityConfidenceIsInfo {
  all d: Diagnostic | d.rule = AssociativityConfidence implies d.severity = Info
}

-------------------------------------------------------------------------------
-- Context Propagation Degree (Axis 4)
-------------------------------------------------------------------------------

abstract sig PropagationSize {}
one sig Finite   extends PropagationSize {}
one sig Unbounded extends PropagationSize {}

sig ContextType {
  propagation: one PropagationSize,
  variantCount: lone Int
}

fact FiniteHasCount {
  all c: ContextType | c.propagation = Finite implies some c.variantCount
}

fact UnboundedHasNoCount {
  all c: ContextType | c.propagation = Unbounded implies no c.variantCount
}

fact CountIsPositive {
  all c: ContextType | some c.variantCount implies c.variantCount > 0
}

-------------------------------------------------------------------------------
-- Template Configuration
-------------------------------------------------------------------------------

abstract sig Preset {}
one sig DDD       extends Preset {}
one sig Hexagonal extends Preset {}
one sig Clean     extends Preset {}

sig PathPattern {}

sig LayerExpectation {
  pathPattern: one PathPattern,
  expectedStructures: set AlgebraicStructure,
  expectedHigherKinded: set HigherKindedStructure,
  maxPropagation: lone Int
}

fact ValidMaxPropagation {
  all l: LayerExpectation |
    some l.maxPropagation implies (l.maxPropagation > 0 or l.maxPropagation = -1)
}

-------------------------------------------------------------------------------
-- Compliance Gap (Axis 2)
-------------------------------------------------------------------------------

sig ComplianceReport {
  declaration: one AlgebraicDeclaration,
  totalLaws: one Int,
  passingLaws: one Int
}

fact ValidComplianceCounts {
  all r: ComplianceReport |
    r.passingLaws >= 0 and r.passingLaws <= r.totalLaws and r.totalLaws > 0
}

-------------------------------------------------------------------------------
-- Assertions
-------------------------------------------------------------------------------

assert MonoidIntegrity {
  all d: AlgebraicDeclaration |
    (no i: IgnoreAnnotation | i.declaration = d) and d.targetStructure.rank >= 2
    implies some d.identityName
}

assert GroupIntegrity {
  all d: AlgebraicDeclaration |
    (no i: IgnoreAnnotation | i.declaration = d) and d.targetStructure.rank >= 3
    implies some d.inverseName
}

check MonoidIntegrity for 5
check GroupIntegrity for 5
