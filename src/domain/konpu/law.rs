#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Law {
    Associativity,
    LeftIdentity,
    RightIdentity,
    InverseLeft,
    InverseRight,
    FunctorIdentity,
    FunctorComposition,
    ApplicativeIdentity,
    ApplicativeComposition,
    MonadLeftIdentity,
    MonadRightIdentity,
    MonadAssociativity,
}
