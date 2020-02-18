# System

In order to run any Kompact component, we need a `KompactSystem`. The system managed the runtime variables, the thread pool, logging, and many aspects of the Kompact. Such a system is created from a `KompactConfig` via the `build()` function. The config instance allows customisation of many parameters of the runtime, which we will discuss in upcoming sections. For now, the `default()` instance will do just fine.  It creates a thread pool with one thread for each CPU core, as reported by [num_cpus](https://crates.io/crates/num_cpus), schedules fairly between messags and events, and does some internal message/event batching to improve performance.

> **Note:** As opposed to Kompics, in Kompact it is perfect viable to have multiple systems running in the same process, for example with different configurations.

When a Kompact system is not used anymore it should be shut down via the `shutdown()` function. Sometimes it is a component instead of the main-thread that must decide when to shut down. In that case, it can use `self.ctx.system().shutdown_async()` and the main-thread can wait for this complete with `await_termination()`. 

> **Note:** Neither `shutdown_async()` nor `await_termination()` has a particularly efficient implementation, as this should be a relatively rare thing to do in the lifetime of a Kompact system, and thus doesn't warrant optimisation at this point. That also means, though, that `await_termination()` should definitely **not** be used as a timing marker in a benchmark, as *some* people have done with the equivalent Akka API.

## Tying Things Together

For our worker pool example, we will simply use the default configuration, and start a `Manager` component with a configurable number of workers. We then create some data array of a configurable size and send a work request with it and an aggregation function to the manager instance. We'll use simple addition with overflow as our aggregation function, which means our neutral element is `0u64`. The data array we'll generate is simply the integers from `1` to `data_size`, which means our aggregate will actually calculate a [triangular number](https://en.wikipedia.org/wiki/Triangular_number) (modulo overflows, for which we probably don't have enough memory for the data array anyway). Since that particular number has a much simpler solution, i.e. \\( \sum_{k=1}^n k = \frac{n\cdot(n+1)}{2} \\), we will also use an assertion to verify we are actually producing the right result (again, this probably won't work if we actually do overflow during aggregration, but oh well...details ;).

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:234:255}}
```

Now all we are missing are values for the two parameters; `num_workers` and `data_size`. We'll read those from command-line so we can play around with them.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:222:232}}
```

Now we can run our example, by giving it some parameters, say `4 100000` to run 4 workers and calculate the 100000th triangular number. If you play with larger numbers you'll see that a) it uses more and more memory and b) it will spend most of its time creating the original array, as our aggregration function is very simple and parallelisable, while data creation is done sequentially. Of course, in a real worker pool we'd probably read data from disk somewhere or from an already memory resident set, perhaps. But this is good enough for our little example.

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary with:
> ```bash
> cargo run --release --bin workers 4 100000
> ```
