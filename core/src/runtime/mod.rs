use super::*;

use std::{
    error,
    fmt,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
        Once,
    },
};

use crate::config::ConfigError;

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
        // Overwriting the current value will implicitly drop it.
        DEFAULT_ROOT_LOGGER = None;
    }
}

type SchedulerBuilder = dyn Fn(usize) -> Box<dyn Scheduler>;

type ScBuilder = dyn Fn(&KompactSystem, KPromise<()>, KPromise<()>) -> Box<dyn SystemComponents>;

type TimerBuilder = dyn Fn() -> Box<dyn TimerComponent>;

/// A Kompact system error
#[derive(Debug)]
pub enum KompactError {
    /// A mutex in the system has been poisoned
    Poisoned,
    /// An error occurred loading the HOCON config
    ConfigLoadingError(hocon::Error),
    /// An error occurred reading values from the loaded config
    ConfigError(ConfigError),
    /// Something else occurred
    Other(Box<dyn error::Error>),
}

impl KompactError {
    /// Wrap an arbitrary [Error](std::error::Error) into a `KompactError`
    pub fn from_other<E>(e: E) -> Self
    where
        E: error::Error + 'static,
    {
        KompactError::Other(Box::new(e))
    }
}

impl PartialEq for KompactError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (KompactError::Poisoned, KompactError::Poisoned) => true,
            (KompactError::ConfigLoadingError(she), KompactError::ConfigLoadingError(ohe)) => {
                she == ohe
            }
            (KompactError::ConfigError(se), KompactError::ConfigError(oe)) => se == oe,
            _ => false,
        }
    }
}

impl From<hocon::Error> for KompactError {
    fn from(e: hocon::Error) -> Self {
        KompactError::ConfigLoadingError(e)
    }
}

impl From<ConfigError> for KompactError {
    fn from(e: ConfigError) -> Self {
        KompactError::ConfigError(e)
    }
}

impl fmt::Display for KompactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KompactError::Poisoned => write!(f, "A mutex in the KompactSystem has been poisoned"),
            KompactError::ConfigLoadingError(he) => {
                write!(f, "An issue occurred loading configuration: {}", he)
            }
            KompactError::ConfigError(e) => {
                write!(f, "An issue occurred reading configuration: {}", e)
            }
            KompactError::Other(o) => write!(f, "An unknown issue occurred: {}", o),
        }
    }
}

impl error::Error for KompactError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            KompactError::Poisoned => None,
            KompactError::ConfigLoadingError(ref e) => Some(e),
            KompactError::ConfigError(ref e) => Some(e),
            KompactError::Other(ref o) => Some(o.as_ref()),
        }
    }
}
