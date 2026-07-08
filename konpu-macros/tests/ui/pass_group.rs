#[konpu::group(op = "op", identity = "empty", inverse = "inv")]
struct ValidGroup;

impl ValidGroup {
    fn op(self, _other: Self) -> Self {
        self
    }
    fn empty() -> Self {
        Self
    }
    fn inv(self) -> Self {
        self
    }
}

fn main() {}