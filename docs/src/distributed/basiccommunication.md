# Basic Communication

In order to use remote communication with Kompact we need to replace the default `Dispatcher` implementation, with the provided `NetworkDispatcher`. Custom dispatchers in general are set with the `KompactConfig::system_components(...)` function, which also allows replacement of the system's deadletter box, that is the component that handles messages where no recipient could be resolved. An instance of the `NetworkDispatcher` should be created via its configuration struct using `NetworkConfig::build()`. This type also allows to specify the listening socket for the system via `KompactConfig::with_socket(...)`. The default implementation will bind to `127.0.0.1` on a random free port. Attempting to bind on an occupied port, or without appropriate rights on a reserved port such as 80 will cause the creation of the `KompactSystem` instance to fail.

Once a Kompact system with a network dispatcher is created, we need to acquire actor paths for each component we want to be addressable. Kompact requires components to be explicitly registered with a dispatcher and returns an appropriate actor path as the result of a successful registration. The easiest way to acquire a registered component and a unique actor path for it, is to call `KompactSystem::create_registered(...)` instead of `KompactSystem::create(...)` when creating it. This will return both the component and a future with the actor path, which completes once registration was successful. It is typically recommended not to start a component before registration is complete, as messages it sends with its unique path as source might not be answerable until registration is completed.

Sending messages is achieved by calling `ActorPath::tell(...)` with something that is serialisable (i.e. implements the `Serialisable` trait) and something that can produce a source address as well as a reference to the `Dispatcher`, typically just `self` from within a component.

In order to receive messages, a component must implement (some variant of) the `Actor` trait, and in particular its `receive_network(...)` function. Deserialisation happens lazily in Kompact, that means components are passed serialised data and a serialisation identifier in the form of a `NetworkMessage` message. They must then decide based on the identifier if they want to try and deserialise the content into a message. This can be done using the `NetworkMessage::try_deserialise::<TargetType, Deserialiser>()` function, or more conveniently for multiple messages via the `match_deser!` macro. We will get back to serialisation in more detail [later](serialisation.md).

## Example

In this section we will go through a concrete example of a distributed service in Kompact. In particular, we are going to develop a distributed leader election abstraction, which internally uses heartbeats to establish a "candidate set" of live nodes, and then deterministically picks one node from the set to be the "leader".

### Local Abstraction

Locally we want to expose a port abstraction called `EventualLeaderDetection`, which has no requests and only a single indication: The `Trust` event indicates the selection of a new leader.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/trusting.rs:10:17}}
```

In order to see some results later when we run it, we will also add a quick printer component for these `Trust` events:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/trusting.rs:19:39}}
```

### Messages

We have two ways to interact with our leader election implementation: Different instances will send `Heartbeat` message over the network among themselves. For simplicity we will use [Serde](https://crates.io/crates/serde) as a serialisation mechanism for now. For Serde serialisation to work correctly with Kompact we have assign a serialisation id to `Heartbeat`, that is a unique number that can be used to identify it during deserialisation. It's very similar to a `TypeId`, except that it's guaranteed to be same in any binary generated with the code included since the constant is hardcoded. For the example, we'll simply use `1234` since that isn't taken, yet. In a larger project, however, it's important to keep track of these ids to prevent duplicates.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/trusting.rs:2:8}}
```

Additionally, we want be able to change the set of involved processes at runtime. This is primarily due to the fact that we will use unique paths for now and we simply don't know the full set of unique paths at creation time of the actors that they refer to.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/leader_election.rs:5:6}}
```

### State

There is a bit of state we need to keep track of in our `EventualLeaderElector` component:

- First me must provide the `EventualLeaderDetection` port, of course. 
-  We also need to track the current process set, which we will handle as a boxed slice shared behind an `Arc`, since all components should have the same set anyway. Of course, if we were running this in real distribution, and not just with multiple systems in a single process, we would probably only run a single instance per process and a simple boxed slice (or just a normal vector) would probably be more sensible. 
- Further we must track the current candidate set, for which we will use a standard `HashSet` to avoid adding duplicates. 
- We also need to know how often to check the candidate set and update our leader. Since this time needs to be able to dynamically adjust to network conditions, we keep two values for this in our state: The current `period` and a `delta` value, which we use when we need to adjust the period. The `delta` is technically immutable and could be a constant, but we want to make both values [configurable](../local/configuration.md), so we need to store the loaded values somewhere.
- Finally, we to keep track of the current [timer handle](../local/timers.md) and the current leader, if any.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/leader_election.rs:8:18}}
```

In order to load our configuration values from a file, we need to put something like the following into an `application.conf` file in the current working directory:

```hocon
omega {
	initial-period = 10 ms
	delta = 1 ms
}
```

And the we can load it and start the initial timeout in the handler for the `ControlPort` as before:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/leader_election.rs:76:100}}
```

### Leader Election Algorithm

This part isn't very specific to networking, but basically the election algorithm works as follows: Every time the timeout fires we clear out the current candidate set into a temporary vector. We then sort the vector and take the last element, if any, as the potential new leader. If that new leader is not the same as the current one then either our current leader has failed, or the timeout is wrong. For simplicity we will assume both is true and replace the leader and update the scheduled timeout by adding the `delta` to the current `period`. We then announce our new leader choice via a trigger on the `EventualLeaderDetection` port. Whether or not we replaced the leader, we always send heartbeats to everyone in the process set.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/leader_election.rs:34:67}}
```

### Sending Network Messages

The only place in this example where we are sending remote messages is when we are sending heartbeats:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/leader_election.rs:69:73}}
```

We invoke the `ActorPath::tell(...)` method with a tuple of the actual `Heartbeat` together with the serialiser with want to use, which is `kompact::serde_serialisers::Serde`. We also pass a reference to `self` which will automatically insert our unique actor path into the message as the source and send everything to our system's dispatcher, which will take care of serialisation, as well as network channel creation and selection for us.

### Handling Network Messages

In order to handle (network) messages we must implement the Actor trait as described [previously](../local/communication/messagesandevents.md). The local message type we are handling is `UpdateProcesses` and whenever we get it, we simply replace our current `processes` with the new value.

For network messages, on the other hand, we don't know what are being given, generally, so we get `NetworkMessage`. This is basically a wrapper around a sender `ActorPath`, a serialisation id, and a byte buffer with the serialised data. In our example, we know we only want to handle messages that deserialise to `Heartbeat`. We also know we need to use `Serde` as a deserialiser, since that's what we used for serialisation in the first place. Thus, we use `NetMessage::try_deserialise::<Heartbeat, Serde>()` to attempt to deserialise a `Heartbeat` from the buffer using the `Serde` deserialiser. This call will automatically check if the serialisation id matches `Heartbeat::SER_ID` and if yes, attempt to deserialise it using `Serde`. If it doesn't work, we'll get a `Result::Err` instead. If it does work, however, we don't actually care about the Hearbeat itself, but we insert the sender from the `NetMessage` into `self.candidates`.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/leader_election.rs:104:126}}
```

### System Setup

In this example we need to set up multiple systems in the same process for the first time, since we want them to communicate via the network instead of directly as a preparation for actually running distributed. We are going to take the number of systems (and thus leader election components) as a command line argument. We start each system with the same configuration file and give them each a `NetworkDispatcher` with default settings. This way we don't have to manually pick a bunch of ports and hope they happen to be free. On the other hand that means, of course, that we can't predict what system addresses are going to look like. So in order to give everyone a set of processes to talk to, we need to wait until all systems are set up and all the leader elector components started and registered, collect all the registrations into a vector and then send an update to every component with the complete set.

At this point the system is running just fine and we give it some time to settle on timeouts and elect a leader. We will see the result in the logging messages eventually. Now to see the leader election responding to actual changes, we are going to kill one system at a time and always give it a second to settle. This way we can watch the elector on the remaining systems updating the trust values one by one.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/leader_election.rs:128:183}}
```

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary with:
> ```bash
> cargo run --release --bin leader_election 3
> ```
> Not that running in debug mode will produce a lot of output know as it'll trace all the network messages.
