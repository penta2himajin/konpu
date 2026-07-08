#[konpu::monoid(op = "op", identity = "empty")]
pub struct Partial;
impl Partial {
    pub fn op(self, _other: Self) -> Self { Self }
    pub fn empty() -> Self { Self }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[konpu::law(left_identity)]
    #[test]
    fn left() {}
}