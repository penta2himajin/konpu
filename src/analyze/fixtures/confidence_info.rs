#[konpu::semigroup(op = "op")]
pub struct PureSemigroup;
impl PureSemigroup {
    pub fn op(self, _other: Self) -> Self { Self }
}