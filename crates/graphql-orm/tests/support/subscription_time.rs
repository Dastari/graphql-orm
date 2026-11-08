//! Manually advanced virtual time for subscription tests with external DB workers.
//!
//! A runnable task prevents Tokio's idle auto-advance from turning an unrelated
//! database worker wakeup into a timer tick.
pub struct ManualTime(tokio::task::JoinHandle<()>);
impl ManualTime {
    pub fn pause() -> Self {
        tokio::time::pause();
        Self(tokio::spawn(async {
            loop {
                tokio::task::yield_now().await;
            }
        }))
    }
}
impl Drop for ManualTime {
    fn drop(&mut self) {
        self.0.abort();
        tokio::time::resume();
    }
}
