# Internal State

Now that we have looked at the fundamental ideas of components and actors in isolation, let us look at something both our models share: The idea that every component/actor has its own internal state, which it has exclusive access to, without the need for synchronisation.

Access to internal state is what separates our components from being simple producers and consumers of messages and events, and makes them a powerful abstraction to build complicated systems, services, and applications with. But so far, our examples have not used any internal state at all – they simply terminated after the first event or message. In this chapter we will build something slightly less boring: a "Counter".

## A Counter Example
(The pun in the title is mostly intended ;)

In this example we will make use of the simplest of state variables, that is integer counters. We count both messages and events separately, to see how the models work together. Since state that is never read is totally useless, we will also allow the counters to be queried. In fact, we will simply consider any update also a query and always respond with the current count.

### Messages

First we need to set up the message types and ports:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/counters.rs:messages}}
```

We will use the same types both for the port and actor communication, so `CountMe` and `CurrentCount` are both events and messages.
Since we want to provide a counter *service*, we'll say that `CountMe` is going to be a *request* on the `CounterPort`, and `CurrentCount` is considered an *indication*. We could also design things the other way around, but this way it matches better with our "service" metaphor.

### State

Our internal state is going to be the two counters, plus the component context and a *provided* port instance for `CounterPort`:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/counters.rs:state}}
```

We also added a quick `current_count()` function, which access our internal state constructs a `CurrentCount` instance from it. This way, we can reuse the function for both event and message handling.

### Counting Stuff

In addition to counting the `CountMe` events and messages, we will also count control events incoming at the `ControlPort`. However, we will not respond to those. As mentioned previously, control events are handled indirectly via the `ComponentLifecycle` trait. On the other hand, for every `CountMe` event we will respond with the current state of both counters.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/counters.rs:behaviour}}
```

In the Kompics-style communication, we reply by simply triggering the `CurrentCount` event on our `counter_port` to whoever may listen. In the Actor-style, we need to know some reference to respond to. Since we are not responding to another component, but to the main-thread, we will use the `Ask`-pattern provided by Kompact, which converts our response message into a future that can be blocked on, until the result is available. We will describe this pattern in more detail in a [later section](../local/communication/ask.md).

### Sending Stuff

In order to count something, we must of course send some events and messages. We could do so in Actor-style by using `tell(...)` as before, but this time we want to wait for a response as well. So instead we will use `ask(...)` and wrap our `CountMe` into an `Ask` instance as required by our actor's implementation. In the Kompics-style, we can trigger on a port reference using `system.trigger_r(...)` instead. Whenever we get a response, we print it using the system's logger:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/counters.rs:main}}
```

There are two things worth noting here:

1. We are never getting any responses from the Kompics-style communication. There simply isn't anything subscribed to our port, so the responses we are sending are simply dropped immediately. Kompact does not provide an `Ask`-equivalent for ports, since maintaining two mechanisms to achieve the same effect is inefficient, and this communication pattern is very unusual for the Kompics model.
2. We are also not getting any feedback when the events sent to the port are being handled. In order to see them being handled at all, we added a `thread::sleep(...)` invocation there. Events and messages in Kompact do **not** share the same queues and there are no ordering guarantees between them. Quite the opposite, in fact: Kompact ensures a certain amount of fairness between the two mechanisms and by the default will try to handle one message for every event it handles. Thus, without the sleep, we would see between one (the start event) and 101 events being counted when the final `Ask` returns. Even like this, it's not guaranteed that any or all events are handled before the sleep expires. It's just very likely, if your computer isn't terribly slow.

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary with:
> ```bash
> cargo run --release --bin counters
> ```

## Conclusions

We have shown how Kompact handles internal state, and that it is automatically shared between the two different communication styles Kompact provides.

We have also seen, that there are no ordering guarantees between ports and message communication, something that is also true among different ports on the same component. It is thus important to remember that for applications, that require a certain sequence of events to be processed before proceeding, verifying completion must happen through the same communication style and even through the same port.

We will go through all the new parts introduced in this chapter again in detail in the following sections.
