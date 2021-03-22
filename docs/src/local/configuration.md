# Configuration

Since it is often inconvenient to pass around a large number of parameters when setting up a component system, Kompact also offers a configuration system allowing parameters to be loaded from a file or provided as a large string at the top level, for example. This system is powered by the [Hocon](https://crates.io/crates/hocon) crate and uses its APIs with very little additional support. 

Configuration options must be set on the `KompactConfig` instance before the system is started and the resulting configuration remains immutable for the lifetime of the system. A configuration can be loaded from a file by passing a path to the file to the `load_config_file(...)` function. Alternatively, configuration values can be loaded directly from a string using `load_config_str(...)`.

Within each component the [Hocon](https://docs.rs/hocon/latest/hocon/enum.Hocon.html) configuration instance can be accessed via the context and individual keys via bracket notation, e.g. `self.ctx.config()["my-key"]`. The configuration can also be accessed outside a component via `KompactSystem::config()`.

In addition to component configuration, many parts of Kompact's runtime can also be configured via this mechanism. The complete set of available configuration keys and their effects is described in the modules below [kompact::config_keys](https://docs.rs/kompact/latest/kompact/config_keys/index.html).

## Example

We are going to reuse the `Buncher` from the [timers](timers.md) section and pass its two parameters, `batch_size` and `timeout`, via configuration instead of the constructor.

We'll start off by creating a configuration file `application.conf` in the working directory, so its easy to find later. Something like this:

```hocon
{{#rustdoc_include ../../examples/application.conf}}
```

We can then add this file to the `KompicsConfig` instance using the `load_config_file(...)` function: 

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_config.rs:config_file}}
```

To show off how multiple configuration sources can be combined, we will override the `batch-size` value from the main function with a literal string, *after* the file has been loaded:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_config.rs:system}}
```

Now we change the `Buncher` constructor to not take any arguments anymore. Since we still need to put some values into the struct fields, let's put some default values, say batch size of 0 and a timeout of 1ms. We could also go with an `Option`, if it's important to know whether the component was initialised properly nor not. We also don't know the required capacity for the vector anymore, so we just create an empty one, and extend it later once we have read the batch size from the config file.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_config.rs:new}}
```

And, of course, we must also update the matching `create(...)` call in the main function:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_config.rs:create_buncher}}
```

Finally, the actual config access happens in the `on_start`code. At this point the component is properly initialised and we have acceess to configuration values. The [Hocon](https://docs.rs/hocon/latest/hocon/enum.Hocon.html) type has a bunch of very convenient conversion functions, so we can get a `Duration` directly from the `100 ms` string in the file, for example. Once we have read the values for `batch_size` and `timeout`, we can also go ahead and reserve the required additional space in the `current_batch` vector.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_config.rs:on_start}}
```

At this point we can run the example, and we can see from the regular "50 event"-sized batches in the beginning that our overriding of the batch size worked just fine.

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary with:
> ```bash
> cargo run --release --bin buncher_config
> ```
