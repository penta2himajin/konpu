#[konpu::semigroup(op = "op")]
struct ValidSemigroup;

impl ValidSemigroup {
    fn op(self, _other: Self) -> Self {
        self
    }
}

fn main() {}