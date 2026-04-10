#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HigherKindedStructure {
    Functor,
    Applicative,
    MonadS,
}

impl HigherKindedStructure {
    pub const fn hkRank(&self) -> i64 {
        match self {
            Self::Functor => 1,
            Self::Applicative => 2,
            Self::MonadS => 3,
        }
    }
}
