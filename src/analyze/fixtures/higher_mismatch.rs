#[konpu::monoid(op = "op", identity = "empty", higher = "applicative")]
pub struct HigherMismatch;
impl HigherMismatch {
    pub fn op(self, _o: Self) -> Self { Self }
    pub fn empty() -> Self { Self }
}