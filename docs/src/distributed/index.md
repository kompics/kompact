# Distributed Kompact

Each Kompact system can be configured to use a networking library, called a `Dispatcher`, to communicate with other remote systems. In order to send messages to remote components, a special kind of actor reference is needed: an `ActorPath`. This is different from an `ActorRef` in that it contains the necessary information to route a message to the target component, not a reference to a queue. The queue for this kind of message is the system wide `Dispatcher`, which is responsible for figuring out how to get the message to the target indicated in the `ActorPath`. The implementation provided in the `NetworkDispatcher` that ships with Kompact will automatically establish and maintain the needed network links for any target you are sending to.

## Actor Paths

In addition to serving as opaque handles to remote components, actor paths can also be treated as human-readable resource identifiers. Internally, they are divided into two major parts:

1. A `SystemPath`, which identifies the Kompact system we are trying to send to, and
2. and the actual actor path tail, which identifies the actor within the system.

Kompact provides two flavours of actor paths: 

1. A *Unique Path* identifies exactly one concrete instance of a component.
2. A *Named Path* can identify one or more instances of a component providing some service. The component(s) that a named path points to can be changed dynamically over time.

Examples of actor path string representations are:

- `tcp://127.0.0.1:63482#c6a799f0-77ff-4548-9726-744b90556ce7` (unique)
- `tcp://127.0.0.1:63482/my-service/instance1` (named)

### System Paths

A system path is essentially the same as you would address a server over a network. It specifies the transport protocol to use, the IP address, and the port. Different dispatchers are free to implement whichever set of transport protocols they wish to support. The provided `NetworkDispatcher` currently only offers TCP, in addition to the "fake" `local` protocol, which simply specifies an actor path within the same system via the dispatcher.

The `SystemPath` type specifies a system path alone, and can be acquired via `KompactSystem::system_path()`, for example. It doesn't have any function by itself, but can be used to build up a full actor path or for comparisons, for example.

### Unique Paths 

The `ActorPath::Unique` variant identifies a concrete instance of a component by its unique identifier, that is the same one you would get with `self.ctx.id()`, for example. So a unique path is really just a system path combined with a component id, which in the string representation is separated by a `#` character, e.g.: `tcp://127.0.0.1:63482#c6a799f0-77ff-4548-9726-744b90556ce7`

That means that once the target component is destroyed (due to a fault, for example) its unique actor path becomes invalid and can not be reassigned to even if a component of the same type is started to take its place. That makes unique paths relatively inflexible. However, the dispatcher is significantly faster at resolving unique paths compared to named paths, so they are still recommended for performance critical communication.

### Named Paths

The `ActorPath::Named` variant is more flexible than a unique path, in that it can be reassigned later. It also allows the specification of an actual path, that is a sequence of stringss, which could be hierarchical like in a filesystem. This opens up the possibilities for things like broadcast or routing semantics over path subtrees, for example.

In human-readable format a named path is represented by a system path followed by a sequence of strings beginning with and separated by forward slash (`/`) characters, just like a unix filesystem path would.

Multiple named paths can be registered to the same component.
