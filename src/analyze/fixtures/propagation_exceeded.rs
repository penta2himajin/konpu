#[konpu::semigroup(op = "op")]
pub struct BadProp {
    items: Vec<i32>,
}
impl BadProp {
    pub fn op(self, _other: Self) -> Self { Self { items: Vec::new() } }
}