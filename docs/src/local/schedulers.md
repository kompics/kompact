# Schedulers

Kompact allows the core component scheduler to be exchanged, in order to support different kinds of workloads.
The default `crossbeam_workstealing_pool` scheduler from the [executors](https://docs.rs/executors/latest/executors/crossbeam_workstealing_pool/index.html) crate, for example, is designed for fork-join type workloads. That is, workloads where a small number of (pool) external events spawns a large number of (pool) internal events. 
But not all workloads are of this type. Sometimes the majority of events are (pool) external, and there is little communication between components running on the thread pool. Our somewhat contrived "counter"-example from the [introduction](../introduction/state.md) was of this nature, for example. We were sending events and message from the main-thread to the `Counter`, which was running on Kompact's thread-pool. But we never sent any messages or events to any other component on that pool. In fact, we also only had a single component, and running it a large thread pool seems rather silly. (Kompact's default thread pool has one thread for each CPU core, as reported by [num_cpus](https://crates.io/crates/num_cpus).)

## Changing Pool Size

We will first change just the pool size for the "counter"-example, since that is easily done. 

The number of threads in Kompact's thread pool is configured with the `threads(usize)` function on a `KompactConfig` instance. We will simply pass in `1usize` there, before constructing our Kompact system.

That we change this line

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/counters.rs:74}}
```

to this:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/counters_pool.rs:74:76}}
```

If we run this, we will see exactly (modulo event timing) the same output as when running on the larger pool with Kompact's default settings.

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary with:
> ```bash
> cargo run --release --bin counters_pool
> ```

## Changing Scheduler Implementation

Ok, so now we'll switch to a pool that is designed for external events: `crossbeam_channel_pool`, also from the [executors](https://docs.rs/executors/latest/executors/crossbeam_channel_pool/index.html) crate. 

We can set the scheduler implementation to be used by our Kompact system using the `executor(...)` function on a `KompactConfig` instance. That function expects a closure from the number of threads (`usize`) to something that implements the `executors::common::Executor` [trait](https://docs.rs/executors/latest/executors/common/trait.Executor.html).

> **Note:** There is actually a more general API for changing scheduler, in the `scheduler(...)` function, which expects a function returning a `Box<dyn kompact::runtime::Scheduler>`. The `executor(...)` function is simply a shortcut for using schedulers that are compatible with the [executors](https://crates.io/crates/executors) crate.

In order to use the `crossbeam_channel_pool` scheduler, we need to import the `kompact::executors` module, which is simply a re-export from the [executors](https://crates.io/crates/executors) crate:

```rust,edition2018,no_run,noplaypen
use kompact::executors;
```

With that, all we need add is the following line of code, which selects the `ThreadPool` implementation from the `crossbeam_channel_pool` module, instead of the one from the `crossbeam_workstealing_pool` module, that is default.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/counters_channel_pool.rs:77}}
```

If we run this, again, we will see exactly (modulo event timing) the same output as when running on the larger pool with Kompact's default settings.

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary with:
> ```bash
> cargo run --release --bin counters_channel_pool
> ```
