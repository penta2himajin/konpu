#[konpu::semigroup(op = "op")]
pub struct WithLaw;
impl WithLaw {
    pub fn op(self, _other: Self) -> Self { Self }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[konpu::law(associativity)]
    #[test]
    fn assoc_law() {}
}