# Messages and Events

To begin with we must decide what we want to send over events and what over messages, and how exactly our message/event types should look like.

## Messages

For the incoming work assignments, we want some kind of request-response-style communication pattern, so we can reply once the aggregation is complete. 

So we will use Actor communication for this part of the example, so that we can later use the "ask"-pattern again from the main-thread. For now, we know that the result of the work is a `u64`, which we will wrap in a `WorkResult` struct for clarity of purpose. 

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:68}}
```

We also know that we need to pass some data array and an aggregation function when we make a request for work to be done. Since we will want to share the data later with our workers, we'll put it into an atomic reference, i.e. `Arc<[u64]>`. For the aggregation function, we'll simply pass a function pointer of type `fn(u64, &u64) -> u64`, which is the signature accepted by the `fold` function on an iterator. However, in order to start a `fold`, we also need a neutral element, which depends on the aggregation function. So we add that to the work request as a field as well. 

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:4:8}}
```

We will also use message based communicaton for the work assignments for the individual workers in the pool. Since we want to send a different message to each worker, message addressing is a better fit here, than component broadcasting. The `WorkPart` message is really basically the same the `Work` message, except that we add the range that this particular worker is supposed to aggregate to it.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:34:39}}
```

> **Note:** Both Actor messages and events must implement the `std::fmt::Debug` trait in Kompact. Since both `Work` and `WorkPart` contain function pointers, which do not have a sensible `Debug` representation, we implement it manually instead of deriving and simply put a `"<function>"` placeholder in its stead. Since our data arrays can be really big, we also only print their length.

## Events

We will use Kompics-style communication for the results going from the workers to the manager. However, these are of the same type as the final result; a `u64` wrapped in a `WorkResult`. So all we have to do is add the `std::clone::Clone` trait, which is required for events.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:67:68}}
```

We also need a port on which the `WorkResult` can travel; let's call it a `WorkerPort`. Based on the naming, say a `Worker` *provides* the `WorkerPort` service, so messages from `Worker` to `Manager` are *indications*. Since we are using messages for requesting work to be done, we don't need any *request* event on the `WorkerPort` and will just use the empty `Never` type again.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:70:74}}
```
