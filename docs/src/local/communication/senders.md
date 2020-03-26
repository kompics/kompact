# Senders

The one communication-related thing we haven't touched, yet, is how to do request-response style communication among Actors. The ["ask"-pattern](ask.md) gave us request-reponse between an Actor and some arbitrary (non-pool) thread, [ports](../../introduction/components.md) basically give us some form request-response between *request* and *indication* events (with some broadcasting semantic caveats, of course). But for Actor to Actor communication, we have not seen anything of this sort, yet. In fact, you may have noticed that `receive_local(...)` does not actually give us any *sender* information, such as an `ActorRef`. Neither is this available via the component context as would be the case in Akka. 

In Kompact, for local messages at least, sender information must be passed explicitly. This is for two reasons:

1. It avoids creating an `ActorRef` for every message when it's not needed, since actor references are not trivially cheap to create.
2. It allows the sender reference to be typed with the appropriate message type.

This design gives us basically two variants to do request-reponse. If we know we are always going to response to the same component instance, the most efficient thing to do is to get a reference to it once and then just keep it around as part of our internal state. This avoids constantly creating actor references, and is pretty efficient. If, however, we must respond to multiple different actors, which is often the case, we must make the sender reference part of the request message. We can do that either by adding a field to our custom message type, or simply wrapping our custom message type into the Kompact provided `WithSender` struct. `WithSender` is really the same idea as `Ask`, replacing the `KPromise<Response>` with an `ActorRef<Response>` (yes, there is also `WithSenderStrong` using an `ActorRefStrong` instead).

## Workers with Senders

To illustrate this mechanism we are going to rewrite the Workers example from the previous sections to use `WithSender` instead of the `WorkerPort` communication. We will use `WithSender` here, instead of a stored manager actor reference, to illustrate the point, but it should be clear that the latter will be more efficient as we *always* reply to the manager.

First we remove all mentions of `WorkerPort`, of course. Then we change the worker's `Message` type to `WithSender<WorkPart, ManagerMessage>`. Why `ManagerMessage` and not `WorkResult`? Well, since all communication with the manager now happens via messages, we need to differentiate between messages from the main-thread, which are of type `Ask<Work, WorkResult>` and messages from the worker, which are of type `WorkResult`. Since we can only have a single `Message` type, `ManagerMessage` is simply an enum of both options.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers_sender.rs:70:74}}
```

Thus, when the worker wants to `reply(...)` with a `WorkResult` it actually needs to wrap it in a `ManagerMessage` instance or the compiler is going to reject it.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers_sender.rs:205:217}}
```

In the manager we must first update our state to reflect the new message (and thus reference) types.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers_sender.rs:77:84}}
```

We also remove the port connection logic from the `ControlEvent` handler. Then we change the `Message` type of the manager to `ManagerMessage` and match on the `ManagerMessage` variant in the `receive_local(...)` function. For the `ManagerMessage::Work` variant, we basically do the same thing as in the old `receive_local(...)` function, except that we construct a `WithSender` instance from the `WorkPart` instead of sending it directly to the worker. We then simply copy the code from the old `WorkResult` handler into the branch for `ManagerMessage::Result`.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers_sender.rs:122:185}}
```

The `receive_local(...)` function is getting pretty long, so we should probably decompose it into smaller private functions if we actually wanted to maintain this code.

Now finally, when we want to send the `Ask` from the main-thread, we also need to wrap it into `ManagerMessage::Work`. This prevents us from using `Ask::of`, as it only returns a direct `Ask`, but never a wrapped `Ask`. This gets us back to previously mentioned custom function from `KPromise` to the actual message type, which in this case is `ManagerMessage`.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../../examples/src/bin/workers_sender.rs:240:242}}
```

At this point we should able to run the example again, and see the same behaviour as before.

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary with:
> ```bash
> cargo run --release --bin workers_sender 4 100000
> ```
