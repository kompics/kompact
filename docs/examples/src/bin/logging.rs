#![allow(clippy::unused_unit)]
use kompact::prelude::*;
use std::time::Duration;

// ANCHOR: main
pub fn main() {
    let system = KompactConfig::default().build().expect("system");
    trace!(
        system.logger(),
        "You will only see this in debug builds with default features"
    );
    debug!(
        system.logger(),
        "You will only see this in debug builds with default features"
    );
    info!(system.logger(), "You will only see this in debug builds with silent_logging or in release builds with default features");
    warn!(system.logger(), "You will only see this in debug builds with silent_logging or in release builds with default features");
    error!(system.logger(), "You will always see this");

    // remember that logging is asynchronous and won't happen if the system is shut down already
    std::thread::sleep(Duration::from_millis(100));
    system.shutdown().expect("shutdown");
}
// ANCHOR_END: main

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logging() {
        main();
    }
}
