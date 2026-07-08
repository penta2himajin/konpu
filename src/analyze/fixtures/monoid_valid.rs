#[konpu::monoid(op = "concat", identity = "empty")]
pub struct ValidMonoid;
impl ValidMonoid {
    pub fn concat(self, _other: Self) -> Self { Self }
    pub fn empty() -> Self { Self }
}