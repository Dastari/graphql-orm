use std::path::{Path, PathBuf};

// Every consumer workspace shares dependency builds inside the caller's target
// boundary. Resolve relative overrides before invoking Cargo on another manifest.
pub fn target_dir() -> PathBuf {
    derive_target_dir(
        std::env::var_os("CARGO_TARGET_DIR")
            .as_deref()
            .map(Path::new),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("runtime crate must be inside the workspace"),
        &std::env::current_dir().expect("fixture runner working directory"),
    )
}

pub fn derive_target_dir(caller_target: Option<&Path>, workspace: &Path, cwd: &Path) -> PathBuf {
    match caller_target {
        Some(target) => cwd.join(target),
        None => workspace.join("target"),
    }
    .join("fixtures")
}
