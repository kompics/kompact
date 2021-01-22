# Ask

We have now mentioned multiple times that we want to use the "ask"-pattern again, as we did in the [introduction](../../introduction/state.md) briefly already. The "ask"-pattern is simply a mechanism to translate from message-based communication into a thread- or future-based model. It does so by coupling a request message with a *future* that is to be fulfilled with a response for the request. On the sending thread, the one with the future, the result can then be waited for at some point, when it is needed, via blocking, for example. The receiving Actor, on the other hand, gets a combination of the request message with a *promise*, an `Ask` instance, that it can fulfill with the response at any later time. Thus, the `Message` type for such an actor is not `Request` but rather `Ask<Request, Response>`.

> **Note:** The futures returned by Kompact's "ask"-API conform with Rust's built-in async/await mechanism. On top of that Kompact offers some convenience methods that can be called on a `KFuture` to make the common case of blocking on the result easier.

## Manager

The `Message` type for the manager is thus `Ask<Work, WorkResponse>`, which we already saw when describing its [state](state.md). In order to access the actual `Work` instance in our `receive_local(...)` implementation, we use the `Ask::request()` function. 

We then must distribute the work more or less evenly over the available workers. If no workers are available, the manager simply does all the work itself. Otherwise we'll figure out what constitutes and "equal share" (i.e. the `stride`) and then use it to step through the indices into the data, producing sub-ranges, which we send to each worker immediately. Since it may sometimes happen due to rounding that we have a tiny bit of work left at the end, we just do that at the manager and put it directly into the `self.result_accumulator`. This extra work, it the reason for the previously mentioned `+ 1` whenever we are considering the length of the `self.result_accumulator`. It is simply the manager's share of the work. In order to keep this length consistent, we will simply push the `work.neutral` element whenever the manager actually doesn't do any work. Finally, we need to remember to store the request in `self.outstanding_request` so we can reply to it later when all responses have arrived.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:manager_actor}}
```

## Sending Work

When sending work to the manager from the main-thread, we can construct the required `Ask` instance with `ActorRef::ask(...)`. Since we only want to handle a single request at a time, we will immediately `wait()` for the result of the future.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers.rs:main_ask}}
```

> **Note:** For situations where the `Ask` instance is nested, for example, into an enum, Kompact offers the `ActorRef::ask_with` function. Instead of a `Request` value, `ask_with` expects a *function* that takes a `KPromise<Result>` and produces the Actor's `Message` type. This also allows for custom `Ask` variants with more fields, for example.
