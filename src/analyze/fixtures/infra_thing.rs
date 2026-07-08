// Fixture for boundary tests. This file is the `to` layer; it imports the
// `from` layer via a path string that contains the `from` key ("src/domain")
// so the boundary checker can match it.
use crate::src::domain::thing::DomainMarker as _;
#[konpu::monoid(op = "op", identity = "empty")]
pub struct InfraThing;
impl InfraThing {
    pub fn op(self, _o: Self) -> Self { Self }
    pub fn empty() -> Self { Self }
    pub fn from_domain(_d: DomainRef) -> Self { Self }
}

pub struct DomainRef;
impl DomainRef {
    pub fn from_src_domain() -> Self { Self }
}