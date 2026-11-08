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
