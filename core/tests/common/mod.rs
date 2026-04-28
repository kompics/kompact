use std::{
    thread,
    time::{Duration, Instant},
};

/// Wait until `predicate` returns true or panic after `timeout`.
pub fn eventually(name: &str, timeout: Duration, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {name}");
}
