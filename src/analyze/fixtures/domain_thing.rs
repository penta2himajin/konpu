use konpu::analyze::template;
#[konpu::monoid(op = "op", identity = "empty")]
pub struct DomainThing;
impl DomainThing {
    pub fn op(self, _o: Self) -> Self { Self }
    pub fn empty() -> Self { Self }
}