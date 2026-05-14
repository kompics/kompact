//! Test support utilities for Kompact systems.
//!
//! These helpers route logging through libtest's captured output path, keeping
//! successful test runs quiet while preserving logs for failing tests and
//! `--nocapture` runs.

use crate::{
    KompactLogger,
    runtime::{self, KompactConfig, KompactSystem},
    utils::BlockingFutureExt,
};
use regex::Regex;
use slog::{Drain, Logger, PushFnValue, o};
use std::{
    io::{self, Write},
    sync::{Arc, OnceLock},
};

/// Installs a captured logger for the `log` facade.
///
/// Calling this function multiple times is harmless. It panics if another
/// `log` facade logger has already been installed in this process.
pub fn init_test_logger() {
    static INIT: OnceLock<()> = OnceLock::new();

    INIT.get_or_init(|| {
        log::set_logger(&CAPTURED_LOGGER).expect("install captured test logger");
        log::set_max_level(log::LevelFilter::Trace);
        runtime::set_default_logger(build_captured_kompact_logger());
    });
}

/// Creates a Kompact logger whose output is captured by libtest.
///
/// This also installs the matching `log` facade logger once for the test
/// process.
pub fn captured_kompact_logger() -> KompactLogger {
    init_test_logger();
    build_captured_kompact_logger()
}

fn build_captured_kompact_logger() -> KompactLogger {
    let decorator = slog_term::PlainSyncDecorator::new(CapturedOutput::default());
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).chan_size(1024).build().fuse();
    Logger::root_typed(
        Arc::new(drain),
        o!(
            "location" => PushFnValue(|record: &slog::Record<'_>, serializer| {
                serializer.emit(format_args!("{}:{}", record.file(), record.line()))
            })
        ),
    )
}

/// Configures a Kompact system to use the captured test logger.
///
/// Returns the same config for chaining.
pub fn configure_test_logger(config: &mut KompactConfig) -> &mut KompactConfig {
    config.logger(captured_kompact_logger())
}

/// Builds a default Kompact config that uses the captured test logger.
#[must_use]
pub fn test_kompact_config() -> KompactConfig {
    let mut config = KompactConfig::default();
    configure_test_logger(&mut config);
    config
}

/// Builds a default Kompact system that uses the captured test logger.
///
/// # Panics
///
/// Panics if the system cannot be built.
pub fn build_test_kompact_system() -> KompactSystem {
    test_kompact_config().build().wait().expect("system")
}

/// Asserts that an error display string matches `expected`.
///
/// The expected string may include `{LOC}` placeholders. Each placeholder is
/// matched against `(<location_file>:<line>:<column>)`, with line and column
/// left flexible for stable location-aware error tests.
///
/// # Panics
///
/// Panics if the expected pattern does not compile or `actual` does not match.
pub fn assert_display_matches(actual: &str, expected: &str, location_file: &str) {
    let location = format!(r"\({}:\d+:\d+\)", regex::escape(location_file));
    let pattern = format!(
        "^{}$",
        regex::escape(expected).replace(r"\{LOC\}", &location)
    );
    let regex = Regex::new(&pattern).expect("expected display regex should compile");
    assert!(
        regex.is_match(actual),
        "expected display to match:\n{expected}\n\nactual:\n{actual}\n\nregex:\n{pattern}",
    );
}

static CAPTURED_LOGGER: CapturedLogLogger = CapturedLogLogger;

struct CapturedLogLogger;

impl log::Log for CapturedLogLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Trace
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let location = match (record.file(), record.line()) {
            (Some(file), Some(line)) => format!("{file}:{line}"),
            (Some(file), None) => file.to_string(),
            _ => record.target().to_string(),
        };
        println!("[{} {}] {}", record.level(), location, record.args());
    }

    fn flush(&self) {
        let _ = io::stdout().flush();
    }
}

/// Writer that routes slog output through libtest's captured stdout path.
#[derive(Default)]
struct CapturedOutput {
    pending: Vec<u8>,
}

impl CapturedOutput {
    fn emit_complete_lines(&mut self) {
        while let Some(newline_index) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=newline_index).collect();
            Self::emit_bytes(&line);
        }
    }

    fn emit_bytes(bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        print!("{text}");
        let _ = io::stdout().flush();
    }
}

impl Write for CapturedOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        self.emit_complete_lines();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let remaining = std::mem::take(&mut self.pending);
            Self::emit_bytes(&remaining);
        }
        Ok(())
    }
}

impl Drop for CapturedOutput {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::CapturedOutput;
    use std::io::Write;

    #[test]
    fn captured_output_buffers_partial_lines() {
        let mut output = CapturedOutput::default();

        output.write_all(b"partial").expect("write partial line");
        assert_eq!(output.pending, b"partial");

        output.write_all(b" line\nnext").expect("write full line");
        assert_eq!(output.pending, b"next");

        output.flush().expect("flush pending line");
        assert!(output.pending.is_empty());
    }
}
