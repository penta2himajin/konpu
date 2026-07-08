#[konpu::group(op = "op", identity = "empty", inverse = "inv")]
pub struct BadGroup;
impl BadGroup {
    pub fn op(self, _other: Self) -> Self { Self }
    pub fn empty() -> Self { Self }
}