#[konpu::semigroup(op = "op")]
pub struct WithoutLaw;
impl WithoutLaw {
    pub fn op(self, _other: Self) -> Self { Self }
}