# State

Of course, we are going to need some component state to make this aggregation pool work out correctly.

## Worker

Our workers are pretty much stateless, apart from the component context and the *provided* `WorkerPort` instance. Both of these fields, we always simply initialise with the `new()` function.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:192:204}}
```

## Manager

The manager is a bit more complicated, of course. First of all it needs to know how many workers to start in the first place, and then keep track of all their instances (`Arc<Component<Worker>>`), so it can shut them down again later. We will also have the manager hang on to some actor references for each worker, so we don't have to create a new one from the instances later. In fact, we will hold on to strong references (`ActorRefStrong<WorkPart>`), since we know the workers is not going to be deallocated anyway until we remove them from our vector of instances. 

> **Note:** Strong actor references are a bit more efficient than weak ones (`ActorRef<_>`), as they avoid upgrading some internal `std::sync::Weak` instances to `std::sync::Arc` instances on every message. However, they prevent deallocation of the target components, and should thus be used with care in dynamic Kompact systems.

Additionally, we need to keep track of which request, if any, we are currently working on, so we can answer it later. We'll make our lives easy for now, and only deal with one request at a time, but this mechanism can easily be extended for multiple outstanding requests with some additional bookkeeping.

We also need to put the partial results from each worker somewhere and figure out when we have seen all results, so we can combine them and reply to the request. We could simply keep a single `u64` accumulator value around, into which we always merge as soon as we get a result from a worker. In that case, we would also need a field to keep track of how many responses we have already received, so we know when we are done. Since we won't have too many workers, however, we will simply put all results into a vector, and considers ourselves to be done when the vector contains `number_of_workers + 1` entries (we'll get back to the `+1` part later).

And finally, of course, we also need a component context and we need to *require* the `WorkerPort`.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:76:98}}
```
