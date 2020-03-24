# Logging

Kompact uses the [slog](https://crates.io/crates/slog) crate to provide system wide logging facilities.
The basic macros for this, `slog::{crit, debug, error, info, o, trace, warn}` are re-exported in the prelude for convenience. Logging works out of the box with a default asynchronous console logger implementation that roughly corresponds to the following setup code:

```rust,edition2018,no_run,noplaypen
let decorator = slog_term::TermDecorator::new().stdout().build();
let drain = slog_term::FullFormat::new(decorator).build().fuse();
let drain = slog_async::Async::new(drain).chan_size(1024).build().fuse();
let logger = slog::Logger::root_typed(Arc::new(drain));
```

The actual logging levels are controlled via build features. The default features correspond to `max_level_trace` and `release_max_level_info`, that is in debug builds all levels are shown, while in the release profile only `info` and more severe message are shown. Alternatively, Kompact provides a slightly less verbose feature variant called `silent_logging`, which is equivalent to `max_level_info` and `release_max_level_error`.

This is exemplified in the following very simple code example:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/logging.rs:4:15}}
```

Try to run it with a few different build settings and see what you get.

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary with:
> ```bash
> cargo run --release --bin logging
> ```

## Custom Logger

Sometimes the default logging configuration is not sufficient for a particular application. For example, you might need a larger queue size in the `Async` drain, or you may want to write to a file instead of the terminal.

In the following example we replace the default terminal logger with a file logger, logging to `/tmp/myloggingfile` instead. We also increase the queue size in the `Async` drain to 2048, so that it fits the 2048 logging events we are sending it short successing later. In order to replace the default logger, we use the `KompactConfig::logger(...)` function.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/logging_custom.rs:6:49}}
```

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary and then show the logging file with:
> ```bash
> cargo run --release --bin logging_custom
> cat /tmp/myloggingfile
> ```
