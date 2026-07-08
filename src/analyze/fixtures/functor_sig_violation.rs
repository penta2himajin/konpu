#[konpu::semigroup(op = "op", higher = "functor")]
pub struct BadFunctor;

impl BadFunctor {
    pub fn op(self, _other: Self) -> Self { Self }
    pub fn map(&mut self, _f: i64) {}
}