use konpu::analyze::scaffold;

#[konpu::semigroup(op = "op")]
pub struct WithIgnore;
impl WithIgnore {
    pub fn op(self, _other: Self) -> Self { Self }
    #[konpu::ignore(reason = "intentional", note = "skipped for now")]
    pub fn ignored() {}
}