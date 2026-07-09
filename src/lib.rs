// Konpu — Algebraic complexity linter
#[allow(non_snake_case, clippy::nonminimal_bool, clippy::collapsible_if)]
pub mod analyze;
#[allow(non_snake_case, clippy::nonminimal_bool, clippy::collapsible_if)]
pub mod domain;

pub use konpu_macros::{group, ignore, law, magma, monoid, semigroup};

pub use analyze::call_graph::{CallGraphProvider, CallTarget};
