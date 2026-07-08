#[konpu::monoid(op = "concat", identity = "empty")]
pub struct BadMonoid;
impl BadMonoid {
    pub fn concat(self, _other: Self) -> Self { Self }
}