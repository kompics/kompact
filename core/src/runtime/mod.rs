use super::*;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
    Once,
};

mod config;
mod lifecycle;
mod scheduler;
mod system;

pub use config::*;
pub use scheduler::*;
pub use system::*;

static GLOBAL_RUNTIME_COUNT: AtomicUsize = AtomicUsize::new(0);

fn default_runtime_label() -> String {
    let runtime_count = GLOBAL_RUNTIME_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    format!("kompact-runtime-{}", runtime_count)
}

static mut DEFAULT_ROOT_LOGGER: Option<KompactLogger> = None;
static DEFAULT_ROOT_LOGGER_INIT: Once = Once::new();

fn default_logger() -> &'static KompactLogger {
    unsafe {
        DEFAULT_ROOT_LOGGER_INIT.call_once(|| {
            let decorator = slog_term::TermDecorator::new().stdout().build();
            let drain = slog_term::FullFormat::new(decorator).build().fuse();
            let drain = slog_async::Async::new(drain).chan_size(1024).build().fuse();
            DEFAULT_ROOT_LOGGER = Some(slog::Logger::root_typed(
                Arc::new(drain),
                o!(
                "location" => slog::PushFnValue(|r: &slog::Record<'_>, ser: slog::PushFnValueSerializer<'_>| {
                    ser.emit(format_args!("{}:{}", r.file(), r.line()))
                })
                        ),
            ));
        });
        match DEFAULT_ROOT_LOGGER {
            Some(ref l) => l,
            None => panic!("Can't re-initialise global logger after it has been dropped!"),
        }
    }
}

/// Removes the global default logger
///
/// This causes the remaining messages to be flushed to the output.
///
/// This can't be undone (as in, calling `default_logger()` afterwards again will panic),
/// so make sure you use this only right before exiting the programme.
pub fn drop_default_logger() {
    unsafe {
        drop(DEFAULT_ROOT_LOGGER.take());
    }
}

type SchedulerBuilder = dyn Fn(usize) -> Box<dyn Scheduler>;

type SCBuilder = dyn Fn(&KompactSystem, Promise<()>, Promise<()>) -> Box<dyn SystemComponents>;

type TimerBuilder = dyn Fn() -> Box<dyn TimerComponent>;

/// A Kompact system error
#[derive(Debug, PartialEq, Clone)]
pub enum KompactError {
    /// A mutex in the system has been poisoned
    Poisoned,
    /// An error occurred loading the HOCON config
    ConfigError(hocon::Error),
}

impl From<hocon::Error> for KompactError {
    fn from(e: hocon::Error) -> Self {
        KompactError::ConfigError(e)
    }
}
