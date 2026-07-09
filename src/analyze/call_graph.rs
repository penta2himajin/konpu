//! Konpu's analyze facade — re-exports the trait/struct from konpu-cg when the
//! `call-graph` feature is enabled, otherwise exposes a local shim so the
//! `analyze_full_with_cg` API exists in both build modes.

#[cfg(feature = "call-graph")]
pub use konpu_cg::{CallGraphProvider, CallTarget};

#[cfg(not(feature = "call-graph"))]
pub use shim::{CallGraphProvider, CallTarget};

#[cfg(not(feature = "call-graph"))]
mod shim {
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct CallTarget {
        pub target_path: PathBuf,
        pub target_line: usize,
        pub target_name: String,
    }

    pub trait CallGraphProvider {
        fn resolve_outgoing_calls(
            &self,
            _file_path: &Path,
            _line: usize,
            _column: usize,
        ) -> Vec<CallTarget> {
            Vec::new()
        }
    }
}
