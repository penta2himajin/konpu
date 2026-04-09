#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AlgebraicStructure {
    Magma,
    Semigroup,
    Monoid,
    Group,
}

impl AlgebraicStructure {
    pub fn rank(&self) -> i64 {
        match self {
            Self::Magma => 0,
            Self::Semigroup => 1,
            Self::Monoid => 2,
            Self::Group => 3,
        }
    }
}
