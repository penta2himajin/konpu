use super::PropagationSize;

/// Invariant: FiniteHasCount
/// Invariant: UnboundedHasNoCount
/// Invariant: CountIsPositive
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContextType {
    pub propagation: PropagationSize,
    pub variantCount: Option<i64>,
}
