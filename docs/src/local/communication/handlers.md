# Handlers

Now that we have set up all the messages, events, and the state, we need to actually implement the behaviours of the `Manager` and the `Worker`. That means we need to implement the `Actor` trait for both components, to handle the messages we are sending, and we also need to implement the appropriate event handling traits: `ComponentLifecycle` and `Require<WorkerPort>` for the `Manager`, and `Provide<ControlPort>` and `Provide<WorkerPort>` for the `Worker`.

## Worker

### Actor

Since the worker is stateless, its implementation is really simple. It's basically just a francy wrapper around a slice `fold`. That is, whenever we get a `WorkPart` message on our `receive_local(...)` function from the `Actor` trait, we simply take a slice of the range we were allocated to work on, and then call `fold(msg.neutral, msg.merger)` on it to produce the desired `u64` result. And then we simply wrap the result into a `WorkResult`, which we trigger on our instance of `WorkerPort` so it gets back to the manager.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:worker_actor}}
```

### Ports

We also need to provide implementations for `ComponentLifecycle` and `WorkerPort`, because they are expected by Kompact. However, we don't actually want to do anything interesting with them. So we are going to use the `ignore_lifecycle!(Worker)` macro to generate an empty `ComponentLifecycle` implementation, and similarly use the `ignore_requests!(WorkerPort, Worker)` macro to generate an empty `Provide<WorkerPort>` implementation.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:worker_ports}}
```

## Manager

The manager needs to do three things: 1) Manage the worker pool, 2) split up work requests and send chunks to workers, and 3) collect the results from the workers, combine them, and reply with the final result.

We will deal with 1) in a `ComponentLifecycle` handler, and with 3) in the handler for `WorkerPort`, of course. For 2), however, we want to use the "ask"-pattern again, so we will look at that in the next section.

### ComponentLifecycle

Whenever the manager is started (or restarted after being paused) we must populate our pool of workers and connect them appropriately. We can create new components from within an actor by using the `system()` reference from the `ComponentContext`. Of course, we must also remember to actually start the new components, or nothing will happen when we send messages to them later. Additionally, we must fill the appropriate state with component instances and actor references, as we discussed in the previous section.

> **Note:** As opposed to many other Actor or Component frameworks, Kompact does **not** produce a hierarchical structure when calling `create(...)` from within a component. This is because Kompact doesn't have such a strong focus on error handling and supervision as other systems, and maintaining a hierarchical structure is more complicated than maintaining a flat one.

When the manager gets shut down or paused we will clean up the worker pool completely. Even if we are only temporarily paused, it is better to reduce our footprint by cleaning up, than forgetting to do so and hanging on to all the pool memory while not running anyway.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:manager_lifecycle}}
```

> **Note:** We are getting the required `ActorRefStrong<WorkPart>` by first calling `actor_ref()` on a worker instance and then upgrading the result via `hold()` from an `ActorRef<WorkPart>` to `ActorRefStrong<WorkPart>`. This returns a `Result`, as upgrading is impossible if the component is already deallocated. However, since we are holding on to the actual instance of the component here as well, we *know* it's not deallocated, yet, and `hold()` cannot fail, so we simply call `expect(...)` to unwrap it.

### Worker Port

Whenever we get a `WorkResult` from a worker, we will temporarily store it in `self.result_accumulator`, as long we have an outstanding request. After every new addition to the accumulator, we check if we have gotten all responses with `self.result_accumulator.len() == (self.num_workers + 1)` (again, more on the `+ 1` later). If that is so, we will do the final aggregation on the accumulator via `fold(work.neutral, work.merger)` and then `reply(...)` to the outstanding request. Of course, we must also clean up after ourselves, i.e. reset the `self.outstanding_request` to `None` and clear out `self.result_accumulator`.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:manager_worker_port}}
```

