use super::AlgebraicStructure;
use super::HigherKindedStructure;
use super::OperationName;

/// Invariant: MonoidRequiresIdentity
/// Invariant: GroupRequiresInverse
/// Invariant: IdentityDistinctFromOp
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AlgebraicDeclaration {
    pub targetStructure: AlgebraicStructure,
    pub higherKinded: Option<HigherKindedStructure>,
    pub operationName: OperationName,
    pub identityName: Option<OperationName>,
    pub inverseName: Option<OperationName>,
}
