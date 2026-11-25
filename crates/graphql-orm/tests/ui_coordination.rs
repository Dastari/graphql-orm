#[allow(dead_code)]
#[path = "support/ui.rs"]
mod ui;

#[test]
fn live_ui_lock_cannot_be_stolen_when_its_timestamp_is_stale() {
    let owner = ui::lock_project();
    // Reproduce the stale-heartbeat condition without a sleep or CPU load.
    owner
        .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
        .unwrap();
    let contender = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(ui::lock_path())
        .unwrap();
    assert!(matches!(
        contender.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(owner);
    contender
        .try_lock()
        .expect("released lock becomes available");
}

#[test]
fn ui_lock_follows_cargos_configured_target_directory() {
    // Use a child to change Cargo configuration without mutating the test
    // process's environment while other tests may be accessing it.
    const CONFIGURED_TARGET: &str = "GRAPHQL_ORM_UI_TEST_CONFIGURED_TARGET";
    if let Some(target) = std::env::var_os(CONFIGURED_TARGET) {
        assert_eq!(
            ui::lock_path(),
            std::path::PathBuf::from(target).join("graphql-orm-ui.lock")
        );
        return;
    }

    let target = ui::lock_path()
        .parent()
        .unwrap()
        .join("configured-ui-target");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ui_lock_follows_cargos_configured_target_directory",
        ])
        .env_remove("CARGO_TARGET_DIR")
        .env("CARGO_BUILD_TARGET_DIR", &target)
        .env(CONFIGURED_TARGET, &target)
        .output()
        .expect("run UI coordination regression with a Cargo target override");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
