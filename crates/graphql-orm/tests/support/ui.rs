use std::{
    fs::{File, OpenOptions},
    ops::Deref,
    path::PathBuf,
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

pub fn lock_path() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    target.join("graphql-orm-ui.lock")
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
