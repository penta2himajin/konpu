// oxidtr `--konpu` output shape for an Alloy `one sig` monoid identity:
// the identity is a singleton unit struct + INSTANCE const, and the op is a
// free function (receiver-less), not an impl method. konpu must resolve
// `identity = "Zero"` to the singleton type `Zero`.
#[konpu::monoid(op = "add", identity = "Zero")]
pub struct Money {
    pub amount: i64,
}

pub struct Zero;
pub const ZERO_INSTANCE: Zero = Zero;

pub fn add(a: &Money, b: &Money) -> Money {
    Money { amount: a.amount + b.amount }
}
