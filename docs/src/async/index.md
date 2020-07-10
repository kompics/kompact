# Async/Await Interaction

In addition to providing its own asynchronous APIs as described in the previous sections, Kompact also allows components to interact with Rust's async/await features in a variety of manners.
In particular, Kompact provides three different semantics for this interaction:

1. A component can "block" on a future, suspending all other processing until the result of the future is available.
2. A component can run a number of futures concurrently with other messages and events, allowing each future safe mutable access to its internal state whenever it is polled.
3. A component or Kompact system can spawn futures to run on its executor pool.


The third variant is unremarkable and works like any other futures executor. It is invoked via `KompactSystem::spawn(...)` or via `ComponentDefinition::spawn_off(...)`. Variants 1 and 2, however, provide novel interactions between an asychronous API and an actor/component system, and so we will describe in more detail using an example below.

## Example

In order to show off interaction between Kompact components and asynchronous calls, we use the asynchronous DNS resolution API provided by the [async-std-resolver](https://crates.io/crates/async-std-resolver) crate to build a DNS lookup component. In order to tell the component what to look up, we will read domain names from stdin, send them via `ask(...)` to the component and wait for the result to come in, which we then print out. In fact, we will allow multiple concurrent queries to be specified as comma-separated list, to show off concurrent future interaction in Kompact components.

### Messages

The messages we need a very simple, we simply pass a `String` representing a single domain name as a request, and we return an already preformatted string with the resolved IPs as a response.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/dns_resolver.rs:messages}}
```

### State

The component's state is almost as simple, we simply require the usual component context and an instance of the asynchronous dns resolver. Since creation of that instance is performed asynchronously by the async-std-resolver library, we won't have the instance we need available during component creation, and thus use an option indicating whether our component has already been properly initialised or not.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/dns_resolver.rs:state}}
```

### Setup

When we create a resolver instance via `async_std_resolver::resolver`, we actually get a future back that we need to wait for. But our DNS component can't perform any lookups until this future completed. Normally, we would have to manually queue up all requests received during that period until the future completed and then replay them. Instead, we can "block" on the provided future, causing the component itself to enter into a `blocked` lifecycle state, during which it handles no messages or events. Only when the future's result is available will the component enter the `active` state and process other events and messages as normal again.

In order to enter the `blocked` state, we must return a a special variant of the `Handled` enum, which is obtained from the `Handled::block_on(...)` method. This method takes the `self` reference to the component and an asynchronous closure, that is a closure that produces a future when invoked. This closure is given a single parameter by the Kompact API, which is an access guard object to a mutable component reference. In order words, a special owned struct that can be mutably dereferenced to the current component definition type. This guard object ensures safe mutable access to the current component instance, whenever the resulting future is polled, but prevents holding on to actual references over `await` calls (which are illegal). It is very important that this guard object is **never** sent to another thread from within the future. The async closure can not directly close over the component's `self` reference, as the correct lifetime for it can not be guaranteed. Only the references obtained from the special guard object are safe in between `await` calls.

Having said all that, in our case the async closure very simply `await`s the result of the resolver creation and then stores it locally, after which the component unblocks.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/dns_resolver.rs:lifecycle}}
```

> **Note:** The complicated looking `move |async_self| async move {...}` syntax is currently only necessary on stable Rust. On nightly, the much easier `async move |async_self| {...}` syntax is already available.

### Queries

To handle queries we must call `lookup(...)` on the resolver, which returns a future of a dns lookup result, which we must await before replying to the actual request. As we want to handle multiple such outstanding lookups in parallel, we can't simply block on this future as we did before. Instead we want to spawn the future, to run locally on the component whenever it is polled, via `ComponentDefinition::spawn_local(...)`. In this way, we have the same advantages as during blocking, but we can handle mutliple outstanding requests in parallel. Technically, except for some logging, we do not really need access to the component's state in this particular case, but we will use it anyway to showcase the API.

Since the result of a DNS query can consist of multiple IP addresses, we construct a single string by formatting them together with the domain into an enumerated list. We then return that string a reply to the original request.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/dns_resolver.rs:actor}}
```

### Running

In our `main` function we want to set up the component, and then read from the command line over and over until the user enters `"stop"` to end the loop. For each line we read that is not `"stop"`, we will simply assume that it a comma-separated list of domain names. We split them apart, remove unnecessary spaces and then send them one by one to the `DNSComponent` via `ask(...)`. Instead of waiting for each future immediately, we store the response futures until all requests have been sent, and only *then* do we wait for each of them in order. We could also have waited for them in the order they are replied to, instead, it doesn't really matter in this case. Only when the last of them has been completed, do we read input again.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/dns_resolver.rs:main}}
```

> **Note:** If you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) and are trying to run from there, you need to specify the concrete binary with:
> ```bash
> cargo run --bin dns_resolver
> ```
