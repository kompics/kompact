# Named Services

In the last section we discussed how to build a leader election mechanism with a bunch of networked Kompact systems. But we couldn't actually run it in deployment, because we couldn't really figure out how to collect a list of actor paths for all the processes and then distributed that list to every process. This happens because we can only know the actor path of an actor *after* we have created it. We could have manually distributed the actor paths, by writing the assigned path to a file, then collecting it externally, and finally parsing paths from said collected file and passing them to each elector component. But that wouldn't be a very nice system now, would it? 

What we are missing here is a way to predict an `ActorPath` for a particular actor on a particular system. If we can know even a single path on a single host in the distributde actor system, we can have everyone send a message there, which will give that special component the unique paths for everyone that sends there, which it can in turn distribute back to everyone who has "checked in" in this manner. This process is often referred to as "bootrapping". In this section we are going to use named actor paths, which we can predict given some information about the system, to build a bootstrapping "service" for our leader election group.

## Messages

For the bootstrapping communication we require a new `CheckIn` message. It doesn't actually need any content, since we really only care about the `ActorPath` of the sender. We will reply to this message with our `UpdateProcesses` message from the previous section. However, since that has to go over the network now, we need to make it serialisable. We also aren't locally sharing the process set anymore, so we turn the `Arc<[ActorPath]>` into a simple `Vec<ActorPath>`.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/bootstrapping.rs:10:20}}
```

## State

Our bootstrap server's state is almost trivial. All it needs to keep track of is the current process set.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/bootstrapping.rs:22:33}}
```

We also need to alter our leader elector a bit. First it needs to know the actor path of the bootstrap server, so it can actually check in. And second, we need to adapt the type of `processes` to be in line with our changes to `UpdateProcesses`. We'll make it a `Box<[ActorPath]>` instead of `Arc<[ActorPath]>` and do the conversion from `Vec<ActorPath>` whenever we receive an update.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/bootstrapping.rs:57:83}}
```

## Behaviours

The behaviour of the bootstrap server is very simple. Whenever it gets a `CheckIn`, it adds the source of the message to its process set and then broadcasts the new process set to every process in the set. We will use the `NetworkActor` trait to implement the actor part here instead of `Actor`. `NetworkActor` is a convenience trait for actors that handle the same set of messages locally and remotely and ignore all other remote messages. It handles the deserialisation part for us, but we must tell it both the `Message` type and the `Deserialiser` type to use. Of course, in this case we don't actually do anything for local messages, since we need the sender and local messages don't have one.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/bootstrapping.rs:35:55}}
```

We must also make some small changes to the behaviour of the leader elector itself. First of all we must now send the `CheckIn` when we are being started. As before we are using `Serde` as a serialisation mechanism, so we really only have to add the following line to the `ControlEvent::Start` branch:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/bootstrapping.rs:131}}
```

We also have to change how we handle `UpdateProcesses` slightly, since they are now coming in over the network. We thus have to move the code from `receive_local` to `receive_network`. But now we have two different possible network messages we could deserialise whenever we get a `NetMessage`: It could either be a `Heartbeat` or an `UpdateProcesses`. Since trying through them individually one by one is somewhat inefficient, what we really want is something like this:

```rust,edition2018,no_run,noplaypen
match msg.ser_id() {
	Heartbeat::SER_ID => // deserialise and handle Heartbeat
	UpdateProcesses::SER_ID => // deserialise and handle UpdateProcesses
}
```

Kompact provides the `match_deser!` macro to generate code like the above, since this is very common behaviour and writing it manually gets somewhat tedious eventually. The syntax for each different message case in the macro is basically `variable_name: MessageType [DeserialiserType] => <body>,`. Using this, our new actor implementation becomes the following:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/bootstrapping.rs:157:182}}
```


## System

Now the real difference happens in the way we set up the Kompact systems. In the last section we set up a configurable number of systems that were all the same in the same process. Now we are only going to run a single system per process and we have two different setups as well: Most processes will be "clients" and only run the leader elector and the trust printer, but one process will additionally run the `BootstrapServer`.

### Server

The one thing that sets our bootstrap server creation apart from any other actor we have created so far, is that we want a *named actor path* for it. Basically, we want any other process to be able to constuct a valid `ActorPath` instance for the bootstrap server, such as `tcp://127.0.0.1:<port>/bootstrap` , given only the port for it. In order to make Kompact resolve that path to the correct component we must do two things:

1. Make sure that the Kompact system actually runs on localhost at the given port, and
2. Register a named path alias for the `BootstrapServer` with the name `"bootstrap"`.

To achieve the first part, we create the `NetworkDispatcher` from a `SocketAddr` instance that contains the correct IP and port instead of using the default value as we did before. To register a component with a named path, we must call `KompactSystem::register_by_alias(...)` with the target component and the path to register. The rest is more or less as before.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/bootstrapping.rs:205:232}}
```

### Client

The client setup works almost the same as in the previous section, except that we need to construct the required `ActorPath` instance for the bootstrap server given its `SocketAddr` now. We can do so using `NamedPath::with_socket(...)` which will construct a `NamedPath` instance that can easily be converted into an `ActorPath`. We pass this instance to the leader elector component during construction.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/bootstrapping.rs:243:260}}
```

### Running with Commandline Arguments

All that is left to do is to convert the port numbers given on the command line to the required `SocketAddr` instances and calling the correct method. When we are given 1 argument (port number) we will start a bootstrap server, and if we are given 2 argumments (server port and client port) we will start a client instead.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/bootstrapping.rs:184:203}}
```

Now we can run this by first starting a server in one shell and then a few clients in a few other shells. We can also see changes in trust events as we kill and add processes.

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can build a binary with:
> ```bash
> cargo build --release
> ```
> You can run the bootstrap server on port 12345 with:
> ```bash
> ../../target/release/bootstrapping 12345
> ```
> Similarly, you can run a matching client on some free port with:
> ```bash
> ../../target/release/bootstrapping 12345 0
> ```
