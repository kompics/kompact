use std::{error, fmt};

use snafu::IntoError;

/// Internal display helper for optional SNAFU backtraces.
///
/// This is public so extension crates can reuse the same diagnostic formatting,
/// but it is not part of Kompact's supported public API.
#[doc(hidden)]
pub struct BacktraceSuffix<'a>(pub Option<&'a snafu::Backtrace>);

impl fmt::Display for BacktraceSuffix<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(backtrace) = self.0 {
            write!(f, "\nBacktrace:\n{backtrace}")?;
        }
        Ok(())
    }
}

/// Internal display helper for a single source cause.
#[doc(hidden)]
pub struct SourceCause<'a>(
    pub &'a dyn fmt::Display,
    pub Option<&'a snafu::Location>,
    pub Option<&'a snafu::Backtrace>,
);

impl fmt::Display for SourceCause<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\n  Caused by: {}", self.0)?;
        if let Some(location) = self.1 {
            write!(f, " ({location})")?;
        }
        write!(f, ".{}", BacktraceSuffix(self.2))
    }
}

/// Internal display helper for source chains that already include causes.
#[doc(hidden)]
pub struct NestedCause<'a>(pub &'a dyn fmt::Display);

impl fmt::Display for NestedCause<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rendered = self.0.to_string();
        let mut lines = rendered.lines();
        if let Some(first) = lines.next() {
            write!(f, "\n  Caused by: {first}")?;
        }
        for line in lines {
            write!(f, "\n{line}")?;
        }
        Ok(())
    }
}

/// Error returned when a scheduler fails to shut down.
#[derive(Debug, snafu::Snafu)]
pub struct SchedulerShutdownError(SchedulerShutdownErrorInner);

#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub(crate)))]
#[snafu(context(name(SchedulerShutdownSnafu)))]
#[snafu(display(
    "scheduler shutdown failed ({location}).{}",
    SourceCause(source.as_ref(), Some(location), backtrace.as_ref())
))]
pub(crate) struct SchedulerShutdownErrorInner {
    #[snafu(source(from(String, Into::into)))]
    source: Box<dyn error::Error + Send + Sync + 'static>,
    #[snafu(implicit)]
    location: snafu::Location,
    backtrace: Option<snafu::Backtrace>,
}

impl SchedulerShutdownError {
    /// Construct a scheduler shutdown error from a message.
    #[track_caller]
    pub fn message(message: impl Into<String>) -> Self {
        SchedulerShutdownSnafu.into_error(message.into()).into()
    }
}

impl From<String> for SchedulerShutdownError {
    fn from(message: String) -> Self {
        Self::message(message)
    }
}

impl From<&str> for SchedulerShutdownError {
    fn from(message: &str) -> Self {
        Self::message(message)
    }
}

/// Error returned when a timer fails to shut down.
#[derive(Debug, snafu::Snafu)]
pub struct TimerShutdownError(TimerShutdownErrorInner);

#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub(crate)))]
#[snafu(context(name(TimerShutdownSnafu)))]
#[snafu(display(
    "timer shutdown failed ({location}).{}",
    SourceCause(source.as_ref(), Some(location), backtrace.as_ref())
))]
pub(crate) struct TimerShutdownErrorInner {
    #[snafu(source(from(String, Into::into)))]
    source: Box<dyn error::Error + Send + Sync + 'static>,
    #[snafu(implicit)]
    location: snafu::Location,
    backtrace: Option<snafu::Backtrace>,
}

impl TimerShutdownError {
    /// Construct a timer shutdown error from a message.
    #[track_caller]
    pub fn message(message: impl Into<String>) -> Self {
        TimerShutdownSnafu.into_error(message.into()).into()
    }

    /// Construct a timer shutdown error from a debug-only third-party error.
    #[track_caller]
    pub fn from_debug(error: impl fmt::Debug) -> Self {
        Self::message(format!("{:?}", error))
    }
}

impl From<String> for TimerShutdownError {
    fn from(message: String) -> Self {
        Self::message(message)
    }
}

impl From<&str> for TimerShutdownError {
    fn from(message: &str) -> Self {
        Self::message(message)
    }
}

/// Error returned when system components fail to shut down.
#[derive(Debug, snafu::Snafu)]
pub struct SystemComponentsShutdownError(SystemComponentsShutdownErrorKind);

#[derive(Debug, snafu::Snafu)]
#[snafu(module(system_components_shutdown_error), visibility(pub(crate)))]
pub(crate) enum SystemComponentsShutdownErrorKind {
    #[snafu(display(
        "system components shutdown failed ({location}).{}",
        SourceCause(source.as_ref(), Some(location), backtrace.as_ref())
    ))]
    Message {
        #[snafu(source(from(String, Into::into)))]
        source: Box<dyn error::Error + Send + Sync + 'static>,
        #[snafu(implicit)]
        location: snafu::Location,
        backtrace: Option<snafu::Backtrace>,
    },
    #[snafu(display(
        "system components shutdown failed ({location}).{}{}",
        SourceCause(message, Some(location), None),
        SourceCause(source.as_ref(), None, backtrace.as_ref())
    ))]
    Source {
        message: String,
        source: Box<dyn error::Error + Send + Sync + 'static>,
        #[snafu(implicit)]
        location: snafu::Location,
        backtrace: Option<snafu::Backtrace>,
    },
}

impl SystemComponentsShutdownError {
    /// Construct a system-components shutdown error from a message.
    #[track_caller]
    pub fn message(message: impl Into<String>) -> Self {
        system_components_shutdown_error::MessageSnafu
            .into_error(message.into())
            .into()
    }

    /// Construct a system-components shutdown error with a source.
    #[track_caller]
    pub fn source(
        message: impl Into<String>,
        source: impl error::Error + Send + Sync + 'static,
    ) -> Self {
        let source: Box<dyn error::Error + Send + Sync + 'static> = Box::new(source);
        system_components_shutdown_error::SourceSnafu {
            message: message.into(),
        }
        .into_error(source)
        .into()
    }
}

impl From<String> for SystemComponentsShutdownError {
    fn from(message: String) -> Self {
        Self::message(message)
    }
}

impl From<&str> for SystemComponentsShutdownError {
    fn from(message: &str) -> Self {
        Self::message(message)
    }
}

/// Error returned when a Kompact system fails to shut down.
#[derive(Debug, snafu::Snafu)]
pub struct ShutdownError(Box<ShutdownErrorKind>);

#[derive(Debug, snafu::Snafu)]
#[snafu(module(shutdown_error), visibility(pub(crate)))]
pub(crate) enum ShutdownErrorKind {
    #[snafu(display(
        "system shutdown failed while stopping system components ({location}).{}",
        NestedCause(source)
    ))]
    SystemComponents {
        #[snafu(backtrace)]
        source: SystemComponentsShutdownError,
        #[snafu(implicit)]
        location: snafu::Location,
    },
    #[snafu(display(
        "system shutdown failed while stopping timer ({location}).{}",
        NestedCause(source)
    ))]
    Timer {
        #[snafu(backtrace)]
        source: TimerShutdownError,
        #[snafu(implicit)]
        location: snafu::Location,
    },
    #[snafu(display(
        "system shutdown failed while stopping scheduler ({location}).{}",
        NestedCause(source)
    ))]
    Scheduler {
        #[snafu(backtrace)]
        source: SchedulerShutdownError,
        #[snafu(implicit)]
        location: snafu::Location,
    },
}

impl From<SystemComponentsShutdownError> for ShutdownError {
    #[track_caller]
    fn from(source: SystemComponentsShutdownError) -> Self {
        shutdown_error::SystemComponentsSnafu
            .into_error(source)
            .into()
    }
}

impl From<TimerShutdownError> for ShutdownError {
    #[track_caller]
    fn from(source: TimerShutdownError) -> Self {
        shutdown_error::TimerSnafu.into_error(source).into()
    }
}

impl From<SchedulerShutdownError> for ShutdownError {
    #[track_caller]
    fn from(source: SchedulerShutdownError) -> Self {
        shutdown_error::SchedulerSnafu.into_error(source).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::assert_display_matches;
    use std::error::Error;

    #[test]
    fn opaque_shutdown_error_preserves_source_chain() {
        let scheduler_error: SchedulerShutdownError = SchedulerShutdownSnafu
            .into_error("pool did not stop".to_string())
            .into();
        let error: ShutdownError = shutdown_error::SchedulerSnafu
            .into_error(scheduler_error)
            .into();

        assert_display_matches(
            &error.to_string(),
            "\
system shutdown failed while stopping scheduler {LOC}.
  Caused by: scheduler shutdown failed {LOC}.
  Caused by: pool did not stop {LOC}.",
            file!(),
        );
        let source = error.source().expect("shutdown error should expose source");
        assert_display_matches(
            &source.to_string(),
            "\
scheduler shutdown failed {LOC}.
  Caused by: pool did not stop {LOC}.",
            file!(),
        );
    }

    #[test]
    fn shutdown_display_includes_context_source_and_locations() {
        let source = std::io::Error::other("promise channel disconnected");
        let components_error: SystemComponentsShutdownError =
            system_components_shutdown_error::SourceSnafu {
                message: "supervisor shutdown promise was dropped".to_string(),
            }
            .into_error(Box::new(source))
            .into();
        let error: ShutdownError = shutdown_error::SystemComponentsSnafu
            .into_error(components_error)
            .into();

        assert_display_matches(
            &error.to_string(),
            "\
system shutdown failed while stopping system components {LOC}.
  Caused by: system components shutdown failed {LOC}.
  Caused by: supervisor shutdown promise was dropped {LOC}.
  Caused by: promise channel disconnected.",
            file!(),
        );
    }

    #[test]
    fn shutdown_error_exposes_printable_source_chain() {
        let source = std::io::Error::other("promise channel disconnected");
        let components_error: SystemComponentsShutdownError =
            system_components_shutdown_error::SourceSnafu {
                message: "supervisor shutdown promise was dropped".to_string(),
            }
            .into_error(Box::new(source))
            .into();
        let error: ShutdownError = shutdown_error::SystemComponentsSnafu
            .into_error(components_error)
            .into();

        let mut messages = Vec::new();
        let mut current: Option<&(dyn Error + 'static)> = Some(&error);
        while let Some(error) = current {
            messages.push(error.to_string());
            current = error.source();
        }

        assert_eq!(messages.len(), 3);
        assert_display_matches(
            &messages[0],
            "\
system shutdown failed while stopping system components {LOC}.
  Caused by: system components shutdown failed {LOC}.
  Caused by: supervisor shutdown promise was dropped {LOC}.
  Caused by: promise channel disconnected.",
            file!(),
        );
        assert_display_matches(
            &messages[1],
            "\
system components shutdown failed {LOC}.
  Caused by: supervisor shutdown promise was dropped {LOC}.
  Caused by: promise channel disconnected.",
            file!(),
        );
        assert_eq!("promise channel disconnected", messages[2]);
    }

    #[test]
    fn backtrace_suffix_prints_backtrace_when_available() {
        let backtrace = snafu::Backtrace::force_capture();
        let rendered = BacktraceSuffix(Some(&backtrace)).to_string();

        assert!(rendered.contains("Backtrace:"));
    }
}
