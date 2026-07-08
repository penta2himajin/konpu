#[konpu::semigroup(op = "op")]
pub struct BadSemigroup;
impl BadSemigroup {
    pub fn op(&mut self, _other: &Self) {}
}