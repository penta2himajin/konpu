#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IgnoreReason {
    Intentional,
    Debt,
    Infeasible,
}
