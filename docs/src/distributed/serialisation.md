# Serialisation

In this section we are going take a closer look at the various serialisation options offered by Kompact. In particular, we will look at how to write different kinds of custom serialiser implementations, as well how to handle messages that are all but guaranteed to actually go over the network more efficiently.

## Custom Serialisation

At the centre of Kompact's serialisation mechanisms are the `Serialisable` and `Deserialiser` traits, the signature of which looks roughtly like this:

```rust,edition2018,no_run,noplaypen
pub trait Serialisable: Send + Debug {
    /// The serialisation id for this serialisable
    fn ser_id(&self) -> SerId;

    /// An indicator how many bytes must be reserved in a buffer for a value to be
    /// serialsed into it with this serialiser
    fn size_hint(&self) -> Option<usize>;

    /// Serialises this object (`self`) into `buf`
    fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError>;

    /// Try move this object onto the heap for reflection, instead of serialising
    fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>>;
}
```

```rust,edition2018,no_run,noplaypen
pub trait Deserialiser<T>: Send {
    /// The serialisation id for which this deserialiser is to be invoked
    const SER_ID: SerId;

    /// Try to deserialise a `T` from the given `buf`
    fn deserialise(buf: &mut dyn Buf) -> Result<T, SerError>;
}
```

### Outgoing Path

When `ActorPath::tell(...)` is invoked with a type that is `Serialisable`, it will create a boxed trait object from the given instance and send it to the network layer. Only when the network layer has the determind that the destination must be accessed via a network channel, will the runtime serialise the instance into the network channel's buffer. If it turns out the destination is on the same actor system as the source, it will simply call `Serialisable::local(...)` to get a boxed instance of the `Any` trait and then send it directly to the target component, without ever serialising. This approach is called **lazy serialisation**. For the vast majority of `Serialisable` implementations, `Serialisable::local(...)` is implemented simply as `Ok(self)`. However, for some more advanced usages (e.g., serialisation proxies) the implementation may have to call some additional code.

Once it is determined that an instance does indeed need to be serialised, the runtime will reserve some buffer memory for it to be serialised into. It does so by querying the `Serialisable::size_hint(...)` function for an estimate of how much space the type is likely going to take. For some types this is easy to know statically, but others it is not so clear. In any case, this is just an optimisation. Serialisation will proceed correctly even if the estimate is terribly wrong or no estimate is given at all.

The first thing in the new serialisation buffer is typically the serialisation id obtained via `Serialisable::ser_id(...)`. Typically, Kompact will only require a single serialisation id for the message to be written into the buffer, even if the message uses other serialisers internally, as long as all the internal types are statically known. This top-level serialisation id must match the `Deserialiser::SER_ID` for the deserialiser to be used for this instance. For types that implement both `Serialisable` and `Deserialiser`, as most do, it is recommended to simply use is `Self::SER_ID` as the implementation for `Serialisable::ser_id(...)` to make sure the ids match later.

The actual serialisation of the instance is handled by `Serialisable::serialise(...)`, which should use the functions provided by [BufMut](https://docs.rs/bytes/latest/bytes/trait.BufMut.html) to serialise the individual parts of the instance into the buffer.

#### Serialiser

Instead of implementing `Serialisable` we can also implement the `Serialiser` trait:

```rust,edition2018,no_run,noplaypen
pub trait Serialiser<T>: Send {
    /// The serialisation id for this serialiser
    fn ser_id(&self) -> SerId;

    /// An indicator how many bytes must be reserved in a buffer for a value to be
    fn size_hint(&self) -> Option<usize>;

    /// Serialise `v` into `buf`.
    fn serialise(&self, v: &T, buf: &mut dyn BufMut) -> Result<(), SerError>;
}
```

This behaves essentually the same, except that it doesn't serialise itself, but an instance of another type `T`. In order to use an instance `t: T` with a `Serialiser<T>` we can simply pass a pair of the two to the `ActorPath::tell(...)` function, as we have already seen in the previous section, for example with `Serde`:

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/serialisation.rs:send_heartbeats}}
```

### Incoming Path

For any incoming network message the Kompact framework will buffer all data, and once it is complete, it will read out the serialisation id and create a `NetMessage` from it and the remaining buffer. It will then send the `NetMessage` directly to the destination component without any further processing. This approach is called **lazy deserialisation** and is quite different from most other actor/component frameworks, which tend to deserialise eagerly and then type match later at the destination component. However, in Rust the lazy approach is more efficient as it avoids unnecessary heap allocations for the deserialised instance.

When the `NetMessage::try_deserialise` function is called on the destination component, the serialisation ids of the message and the given `Deserialiser` will be checked and if they match up the `Deserialiser::deserialise(...)` function is called with the message's data. For custom deserialisers, this method must use the [Buf](https://docs.rs/bytes/latest/bytes/trait.Buf.html) API to implement essentially the inverse path of what the serialisable did before.

### Example

To show how custom serialisers can be implemented, we will show two examples re-using the bootstrapping leader election from the previous sections.

#### Serialiser

In our example, `CheckIn` is a *zero-sized type* (ZST), since we don't really care about the message, only about the sender. Since ZSTs have no content, we can uniquely identify them by their serialisation id alone and all the serialisers for them are basically identical, in that their `serialise(...)` function consists simply of `Ok(())`. For this example, instead of using `Serde` for `CheckIn`, we will write our own `Serialiser` implementation for ZSTs and then use it for `CheckIn`. We could also use it for `Heartbeat`, but we won't, so as to leave it as a reference for the other approach.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/serialisation.rs:zstser}}
```

We continue using the `SerialisationId` trait like we did for Serde, because we need to write id of the ZST not of the `ZstSerialiser`, which can serialise and deserialise many different ZSTs.

In order to create the correct type instance during deserialisation, we use the `Default` trait, which can be trivially derived for ZSTs.

It is clear that this serialiser is basically trivial. We can use it by creating a pair of `Checkin` with a reference to our static instance `CHECK_IN_SER`, which simply specialises the `ZstSerialiser` for `CheckIn`, as we did before: 

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/serialisation.rs:checkin}}
```

#### Serialisable

Since the previous example was somewhat trivial, we will do a slightly trickier one for the `Serialisable` example. We will make the `UpdateProcesses` type both `Serialisable` and `Deserialiser<UpdateProcesses>`. This type contains a vector of `ActorPath` instances, which we must handle correctly. We will reuse the `Serialisable` and `Deserialiser<ActorPath>` implementations that are already provided for the `ActorPath` type.

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/serialisation.rs:serialisable}}
```

It would be easy to just iterate through the vector during serialisation and write one path at a time using its own `serialise(...)` implementation. But during deserialisation we need to know how many paths we have to take out of the buffer. We could simply try taking until the buffer refuses us, but this kind of approach often makes it difficult to detect bugs in one's serialiser implementations. We will instead write the length of the vector before we serialise the actor paths, and during deserialisation we will read it first and allocate a vector of appropriate size. If we are concerned about the space the length wastes, we could try to use some better integer encoding like Protocol Buffers do, for example. But for now we don't care so much and simply write a full `u64`. Those extra 8 bytes make little different compared to the sizes of a bunch of actor paths.

We don't really have a good idea for `size_hint(...)` here. It's basically 8 plus the sum of the size hints for each actor path. In this case, we actually know we are pretty much just going to send unique actor paths in this set, so we can assume each one is 23 bytes long. If that assumption turns out to be wrong in practice, it will simply cause some additional allocations during serialisation. In general, as a developer we have to decide on a trade-off between how much time we want to spend calculating accurate size hints, and how much time we want to spend on potential reallocations. We could also simply return a large number such as 1024 and accept that we may may often waste much of that allocated space. Application requirements (read: benchmarking) will determine which is the best choice in a particular scenario.

## Eager Serialisation

As mentioned above, using `ActorPath::tell(...)` may cause a stack-to-heap move of the data, as it is being converted into a boxed trait object for lazy serialisation. This approach optimises for avoiding expensive serialisations in the case where the `ActorPath` turns out to be local. However, this may not always be the appropriate approach, in particular if serialisation is quick compared to allocation, or most actor paths are not going to be local anyway. For these cases, Kompact also allows **eager serialisation**. To force an instance to be serialised eagerly, on the sending component's thread, you can use `ActorPath::tell_serialised(...)`. It works essentially the same as `ActorPath::tell(...)` but uses a buffer pool local to the sending component to serialise the data into, before sending it off to the dispatcher. If then in the dispatcher it turns out that the actor path was actually local, the data simply has to be deserialised again, as if it had arrived remotely. If the target is remote, however, the data can be written directly into the appropriate channel.

### Example

To show an easy usage for this approach, we use eager serialisation in the `BootstrapServer::broadcast_processess` function: 

```rust,edition2018,no_run,noplaypen
{{#rustdoc_include ../../examples/src/bin/serialisation.rs:tell_serialised}}
```

As you can see above, another feature of eager serialisation is that you can (and must) deal with serialisaition errors, which you have no control over using lazy serialisation. In particular, your memory allocation may prevent your local buffer pool from allocating a buffer large enough to fit your data at the time of serialisation. In this case you will get a `SerError::BufferError` and must decide how to handle that. You could either retry at a later time, or switch to *lazy serialisation* and hope the network's buffers still have capacity (assuming they likely have priority over component local buffer pools).
