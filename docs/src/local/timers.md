# Timers

Kompact comes with build-in support for scheduling some execution to happen in the future. Such scheduled execution can be either one-off or periodically repeating. Concretely, the scheduling API allows developers to subscribe a handler closure to the firing of a timeout after some `Duration`. This closure takes two arguments: 

1. A new mutable reference to the scheduling component, so that its state can be accessed safely from within the closure, and
2. a handle to the timeout being triggered, so that different timeouts can be differentiated. The handle is an opaque type named `ScheduledTimer`, but currently is simply a wrapper around a `Uuid` instance assigned (and returned) when the timeout is originally scheduled.

## Batching Example

In order to show the scheduling API, we will develop a batching component, called a `Buncher`, that collects received events locally until either a pre-configured batch size is reached or a defined timeout expires, whichever happens first. Once the batch is closed by either condition, a new `Batch` event is triggered on the port containing all the collected events.

Since there are two variants of scheduled execution, we will also implement two variants of the batching component:

1. The *regular* variant simply schedules a periodic timeout once, and then fires a batch whenever the timeout expires, no matter how long ago the last batch was triggered (which could be fairly recently if it was triggered by the batch size condition).
2. The *adaptive* variant schedules a new one-off timeout for every batch. If a batch is triggered by size instead of time, this variant will cancel the current timeout and schedule a new one with the full duration again. This approach is more practical, as it results in more evenly sized batches than the *regular* variant.

### Shared Code

Both implementations share the basic events and ports involved. They also both use a printer component for `Batch` events, which simply logs the size of each batch so we can see it during execution.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/batching.rs}}
```

They'll also really use the same running code, even though its repeated in each file so it picks the correct implementation. In either case, we set up the `Buncher` and the `BatchPrinter` in a default system, connect them via `biconnect_components(...)` and then send them two waves of `Ping` events. The first wave comes around every millisecond, depending on concrete thread scheduling by the OS, while the second comes around every second millisecond.
With a batch size of 100 and a timeout of 150ms we will see mostly full batches in the first wave, while we usually see time-triggered waves in the second wave.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_regular.rs:main}}
```

### Regular Buncher

The state of the `Buncher` consists of the two configuration values, batch size and timeout, as well as the `Vec` storing the currently collecting batch and the handle for the currently scheduled timeout (`ScheduledTimer`).

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_regular.rs:state}}
```

As part of the lifecyle we must set up the timer, but also make sure to clean it up after we are done. To be able to do so, we must store the `ScheduledTimer` handle that the `schedule_periodic(...)` function returns in a local field, so we can access it when we are paused or killed and pass it as a parameter to `cancel_timer(...)`.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_regular.rs:lifecycle}}
```

> **Warning:** Not cleaning up timeouts will cause them to be triggered over and over again. Not only will it slow down the timer facilities, but may also cause a lot of logging, depending on the logging level you are compiling with. Make sure to always clean up scheduled timeouts, especially periodic ones.

The first parameter of the `schedule_periodic(...)` function is the time until the timeout is triggered the first time. The second parameters gives the periodicity. We'll use the same value for both here.
The actual code we want to call whenever our periodic timeout is triggered is a private function called `handle_timeout(...)` which has the signature expected by the `schedule_periodic(...)` function. It checks that the timeout we got is actually an expected timeout, before invoking the actual `trigger_batch(...)` function.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_regular.rs:private_functions}}
```

The actual handler for the `Ping` events on the `Buncher` is pretty straight forward. We simply add the event to our active batch. Then we check if the batch is full, and if it is we again call `trigger_batch(...)`.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_regular.rs:batching_port}}
```

If we go and run this implementation with the main function from above, we will see that for the first wave we often get a full batch followed by a very small batch, e.g.:

```bash
Mar 10 15:42:03.734 INFO Got a batch with 100 Pings., ctype: BatchPrinter, cid: 4c79e0b1-1d74-455b-987a-14f66bcd4025, system: kompact-runtime-1, location: docs/examples/src/batching.rs:33
Mar 10 15:42:03.762 INFO Got a batch with 22 Pings., ctype: BatchPrinter, cid: 4c79e0b1-1d74-455b-987a-14f66bcd4025, system: kompact-runtime-1, location: docs/examples/src/batching.rs:33
Mar 10 15:42:03.890 INFO Got a batch with 100 Pings., ctype: BatchPrinter, cid: 4c79e0b1-1d74-455b-987a-14f66bcd4025, system: kompact-runtime-1, location: docs/examples/src/batching.rs:33
Mar 10 15:42:03.912 INFO Got a batch with 16 Pings., ctype: BatchPrinter, cid: 4c79e0b1-1d74-455b-987a-14f66bcd4025, system: kompact-runtime-1, location: docs/examples/src/batching.rs:33
```

This happens because we hit 100 Pings somewhere around 120ms into the timeout, and then there is only around 30ms left to collect events for the next batch. This, of course, isn't particularly great behaviour for a batching abstraction. We would much rather have regular batches if the input is coming in regularly.

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary with:
> ```bash
> cargo run --release --bin buncher_regular
> ```

### Adaptive Buncher

In order to get more regular sized batches, we need to reset our timeout whenever we trigger a batch based on size. Since this will cause our timeouts to be very irregular anyway, we will just skip periodic timeouts altogether and always schedule a new timer whenever we trigger a batch, no matter which condition triggered it.

To do so, we must first change the handler for lifecycle events to use `schedule_once(...)` instead of `schedule_periodic(...)`.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_adaptive.rs:lifecycle}}
```

We must also remember to reschedule a new timeout when we handle a current one. It's important to correctly replace the handle for the timeout so we never accidentally trigger on an outdated timeout.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_adaptive.rs:handle_timeout}}
```

Finally, when we trigger a batch based on size, we must proactively cancel the current timeout and schedule a new one. Note that this cancellation API is asychronous, so it can very well happen that an already cancelled timeout will still be invoked because it was already queued up. That is why we must always check for a matching timeout handle before executing a received timeout.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/buncher_adaptive.rs:batching_port}}
```

If we run again now, we can see that the first wave of pings is pretty much always triggered based on size, while the second wave is always triggered based on timeout, giving us much more regular batches.

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary with:
> ```bash
> cargo run --release --bin buncher_adaptive
> ```
