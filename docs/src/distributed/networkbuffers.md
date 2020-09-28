# Network Buffers

Kompact uses a BufferPool system to serialize network messages. This section will describe the BufferPools briefly and how they can be configured with different parameters.

Before we begin describing the Network Buffers we remind the reader that there are two different methods for sending messages over the network in Kompact:   
1. **Lazy serialisation** `dst.tell(msg: M, from: S);`
2. **Eager serialisation** `dst.tell_serialised(msg: M, from: &self)?;`

With lazy serialisation the Actor moves the data to the heap, and transfers it unserialised to the `NetworkDispatcher`, which later serialises the message into its (the `NetworkDispatcher's`) own buffers.   
Eager serialisation serialises the data immediately into the Actors Buffers, and then transfers ownership of the serialised the data to the `NetworkDispatcher`.

## How the BufferPools work

### BufferPool locations
In a Kompact system where many actors use eager serialisation there will be many `BufferPools`. Ìf the actors in the system only use lazy serialisation there will be two pools, one used by the `NetworkDispatcher` for serialising outbound data, and one used by the `NetworkThread` for receiving incoming data.

### BufferPool, BufferChunk, ChunkLease
Each `BufferPool` (pool) consists of more than one `BufferChunks` (chunks). A chunk is the concrete memory area used for serialising data into. There may be many messages serialised into a single chunk, and discrete slices of the chunks (i.e. individual messages) can be extracted and sent to other threads/actors through the smart-pointer `ChunkLease` (lease). When a chunk runs out of space it will be locked and returned to the pool. If and only if all outstanding leases created from a chunk have been dropped may the chunk be unlocked and reused, or deallocated.

When a pool is created it will pre-allocate a configurable amount of chunks, and will attempt to reuse those as long as possible, and only when it needs to will it allocate more chunks, up to a configurable maximum number of chunks. 

**Note: In the current version, behaviour is unstable when a pool runs out of chunks and is unable to allocate more and will often cause a panic, similar to out-of-memory exception.** 

### BufferPool interface
Actors access their pool through the `EncodeBuffer` wrapper which maintains a single active chunk at a time, and automatically swaps the active buffer with the local `BufferPool` when necessary.  

The method `tell_serialised(msg, &self)?;` automatically uses the `EncodeBuffer` interface such that users of Kompact do not need to use the interfaces of the pool (why the method requires the `self` reference). Currently, a serialised message may not be greater than the size of the `BufferChunk`.  

### BufferPool initialization
Actors initialize their local buffers automatically when the first invocation of `tell_serialised(...)` occurs. If an Actor never invokes the method it will not allocate any buffers.  

An actor may call the initialization method through the call `self.ctx.borrow().init_buffers(None, None);`[^1] to explicitly initialize the local `BufferPool` without sending a message.

## BufferConfig 

### Parameters
There are four configurable parameters in the BufferConfig:

1. `chunk_size`: the size (in bytes) of the `BufferChunks`, and thereby dictating the maximum serialised message size. Default value is 128KB.
2. `initial_chunk_count`: how many `BufferChunks` the `BufferPool` will pre-allocate. Default value is 2.
3. `max_chunk_count`: the maximum number of `BufferChunks` the `BufferPool` may have allocated simultaneously. Default value is 1000.
4. `encode_buf_min_free_space`: When an Actor begins serialising a message the `EncodeBuffer` will compare how much space (in bytes) is left in the active chunk and compare it to this parameter, if there is less free space the active chunk will be replaced with a new one from the pool before continuing the serialisation. Default value is 64.

### Configuring the Buffers

#### Individual Actor Configuration
If no `BufferConfig` is specified Kompact will use the default settings for all `BufferPools`. Actors may be configured with individual `BufferConfigs` through the `init_buffer(Some(config), None)`[^1] config. It is important that the call is made before any calls to `tell_serialised(...)`. For example, the `on_start()` function of the `ComponentLifecycle` may be used to ensure this, as in the following example:

```rust,edition2018,no_run,noplaypen
impl ComponentLifecycle for CustomBufferConfigActor {
    fn on_start(&mut self) -> Handled {
        let mut buffer_config = BufferConfig::default();
        buffer_config.encode_buf_min_free_space(128);
        buffer_config.max_chunk_count(5);
        buffer_config.initial_pool_count(4);
        buffer_config.chunk_size(256000);
        
        self.ctx.borrow().init_buffers(Some(buffer_config), None);
        Handled::Ok
    }
    ...
}
```

#### Configuring All Actors
If a programmer wishes for all actors to use the same `BufferConfig` configuration, a Hocon string can be inserted into the `KompactConfig` or loaded from a Hocon-file ([see configuration chapter on loading configurations](./../local/configuration.md)), for example: 
```rust,edition2018,no_run,noplaypen
let mut cfg = KompactConfig::new();
cfg.load_config_str(
    r#"{
        buffer_config {
            chunk_size: 256000,
            initial_pool_count: 3,
            max_pool_count: 4,
            encode_min_remaining: 20,
            }
        }"#,
);
...
let system = cfg.build().expect("KompactSystem");
```
If a `BufferConfig` is loaded into the systems `KompactConfig` then all actors will use that configuration instead of the default `BufferConfig`, however individual actors may still override the configuration by using the `init_buffers(...)` method.

#### Configuring the NetworkDispatcher and NetworkThread
The `NetworkDispatcher` and `NetworkThread` are configured separately from the Actors and use their buffers for lazy serialisation and receiving data from the network. To configure their buffers the `NetworkConfig` may be created using the method `::with_buffer_config(...)` as in the example below:

```rust,edition2018,no_run,noplaypen
let mut cfg = KompactConfig::new();
let mut network_buffer_config = BufferConfig::default();
network_buffer_config.chunk_size(512);
network_buffer_config.initial_pool_count(2);
network_buffer_config.max_chunk_count(3);
network_buffer_config.encode_buf_min_free_space(10);
cfg.system_components(DeadletterBox::new, {
    NetworkConfig::with_buffer_config(
        "127.0.0.1:0".parse().expect("Address should work"),
        network_buffer_config,
    )
    .build()
});
let system = cfg.build().expect("KompactSystem");
```

#### BufferConfig Validation

`BufferConfig` implements the method `validate()` which causes a panic if any of the parameters are valid. It is invoked whenever a `BufferPool` is created from the given configuration. The validation checks the following conditions hold true:   
`chunk_size` > `encode_buf_min_free_space`   
`chunk_size` > 127   
`max_chunk_count` >= `initial_chunk_count`

- - - 
[^1]: The method `init_buffers(...)` takes two `Option` arguments, of which the second argument has not been covered. The second argument allows users of Kompact to specify a `CustomAllocator`, an untested experimental feature which is left undocumented for now. 