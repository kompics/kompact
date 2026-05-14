use std::{
    thread,
    time::{Duration, Instant},
};

pub use kompact::test_support::build_test_kompact_system as build_test_system;

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
