use super::*;
use crate::{messaging::DispatchEnvelope, utils::checked_casts::*};
use bytes::{Buf, BufMut};
use core::{cmp, fmt, ptr};
use hocon::{Hocon, HoconLoader};
use std::{
    convert::TryFrom,
    fmt::{Debug, Formatter},
    sync::Arc,
};

pub(crate) mod buffer_pool;
pub mod chunk_lease;
pub mod chunk_ref;
pub(crate) mod decode_buffer;
pub(crate) mod encode_buffer;

pub(crate) use self::buffer_pool::*;
pub use self::{chunk_lease::*, chunk_ref::*, decode_buffer::*, encode_buffer::*};
use std::path::PathBuf;

/// The configuration for the network buffers
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BufferConfig {
    /// Specifies the size in bytes of `BufferChunk`s
    pub(crate) chunk_size: usize,
    /// Specifies the number of `BufferChunk` a `BufferPool` will pre-allocate on creation.
    pub(crate) initial_chunk_count: usize,
    /// Specifies the max number of `BufferChunk`s each `BufferPool` may have.
    pub(crate) max_chunk_count: usize,
    /// Minimum number of bytes an `EncodeBuffer` must have available before serialisation into it.
    pub(crate) encode_buf_min_free_space: usize,
}

impl BufferConfig {
    /// Tries to deserialise a configuration instance from the specified in the given `config`.
    ///
    /// Returns the same values as [BufferConfig::default](BufferConfig::default)
    /// if it fails to read from the config.
    pub fn from_config(config: &Hocon) -> Self {
        let mut buffer_config = BufferConfig::default();
        if let Some(chunk_size) = config["buffer_config"]["chunk_size"].as_bytes() {
            buffer_config.chunk_size = chunk_size
                .ceil_checked_cast()
                .expect("Invalid byte number for chunk_size");
        }
        if let Some(initial_chunk_count) = config["buffer_config"]["initial_chunk_count"].as_i64() {
            buffer_config.initial_chunk_count =
                usize::try_from(initial_chunk_count).expect("Invalid initial_chunk_count");
        }
        if let Some(max_chunk_count) = config["buffer_config"]["max_chunk_count"].as_i64() {
            buffer_config.max_chunk_count =
                usize::try_from(max_chunk_count).expect("Invalid max_chunk_count")
        }
        if let Some(encode_min_remaining) =
            config["buffer_config"]["encode_min_remaining"].as_bytes()
        {
            buffer_config.encode_buf_min_free_space = encode_min_remaining
                .ceil_checked_cast()
                .expect("Invalid byte number for encode_min_remaining");
        }
        buffer_config.validate();
        buffer_config
    }

    /// Tries to deserialise a `BufferConfig` from a HOCON file at `path`
    ///
    /// Returns the same values as [BufferConfig::default](BufferConfig::default)
    /// if it fails to read from the config.
    ///
    /// # Panics
    ///
    /// Panics if it fails to load the file or fails to load the file as HOCON.
    pub fn from_config_file<P>(path: P) -> BufferConfig
    where
        P: Into<PathBuf>,
    {
        let p: PathBuf = path.into();
        let doc = HoconLoader::new()
            .load_file(p)
            .expect("Failed to load file")
            .hocon()
            .expect("Failed to load as HOCON");
        Self::from_config(&doc)
    }

    /// Sets the `chunk_size` to the given number of bytes.
    ///
    /// Specifies the size in bytes of each [BufferChunk](BufferChunk)
    /// instance produced by pools using this configuration.
    ///
    /// Must be at least 128 bytes, and must also be greater than [encode_buf_min_free_space](BufferConfig::encode_buf_min_free_space).
    pub fn chunk_size(&mut self, size: usize) -> () {
        self.chunk_size = size;
    }

    /// Sets the `initial_chunk_count` to the given number.
    ///
    /// Specifies the number of [BufferChunk](BufferChunk)
    /// instances that pools using this configuration create
    /// initially.
    ///
    /// May be at most the value of [max_chunk_count](BufferConfig::max_chunk_count).
    pub fn initial_chunk_count(&mut self, count: usize) -> () {
        self.initial_chunk_count = count;
    }

    /// Sets the `max_chunk_count` to the given number.
    ///
    /// Specifies the maximum number of [BufferChunk](BufferChunk)
    /// instances that pools using this configuration may create.
    ///
    /// Must be at least the value of [initial_chunk_count](BufferConfig::initial_chunk_count).
    pub fn max_chunk_count(&mut self, count: usize) -> () {
        self.max_chunk_count = count;
    }

    /// Sets the `encode_buf_min_free_space` to the given number of bytes.
    ///
    /// This sets the minimum number of bytes an [EncodeBuffer](EncodeBuffer)
    /// must have available before attempting any serialisation into it.
    ///
    /// Must be less than [chunk_size](BufferConfig::chunk_size).
    pub fn encode_buf_min_free_space(&mut self, size: usize) -> () {
        self.encode_buf_min_free_space = size;
    }

    /// Performs basic sanity checks on the config parameters and panics if it is invalid.
    ///
    /// This method is called automatically by any `BufferPool` on creation.
    pub fn validate(&self) {
        if self.initial_chunk_count > self.max_chunk_count {
            panic!("initial_chunk_count may not be greater than max_chunk_count")
        }
        if self.chunk_size <= self.encode_buf_min_free_space {
            panic!("chunk_size must be greater than encode_min_remaining")
        }
        if self.chunk_size < 128 {
            panic!("chunk_size smaller than 128 is not allowed")
        }
        if self.max_chunk_count < 2 {
            panic!("max_chunk_count must be greater than 2")
        }
        if self.max_chunk_count < 2 {
            panic!("max_chunk_count must be greater than 2")
        }
    }
}

impl Default for BufferConfig {
    /// Create a new configuration with default values which may then be overwritten
    ///
    /// - [chunk_size](BufferConfig::chunk_size) default value is `128 000`.
    /// - [initial_chunk_count](BufferConfig::initial_chunk_count) default value is `2`.
    /// - [max_chunk_count](BufferConfig::max_chunk_count) default value is `1 000`.
    /// - [encode_buf_min_free_space](BufferConfig::encode_buf_min_free_space) is `64`.
    fn default() -> Self {
        BufferConfig {
            chunk_size: 128 * 1000,        // 128KB chunks
            initial_chunk_count: 2,        // 256KB initial/minimum BufferPools
            max_chunk_count: 1000,         // 128MB maximum BufferPools
            encode_buf_min_free_space: 64, // typical L1 cache line size
        }
    }
}

/// The `Chunk` trait is used by [BufferChunk] and allows creation of special [ChunkAllocator]'s
pub trait Chunk {
    /// Returns a mutable pointer to the underlying buffer
    fn as_mut_ptr(&mut self) -> *mut u8;
    /// Returns the length of the chunk
    fn len(&self) -> usize;
    /// Returns `true` if the length of the chunk is 0
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A Default Kompact Chunk
pub(crate) struct DefaultChunk {
    chunk: Box<[u8]>,
}

impl DefaultChunk {
    pub fn new(size: usize) -> DefaultChunk {
        let v = vec![0u8; size];
        let slice = v.into_boxed_slice();
        DefaultChunk { chunk: slice }
    }
}

impl Chunk for DefaultChunk {
    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.chunk.as_mut_ptr()
    }

    fn len(&self) -> usize {
        self.chunk.len()
    }

    /// Returns `true` if the length of the chunk is 0
    fn is_empty(&self) -> bool {
        self.chunk.is_empty()
    }
}

/// BufferChunk is a lockable pinned byte-slice
/// All modifications to the Chunk goes through the get_slice method
pub struct BufferChunk {
    chunk: *mut dyn Chunk,
    ref_count: Arc<()>,
    locked: bool,
}

impl Debug for BufferChunk {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferChunk")
            .field("Lock", &self.locked)
            .field("Length", &self.len())
            .field("RefCount", &Arc::strong_count(&self.ref_count))
            .finish()
    }
}

impl Drop for BufferChunk {
    fn drop(&mut self) {
        unsafe {
            let chunk = Box::from_raw(self.chunk);
            std::mem::drop(chunk);
        }
    }
}

unsafe impl Send for BufferChunk {}

impl BufferChunk {
    /// Allocate a new Default BufferChunk
    pub fn new(size: usize) -> Self {
        BufferChunk {
            chunk: Box::into_raw(Box::new(DefaultChunk::new(size))),
            ref_count: Arc::new(()),
            locked: false,
        }
    }

    /// Creates a BufferChunk using the given Chunk
    pub fn from_chunk(raw_chunk: *mut dyn Chunk) -> Self {
        BufferChunk {
            chunk: raw_chunk,
            ref_count: Arc::new(()),
            locked: false,
        }
    }

    /// Get the length of the BufferChunk
    pub fn len(&self) -> usize {
        unsafe { Chunk::len(&*self.chunk) }
    }

    pub fn is_empty(&self) -> bool {
        unsafe { Chunk::is_empty(&*self.chunk) }
    }

    /// Return a pointer to a subslice of the chunk
    unsafe fn get_slice(&mut self, from: usize, to: usize) -> &'static mut [u8] {
        assert!(from < to && to <= self.len() && !self.locked);
        let ptr = (&mut *self.chunk).as_mut_ptr();
        let offset_ptr = ptr.add(from);
        std::slice::from_raw_parts_mut(offset_ptr, to - from)
    }

    /// Swaps self with other in-place
    pub fn swap_buffer(&mut self, other: &mut BufferChunk) -> () {
        std::mem::swap(self, other);
    }

    /// Returns a ChunkLease pointing to the subslice between `from` and `to`of the BufferChunk
    /// the returned lease is a consumable Buf.
    pub fn get_lease(&mut self, from: usize, to: usize) -> ChunkLease {
        let lock = self.get_lock();
        unsafe { ChunkLease::new(self.get_slice(from, to), lock) }
    }

    /// Clones the lock, the BufferChunk will be locked until all given locks are deallocated
    pub fn get_lock(&self) -> Arc<()> {
        self.ref_count.clone()
    }

    /// Returns true if this BufferChunk is available again
    pub fn free(&mut self) -> bool {
        if self.locked {
            if Arc::strong_count(&self.ref_count) < 2 {
                self.locked = false;
                true
            } else {
                false
            }
        } else {
            true
        }
    }

    /// Locks the BufferChunk, it will not be writable nor will any leases be created before it has been unlocked.
    pub fn lock(&mut self) -> () {
        self.locked = true;
    }
}

/// Methods required by a ChunkAllocator
pub trait ChunkAllocator: Send + Sync + Debug + 'static {
    /// ChunkAllocators deliver Chunk by raw pointers
    fn get_chunk(&self) -> *mut dyn Chunk;
    /// This method tells the allocator that the [Chunk](Chunk) may be deallocated
    ///
    /// # Safety
    ///
    /// The caller *must* guarantee that the memory pointed to by `ptr`
    /// has *not* been deallocated, yet!
    unsafe fn release(&self, ptr: *mut dyn Chunk) -> ();
}

/// A default allocator for Kompact
///
/// Heap allocates chunks through the normal Rust system allocator
#[derive(Default, Debug)]
pub(crate) struct DefaultAllocator {
    chunk_size: usize,
}

impl ChunkAllocator for DefaultAllocator {
    fn get_chunk(&self) -> *mut dyn Chunk {
        Box::into_raw(Box::new(DefaultChunk::new(self.chunk_size)))
    }

    unsafe fn release(&self, ptr: *mut dyn Chunk) -> () {
        Box::from_raw(ptr);
    }
}

/// Errors that can be thrown during serialisation or deserialisation
#[derive(Debug)]
pub enum BufferError {
    NoAvailableBuffers(String),
}

impl std::error::Error for BufferError {}

impl fmt::Display for BufferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BufferError::NoAvailableBuffers(s) => {
                write!(f, "A BufferPool ran out of Buffers \n {}", s)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use hocon::HoconLoader;
    use std::time::Duration;
    const START_ACTOR_TIMEOUT: Duration = Duration::from_millis(1000);

    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
    #[should_panic(expected = "initial_chunk_count may not be greater than max_chunk_count")]
    fn invalid_pool_counts_config_validation() {
        let hocon = HoconLoader::new()
            .load_str(
                r#"{
            buffer_config {
                chunk_size: 64,
                initial_chunk_count: 3,
                max_chunk_count: 2,
                encode_min_remaining: 2,
                }
            }"#,
            )
            .unwrap()
            .hocon();
        // This should succeed
        let cfg = hocon.unwrap();

        // Validation should panic
        let _ = BufferConfig::from_config(&cfg);
    }

    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
    #[should_panic(expected = "chunk_size must be greater than encode_min_remaining")]
    fn invalid_encode_min_remaining_validation() {
        // The BufferConfig should panic because encode_min_remain too high
        let mut buffer_config = BufferConfig::default();
        buffer_config.chunk_size(128);
        buffer_config.encode_buf_min_free_space(128);
        buffer_config.validate();
    }
    #[derive(ComponentDefinition)]
    struct BufferTestActor {
        ctx: ComponentContext<BufferTestActor>,
        custom_buf: bool,
        started_promise: Option<KPromise<()>>,
    }
    impl BufferTestActor {
        fn with_custom_buffer() -> BufferTestActor {
            BufferTestActor {
                ctx: ComponentContext::uninitialised(),
                custom_buf: true,
                started_promise: None,
            }
        }

        fn without_custom_buffer() -> BufferTestActor {
            BufferTestActor {
                ctx: ComponentContext::uninitialised(),
                custom_buf: false,
                started_promise: None,
            }
        }

        fn get_started_future(&mut self) -> KFuture<()> {
            let (promise, future) = promise();
            self.started_promise = Some(promise);
            future
        }
    }
    impl Actor for BufferTestActor {
        type Message = ();

        fn receive_local(&mut self, _: Self::Message) -> Handled {
            Handled::Ok
        }

        fn receive_network(&mut self, _: NetMessage) -> Handled {
            Handled::Ok
        }
    }
    impl ComponentLifecycle for BufferTestActor {
        fn on_start(&mut self) -> Handled {
            if self.custom_buf {
                let mut buffer_config = BufferConfig::default();
                buffer_config.encode_buf_min_free_space(30);
                buffer_config.max_chunk_count(5);
                buffer_config.initial_chunk_count(4);
                buffer_config.chunk_size(128);
                // Initialize the buffers
                self.ctx.init_buffers(Some(buffer_config), None);
            }
            // Use the Buffer
            let _ = self.ctx.actor_path().tell_serialised(120, self);
            if let Some(promise) = self.started_promise.take() {
                promise.complete().expect("Failed to fulfil promise");
            }
            Handled::Ok
        }

        fn on_stop(&mut self) -> Handled {
            Handled::Ok
        }

        fn on_kill(&mut self) -> Handled {
            Handled::Ok
        }
    }
    fn buffer_config_testing_system() -> KompactSystem {
        let mut cfg = KompactConfig::default();
        let mut network_buffer_config = BufferConfig::default();
        network_buffer_config.chunk_size(512);
        network_buffer_config.initial_chunk_count(2);
        network_buffer_config.max_chunk_count(3);
        network_buffer_config.encode_buf_min_free_space(10);
        cfg.load_config_str(
            r#"{
                buffer_config {
                    chunk_size: "256B",
                    initial_chunk_count: 3,
                    max_chunk_count: 4,
                    encode_min_remaining: "20B",
                }
                }"#,
        );
        cfg.system_components(DeadletterBox::new, {
            NetworkConfig::with_buffer_config(
                "127.0.0.1:0".parse().expect("Address should work"),
                network_buffer_config,
            )
            .build()
        });
        cfg.build().expect("KompactSystem")
    }
    // This integration test sets up a KompactSystem with a Hocon BufferConfig,
    // then runs init_buffers on an Actor with different settings (in the on_start method of dummy),
    // and finally asserts that the actors buffers were set up using the on_start parameters.
    #[test]
    fn buffer_config_init_buffers_overrides_hocon_and_default() {
        let system = buffer_config_testing_system();
        let mut dummy_actor = BufferTestActor::with_custom_buffer();
        let future = dummy_actor.get_started_future();
        let (dummy, _df) = system.create_and_register(|| dummy_actor);
        let dummy_f = system.register_by_alias(&dummy, "dummy");
        let _ = dummy_f.wait_expect(START_ACTOR_TIMEOUT, "dummy failed");
        system.start(&dummy);

        future
            .wait_timeout(START_ACTOR_TIMEOUT)
            .expect("actor never started");

        // Read the buffer_len
        let mut buffer_len = 0;
        let mut buffer_write_pointer = 0;
        dummy.on_definition(|c| {
            if let Some(encode_buffer) = c.ctx.get_buffer_location().borrow().as_ref() {
                buffer_len = encode_buffer.len();
                buffer_write_pointer = encode_buffer.get_write_offset();
            }
        });
        // Assert that the buffer was initialized with parameter in the actors on_start method
        assert_eq!(buffer_len, 128);
        // Check that the buffer was used
        assert_ne!(buffer_write_pointer, 0);
    }

    #[test]
    fn buffer_config_hocon_overrides_default() {
        let system = buffer_config_testing_system();
        let mut dummy_actor = BufferTestActor::without_custom_buffer();
        let future = dummy_actor.get_started_future();
        let (dummy, _df) = system.create_and_register(|| dummy_actor);
        let dummy_f = system.register_by_alias(&dummy, "dummy");
        let _ = dummy_f.wait_expect(START_ACTOR_TIMEOUT, "dummy failed");
        system.start(&dummy);
        future
            .wait_timeout(START_ACTOR_TIMEOUT)
            .expect("actor never started");

        // Read the buffer_len
        let mut buffer_len = 0;
        let mut buffer_write_pointer = 0;
        dummy.on_definition(|c| {
            if let Some(encode_buffer) = c.ctx.get_buffer_location().borrow().as_ref() {
                buffer_len = encode_buffer.len();
                buffer_write_pointer = encode_buffer.get_write_offset();
            }
        });
        // Assert that the buffer was initialized with parameters in the hocon config string
        assert_eq!(buffer_len, 256);
        // Check that the buffer was used
        assert_ne!(buffer_write_pointer, 0);
    }
}
