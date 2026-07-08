#[konpu::monoid(op = "concat", identity = "empty")]
struct ValidMonoid;

impl ValidMonoid {
    fn concat(self, other: Self) -> Self {
        let _ = other;
        self
    }
    fn empty() -> Self {
        Self
    }
}

fn main() {}