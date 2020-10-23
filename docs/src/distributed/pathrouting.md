# Path Routing

In the previous section on [Named Services](namedservices.md) we have seen that we can register components to named paths, such as `tcp://127.0.0.1:<port>/bootstrap`. These paths look very much like a [URL](https://en.wikipedia.org/wiki/URL), and indeed, just like in REST APIs, Kompact named paths form a tree-like hierarchy. For example `tcp://127.0.0.1:<port>/bootstrap/server1` would be a sub-path of `tcp://127.0.0.1:<port>/bootstrap`. This hierarchy is reflected in the way Kompact stores these actor aliases internally, which from a structure like a directory tree.

This approach to named paths opens up the possibility of exploiting the hierarchy for implicit and explicit *routing* of messages over sub-trees (directories, in a sense), which we explore in this section.

## Routing Policies

In general, a routing policy is something that takes a message and set of references and selects one or more references that the message will be sent to. In the concrete case of routing within the named path tree, the type of the message must be `NetMessage` and the references are `DynActorRef`. The  set of references we give to a policy is going to be the set of all registered nodes under a particular prefix in the named actor tree, which we will call the *routing path*. 

> **Example:** If `tcp://127.0.0.1:<port>/bootstrap` is a routing path with some policy P, then whenever we send something to it, we will pass the set containing the actor ref registered at `tcp://127.0.0.1:<port>/bootstrap/server1` to P. If there were another registration at `tcp://127.0.0.1:<port>/bootstrap/servers/server1` we would add that to the set as well.

### Types of Routing Paths

Kompact supports two different types of routing paths: **explicit** paths and **implicit** paths.

In order to explain this in the following paragraphs, consider a system where the following three actors are registered:

1. `tcp::127.0.0.1:1234/parent/child1`
2. `tcp::127.0.0.1:1234/parent/child2`
3. `tcp::127.0.0.1:1234/parent/child1/grandchild`

#### Implicit Routing

Routing in Kompact can be used without any (routing specific) setup at all. If we simply construct an `ActorPath` of the form `tcp::127.0.0.1:1234/parent/*` and send a message there, Kompact will automatically broadcast this message to all three nodes registered above, since all of them have `tcp::127.0.0.1:1234/parent` as their prefix. This kind of implicit routing path is called a **broadcast path**. The other type of implicit routing supported by Kompact is called a **select path** and takes the form `tcp::127.0.0.1:1234/parent/?`. Sending a message to this select path will cause the message to be sent to exactly one of the actors. Which node exactly is subject to the routing policy at `tcp::127.0.0.1:1234/parent`, which is not guaranteed to be stable by the runtime. The current default policy for select is based on hash buckets over the messages sender field.

> **Warning:** In certain deployments allowing implicit routing can become a security risk with respect to [DoS attacks](https://en.wikipedia.org/wiki/Denial-of-service_attack), since an attacker can basically force the system to broadcast a message to every registered node, which can cause a lot unnecessary load.
> 
> If this is a concern for your deployment scenario, you can compile Kompact without default features, which will remove implicit routing completely.

#### Explicit Routing

If implicit routing is not a good match for your use case, Kompact allows you explicitly set a policy at a particular point in the named tree via the `KompactSystem::set_routing_policy(...)` method. Not only does this allow you to customise the behaviour of routing for a particular sub-tree, it also enables you to hide the fact that a tree is routing at all, as with an explicit policy both `tcp::127.0.0.1:1234/parent` (where the routing policy is set) and one of `tcp::127.0.0.1:1234/parent/*` and `tcp::127.0.0.1:1234/parent/?` (depending on whether your police is of broadcast or select type) will exhibit the same behaviour.

Explicit routing works even if implicit routing is disabled.

### Provided Policies

Kompact comes with three routing policies built in:

1. `kompact::routing::groups::BroadcastRouting` is the default policy for broadcast paths. As the name implies, it will simply send a copy of each message to every member of the routing set. In order to improve the efficiency of broadcasting, you may want to override the default implementation of `Serialisable::cloned()` for the types you are broadcasting, at least when you know that local delivery can happen.
2. `kompact::routing::groups::SenderDefaultHashBucketRouting` is the default policy for select paths. It will use the hash of the messages sender field to determine a member to send the message to. Changing the member set in any way will thus also change the assignments. `SenderDefaultHashBucketRouting` is actually just a type alias for a more customisable hash-based routing policy called `kompact::routing::groups::FieldHashBucketRouting`, which lets you decide the field(s) to use for hashing and the actual hashing algorithm.
3. `kompact::routing::groups::RoundRobinRouting` uses a mutable index (an `AtomicUsize` to be exact) to select exactly one member in a round-robin manner.

### Custom Policies

In addition to the already provided routing policies, users can easily implement their own by implementing `RoutingPolicy<DynActorRef, NetMessage>` for their custom type. It is important to note that policy lookups happen concurrently in the store and hence routing must be implemented with a `&self` reference instead of `&mut self`. Thus, routing protocols that must update manage state for each message must rely on atomics or—if really necessary—on mutexes or similar concurrent structures as appropriate for their access pattern.

## Example

To show-case the path routing feature of Kompact, we will sketch a simple client-server application, where the server holds a "database" (just a large slice of strings in our case) and the client sends "queries" against this database. The queries are simply going to be shorter strings, which we will try to find as substrings in the database and return all matching strings. Since our database is actually immutable, we will share it among multiple server components and use **select routing** with the round-robin policy to spread out the load. Since the queries are expensive, we will also cache the results on the clients. To provide an example of broadcast routing we will cache the responses for *any* client at *every* client via broadcast. For simplicity, this example is going to be completely local within a single Kompact system, but the mechanisms involved are really designed for remote use primarily, with local paths only an optimisation normally.

### Messages

We only have two messages, the `Query` with a unique request id and the actual pattern we want to match against, and the `QueryResponse` which has all the fields of the `Query` plus a vector of strings that matched the pattern. For convenience, we will use `Serde` as serialisation mechanism again.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/load_balancer.rs:messages}}
```

### State and Behaviour

As for this example the exact implementation of the servers and clients is not really crucial, we won't describe it in detail here. The important things to note are that the `Client` uses the path `server_path` field to send requests, which we will initialise later with a select path of the form `tcp://127.0.0.1:<port>/server/?`. It also replaces its unique response path with a `broadcast_path`, which we will initialise later with a broadcast path of the form `tcp://127.0.0.1:<port>/client/*`.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/load_balancer.rs:client_send}}
```

### System Setup

When setting up the Kompact system in the main, we will use the following constants, which essentially represent configuration of our scenario:


```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/load_balancer.rs:constants}}
```

First of all we set up the routing policies and their associated paths. In order to show off both variants, we will use implicit routing for the client broadcast path and explicit routing for the server select path. As mentioned before, implicit routing does not really require any specific setup. We simply construct the appropriate path, which in this case is going to be our system path followed by `client/*`. For the server load-balancing, we want to use the round-robin policy, which we will register under the `server` alias using `KompactSystem::set_routing_policy(...)`. Like a normal actor registration, this call returns a future with the actual path for this policy. Since the policy is set explicitly, this path will actually be of the form `tcp://127.0.0.1:<port>/server`, but sending a message to `tcp://127.0.0.1:<port>/server/?` would behave in the same manner.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/load_balancer.rs:policies_setup}}
```

We will then create and register both the servers and the clients, making sure to register either with a unique name (based on their index) under the correct path prefix.

#### Servers

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/load_balancer.rs:server_setup}}
```

#### Clients

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/load_balancer.rs:client_setup}}
```

### Running

Finally, we simply start the servers and the clients, then run them for a few seconds, and shut them down again, before shutting down the system itself.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/load_balancer.rs:running}}
```

If we inspect the output in release mode, we can see that both clients and servers print some final statistics about their run. In particular the results of the servers show that the requests were very well balanced, thanks to our round-robin policy:

```
Oct 23 18:15:58.869 INFO Shutting down a Client that ran 1060 requests with 6 cache hits (0.005660377358490566%), ctype: Client, cid: 07739284-1171-43c7-b547-198f9adf31e2, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:152
Oct 23 18:15:58.869 INFO Shutting down a Client that ran 1055 requests with 7 cache hits (0.006635071090047393%), ctype: Client, cid: 7a33e17c-042f-4271-95ea-a725ee471dae, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:152
Oct 23 18:15:58.869 INFO Shutting down a Client that ran 1052 requests with 4 cache hits (0.0038022813688212928%), ctype: Client, cid: 9b3c3c57-8246-4456-a7b8-0d200086df8d, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:152
Oct 23 18:15:58.869 INFO Shutting down a Client that ran 1050 requests with 3 cache hits (0.002857142857142857%), ctype: Client, cid: 1ecdef68-43af-46b4-8a40-a8ad4147b811, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:152
Oct 23 18:15:58.869 INFO Shutting down a Client that ran 1051 requests with 5 cache hits (0.004757373929590866%), ctype: Client, cid: 034f5dcc-a0ba-4bc2-aca0-6f1ab12be139, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:152
Oct 23 18:15:58.870 INFO Shutting down a Client that ran 1047 requests with 2 cache hits (0.0019102196752626551%), ctype: Client, cid: 59679577-6e9a-44ef-9739-08ca1b32b03f, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:152
Oct 23 18:15:58.870 INFO Shutting down a Client that ran 1048 requests with 3 cache hits (0.0028625954198473282%), ctype: Client, cid: ef76ddd0-e240-4ad6-8a10-b98da9ba41ff, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:152
Oct 23 18:15:58.870 INFO Shutting down a Client that ran 1044 requests with 0 cache hits (0%), ctype: Client, cid: ddf7d77a-4987-4411-81a5-bc4841200c32, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:152
Oct 23 18:15:58.870 INFO Shutting down a Client that ran 1051 requests with 7 cache hits (0.006660323501427212%), ctype: Client, cid: 12b65a83-c443-4853-8337-47ba5c45f60d, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:152
Oct 23 18:15:58.871 INFO Shutting down a Client that ran 1046 requests with 3 cache hits (0.0028680688336520078%), ctype: Client, cid: c7978b3f-9cf2-44d2-b93f-fc32ad90c941, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:152
Oct 23 18:15:58.872 INFO Shutting down a Client that ran 1049 requests with 6 cache hits (0.005719733079122974%), ctype: Client, cid: af389f4d-bc93-4f37-8f50-a70e054651e0, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:152
Oct 23 18:15:58.872 INFO Shutting down a Client that ran 1047 requests with 4 cache hits (0.0038204393505253103%), ctype: Client, cid: ad20509a-dbab-4dd3-a497-99a8488101b3, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:152
Oct 23 18:15:58.873 INFO Shutting down a Server that handled 4183 requests, ctype: QueryServer, cid: 35309404-a989-4b18-848f-5cc719b19a76, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:56
Oct 23 18:15:58.873 INFO Shutting down a Server that handled 4184 requests, ctype: QueryServer, cid: 2a2ed2cb-36bb-4df0-ac0e-0204e12417bd, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:56
Oct 23 18:15:58.873 INFO Shutting down a Server that handled 4183 requests, ctype: QueryServer, cid: a3d6d94a-ff9c-4749-9b6a-db2bfa2ac3e2, system: kompact-runtime-1, location: docs/examples/src/bin/load_balancer.rs:56
```

> **Note:** As before, if you have checked out the [examples folder](https://github.com/kompics/kompact/tree/master/docs/examples) you can run the concrete binary with:
> ```bash
> cargo run --release --bin load_balancer
> ```
> Note that running in debug mode will produce a lot of output as it will trace all the network messages.
