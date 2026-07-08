#[konpu::magma(op = "op")]
pub struct WeakDomain;
impl WeakDomain {
    pub fn op(self, _other: Self) -> Self { Self }
}