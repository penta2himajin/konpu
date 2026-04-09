#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TestStatus {
    Pass,
    Fail,
    Missing,
}
