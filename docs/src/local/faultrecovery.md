# Fault Recovery

Sometimes panics can happen in components that provide crucial services to the rest of system. And while it is, of course, better to try and have `Result::Err` branches in place for every anticipated problem, with the use of 3rd party libraries and even some standard library functions not every possible panic can be prevented. Thus components can "fault" unexpectedly and the service their provide suddenly is not available.

In order to deal with cases where simply letting a component die is just not good enough, Kompact provdes a simple mechanism for recovering from faults: For every individual component, users can register a `RecoveryFunction`, that is basically a function which takes a `FaultContext` and produces a mechanism to recover from that fault, called a `RecoveryHandler`. This recovery handler is executed by the system's `ComponentSupervisor` when it is informed of the fault.

> **Note:** Fault recovery only works when the executed binary is compiled with *panic unwinding*. In binaries set to `panic=abort` none of this applies and fault handling must be dealt with outside the process running the Kompact system.

> **Warning:** Panicking within the `RecoveryHandler` will destroy the `ComponentSupervisor`, which is unrecoverable, and thus lead to "poisoning" of the whole Kompact system.

A `RecoveryFunction` can be registered via `Component::set_recovery_function(...)` from outside a component or via `ComponentContext::set_recovery_function(...)` from within. Either way causes a `Mutex` to be locked, so be aware of the performance cost and the risk for deadlock when using with the latter function (since you are already holding the `Mutex` on the `ComponentDefinition` at this point). That being said, `set_recovery_function(...)` can be called repeatedly to update the state stored in the function. This is particularly useful as a very simply snapshotting mechanism, allowing a replacement component later to be started from this earlier state snapshort, instead of starting from scratch.

Apart from inspecting the `FaultContext` the recovery function must produce some kind of recovery handler. The simplest (and default) handler is `FaultContext::ignore()` which performs no additional action on the supervisor to recover the faulted component. If custom handling is required, it can be provided via `FaultContext::recover_with(...)`, where the user can provide a closure that may use the `FaultContext`, the supervisor's `SystemHandle`, and the supervisor's `KompactLogger` to react to the fault. What happens in this function is completely up to the user and the needs of the application. A common case might be to log some particular message, or create a new component via `system.create(...)` and start it with `system.start(...)`, for example.

> **Warning:** Do *not* block within the `RecoveryHandler`, as that will prevent the `ComponentSupervisor` from doing its job. In particular, absolutely do not block on lifecycle event (e.g., `start_notify`) as that will deadlock the supervisor! If you need to execute a complicated sequence of asynchronous commands to recover from a fault, it is recommended to use a temporary component for this sequence, which can simply be started from the recovery handler.

> **Note:** After recovery all component references (`Arc<Component<CD>>`) and actor references to the old component will be invalid. If your application needs their functionality, you need to devise a mechanism to share the new references (e.g., concurrent queues, `Arc<Mutex<...>>`, etc.). If the component provides a [named service](distributed/namedservices.md) the alias must be re-registered to point to the new instance.

## Unstable Counter Example

In order to showcase the recovery mechanism, we write a timer-based counter, which occasionally overflows and thus causes the component to crash. In order not to lose all the instances we have already counted, we will occasionally store the current count in the recovery function, and during recovery start from that point, i.e. a slightly outdated count, but at least not 0.

In addition to the current count, we will store references to two scheduled timers: For every `count_timeout` we want to increase our `count` by 1 and for every `state_timeout` we will update the recovery function.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/unstable_counter.rs:state}}
```

By default, we just initialise the count to 0 and leave the timeouts unset until we are started.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/unstable_counter.rs:default}}
```

During start, we schedule the two timers and also set our recovery function. Within the recovery function, we simply store the state we want to remember, i.e. the two timeouts and the count. When it is called we produce a recovery handler from this state, that cancels the old timeouts and then starts a new `UnstableCounter` by passing in the last count we stored. 

As usual, we also cancel our timeouts when we are stopped or killed.


```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/unstable_counter.rs:lifecycle}}
```

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/unstable_counter.rs:with_state}}
```

> **Note:** Cancelling the timeouts during faults is not really necessary, as they will be cleaned automatically when the faulty component is dropped. Since we don't control who is holding on to component references, though, it may avoid some unnecessary overhead on a heavily loaded timer if done more eagerly, like this. It is included here mostly as an example of possible cleanup code in a recovery handler.

When our timeouts are triggered we must handle them. The count timeout is easy, we simply increment the `self.count` variable using the `checked_add` to cause a panic on overflow even in release builds. During the state timeout, we essentially reintroduce the recovery function from the `on_start` lifecycle handler, so that we update the state it closed over.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/unstable_counter.rs:timeouts}}
```

In order to run this, we simply start the default instance of the `UnstableCounter` onto a Kompact system and then wait for a bit to let it count. The output will show the counting and the crashes. We can see that after the crash do not start counting from 0, but instead from something much higher, around 199 depending on your exact timing. Also notice how we crash much faster after the first time, since it doesn't take as long to reach 255 again.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/unstable_counter.rs:main}}
```

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary with:
> ```bash
> cargo run --release --bin unstable_counter
> ```
