use std::{
    fs::{File, OpenOptions},
    ops::Deref,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

// trybuild's heartbeat lock can be stolen from a live but descheduled process.
// Retain an OS lock through TestCases::drop, which actually runs the probes.
pub struct UiTests {
    cases: trybuild::TestCases,
    _lock: File,
}

impl Deref for UiTests {
    type Target = trybuild::TestCases;
    fn deref(&self) -> &Self::Target {
        &self.cases
    }
}

pub fn lock_path() -> &'static Path {
    #[derive(serde::Deserialize)]
    struct CargoMetadata {
        target_directory: PathBuf,
    }

    static LOCK_PATH: OnceLock<PathBuf> = OnceLock::new();
    LOCK_PATH.get_or_init(|| {
        // Resolve the target the same way as trybuild, including Cargo config.
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .args(["metadata", "--no-deps", "--format-version=1"])
            .output()
            .expect("run Cargo metadata for the UI target directory");
        assert!(
            output.status.success(),
            "Cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let metadata: CargoMetadata =
            serde_json::from_slice(&output.stdout).expect("Cargo metadata for the UI target");
        metadata.target_directory.join("graphql-orm-ui.lock")
    })
}

pub fn lock_project() -> File {
    let path = lock_path();
    std::fs::create_dir_all(path.parent().unwrap()).expect("UI target directory");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .expect("UI coordination lock");
    lock.lock().expect("exclusive OS UI lock");
    lock
}

pub fn test_cases() -> UiTests {
    let lock = lock_project();
    UiTests {
        cases: trybuild::TestCases::new(),
        _lock: lock,
    }
}
