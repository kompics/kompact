#![allow(clippy::unused_unit)]
use kompact::prelude::*;
use std::{fs::OpenOptions, sync::Arc, time::Duration};

const FILE_NAME: &str = "/tmp/myloggingfile";

// ANCHOR: main
pub fn main() {
    let mut conf = KompactConfig::default();
    let logger = {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(FILE_NAME)
            .expect("logging file");

        // create logger
        let decorator = slog_term::PlainSyncDecorator::new(file);
        let drain = slog_term::FullFormat::new(decorator).build().fuse();
        let drain = slog_async::Async::new(drain).chan_size(2048).build().fuse();
        slog::Logger::root_typed(
            Arc::new(drain),
            o!(
            "location" => slog::PushFnValue(|r: &slog::Record<'_>, ser: slog::PushFnValueSerializer<'_>| {
                ser.emit(format_args!("{}:{}", r.file(), r.line()))
            })),
        )
    };
    conf.logger(logger);
    let system = conf.build().expect("system");
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

    for i in 0..2048 {
        info!(system.logger(), "Logging number {}.", i);
    }

    // remember that logging is asynchronous and won't happen if the system is shut down already
    std::thread::sleep(Duration::from_millis(1000));
    system.shutdown().expect("shutdown");
}
// ANCHOR_END: main

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logging() {
        main();
        std::fs::remove_file(FILE_NAME).expect("remove log file");
    }
}
