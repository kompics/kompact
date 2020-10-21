use super::*;
use actors::Transport;
use arc_swap::ArcSwap;
use dispatch::lookup::ActorStore;
use net::events::NetworkEvent;

use std::{io, net::SocketAddr, sync::Arc, thread};

use crate::{
    messaging::SerialisedFrame,
    net::{events::DispatchEvent, frames::*, network_thread::NetworkThread},
    prelude::NetworkConfig,
};
use bytes::{Buf, BufMut, BytesMut};
use crossbeam_channel::{unbounded as channel, RecvError, SendError, Sender};
use mio::{Interest, Waker};

#[allow(missing_docs)]
pub mod buffers;
pub mod frames;
pub(crate) mod network_channel;
pub(crate) mod network_thread;
pub(crate) mod udp_state;

/// The state of a connection
#[derive(Debug)]
pub enum ConnectionState {
    /// Newly created
    New,
    /// Still initialising
    Initializing,
    /// Connected with a confirmed canonical SocketAddr
    Connected(SocketAddr),
    /// Already closed
    Closed,
    /// Threw an error
    Error(std::io::Error),
}

pub(crate) enum Protocol {
    TCP,
    UDP,
}
impl From<Transport> for Protocol {
    fn from(t: Transport) -> Self {
        match t {
            Transport::TCP => Protocol::TCP,
            Transport::UDP => Protocol::UDP,
            _ => unimplemented!("Unsupported Protocol"),
        }
    }
}

/// Events on the network level
pub mod events {

    use super::ConnectionState;
    use crate::net::frames::*;
    use std::net::SocketAddr;

    use crate::messaging::SerialisedFrame;

    /// Network events emitted by the network `Bridge`
    #[derive(Debug)]
    pub enum NetworkEvent {
        /// The state of a connection changed
        Connection(SocketAddr, ConnectionState),
        /// Data was received
        Data(Frame),
        /// The NetworkThread lost connection to the remote host and rejects the frame
        RejectedFrame(SocketAddr, SerialisedFrame),
    }

    /// BridgeEvents emitted to the network `Bridge`
    #[derive(Debug)]
    pub enum DispatchEvent {
        /// Send the SerialisedFrame to receiver associated with the SocketAddr
        SendTCP(SocketAddr, SerialisedFrame),
        /// Send the SerialisedFrame to receiver associated with the SocketAddr
        SendUDP(SocketAddr, SerialisedFrame),
        /// Tells the network thread to Stop
        Stop,
        /// Tells the network adress to open up a channel to the SocketAddr
        Connect(SocketAddr),
    }

    /// Errors emitted byt the network `Bridge`
    #[derive(Debug)]
    pub enum NetworkError {
        /// The protocol is not supported in this implementation
        UnsupportedProtocol,
        /// There is no executor to run the bridge on
        MissingExecutor,
        /// Some other IO error
        Io(std::io::Error),
    }

    impl From<std::io::Error> for NetworkError {
        fn from(other: std::io::Error) -> Self {
            NetworkError::Io(other)
        }
    }
}

/// The configuration for the network `Bridge`
#[allow(dead_code)]
pub struct BridgeConfig {
    retry_strategy: RetryStrategy,
}

impl BridgeConfig {
    /// Create a new config
    ///
    /// This is the same as the [Default](std::default::Default) implementation.
    pub fn new() -> Self {
        BridgeConfig::default()
    }
}

#[allow(dead_code)]
enum RetryStrategy {
    ExponentialBackoff { base_ms: u64, num_tries: usize },
}

#[allow(dead_code)]
impl Default for BridgeConfig {
    fn default() -> Self {
        let retry_strategy = RetryStrategy::ExponentialBackoff {
            base_ms: 100,
            num_tries: 5,
        };
        BridgeConfig { retry_strategy }
    }
}

/// Bridge to Network Threads. Routes outbound messages to the correct network thread. Single threaded for now.
pub struct Bridge {
    /// Network-specific configuration
    //cfg: BridgeConfig,
    /// Core logger; shared with network thread
    log: KompactLogger,
    /// Shared actor reference lookup table
    // lookup: Arc<ArcSwap<ActorStore>>,
    /// Network Thread stuff:
    // network_thread: Box<NetworkThread>,
    // ^ Can we avoid storing this by moving it into itself?
    network_input_queue: Sender<events::DispatchEvent>,
    waker: Waker,
    /// Tokio Runtime
    // tokio_runtime: Option<Runtime>,
    /// Reference back to the Kompact dispatcher
    dispatcher: Option<DispatcherRef>,
    /// Socket the network actually bound on
    bound_addr: Option<SocketAddr>,
    shutdown_future: KFuture<()>,
}

impl Bridge {
    /// Creates a new bridge
    ///
    /// # Returns
    /// A tuple consisting of the new Bridge object and the network event receiver.
    /// The receiver will allow responding to [NetworkEvent]s for external state management.
    pub fn new(
        lookup: Arc<ArcSwap<ActorStore>>,
        network_thread_log: KompactLogger,
        bridge_log: KompactLogger,
        addr: SocketAddr,
        dispatcher_ref: DispatcherRef,
        network_config: &NetworkConfig,
    ) -> (Self, SocketAddr) {
        let (sender, receiver) = channel();
        let (shutdown_p, shutdown_f) = promise();
        let (mut network_thread, waker) = NetworkThread::new(
            network_thread_log,
            addr,
            lookup,
            receiver,
            shutdown_p,
            dispatcher_ref.clone(),
            network_config.clone(),
        );
        let bound_addr = network_thread.addr;
        let bridge = Bridge {
            // cfg: BridgeConfig::default(),
            log: bridge_log,
            // lookup,
            network_input_queue: sender,
            waker,
            dispatcher: Some(dispatcher_ref),
            bound_addr: Some(bound_addr),
            shutdown_future: shutdown_f,
        };
        if let Err(e) = thread::Builder::new()
            .name("network_thread".to_string())
            .spawn(move || {
                network_thread.run();
            })
        {
            panic!("Failed to start a Network Thread, error: {:?}", e);
        }
        (bridge, bound_addr)
    }

    /// Sets the dispatcher reference, returning the previously stored one
    pub fn set_dispatcher(&mut self, dispatcher: DispatcherRef) -> Option<DispatcherRef> {
        std::mem::replace(&mut self.dispatcher, Some(dispatcher))
    }

    /// Stops the bridge
    pub fn stop(self) -> Result<(), NetworkBridgeErr> {
        debug!(self.log, "Stopping NetworkBridge...");
        self.network_input_queue.send(DispatchEvent::Stop)?;
        self.waker
            .wake()
            .expect("Network Bridge Waking NetworkThread in stop()");
        self.shutdown_future.wait(); // should block until something is sent
        debug!(self.log, "Stopped NetworkBridge.");
        Ok(())
    }

    /// Returns the local address if already bound
    pub fn local_addr(&self) -> &Option<SocketAddr> {
        &self.bound_addr
    }

    /// Forwards `serialized` to the NetworkThread and makes sure that it will wake up.
    pub(crate) fn route(
        &self,
        addr: SocketAddr,
        serialized: SerialisedFrame,
        protocol: Protocol,
    ) -> Result<(), NetworkBridgeErr> {
        match serialized {
            SerialisedFrame::Bytes(bytes) => {
                let size = FrameHead::encoded_len() + bytes.len();
                let mut buf = BytesMut::with_capacity(size);
                let mut head = FrameHead::new(FrameType::Data, bytes.len());
                head.encode_into(&mut buf);
                // TODO: what is this used for?
                buf.put_slice(bytes.bytes());
                match protocol {
                    Protocol::TCP => {
                        self.network_input_queue
                            .send(events::DispatchEvent::SendTCP(
                                addr,
                                SerialisedFrame::Bytes(buf.freeze()),
                            ))?;
                    }
                    Protocol::UDP => {
                        self.network_input_queue
                            .send(events::DispatchEvent::SendUDP(
                                addr,
                                SerialisedFrame::Bytes(buf.freeze()),
                            ))?;
                    }
                }
            }
            SerialisedFrame::ChunkLease(chunk) => match protocol {
                Protocol::TCP => {
                    self.network_input_queue
                        .send(events::DispatchEvent::SendTCP(
                            addr,
                            SerialisedFrame::ChunkLease(chunk),
                        ))?;
                }
                Protocol::UDP => {
                    self.network_input_queue
                        .send(events::DispatchEvent::SendUDP(
                            addr,
                            SerialisedFrame::ChunkLease(chunk),
                        ))?;
                }
            },
            SerialisedFrame::ChunkRef(chunk) => match protocol {
                Protocol::TCP => {
                    self.network_input_queue
                        .send(events::DispatchEvent::SendTCP(
                            addr,
                            SerialisedFrame::ChunkRef(chunk),
                        ))?;
                }
                Protocol::UDP => {
                    self.network_input_queue
                        .send(events::DispatchEvent::SendUDP(
                            addr,
                            SerialisedFrame::ChunkRef(chunk),
                        ))?;
                }
            },
        }
        self.waker.wake()?;
        Ok(())
    }

    /// Attempts to establish a TCP connection to the provided `addr`.
    ///
    /// # Side effects
    /// When the connection is successul:
    ///     - a `ConnectionState::Connected` is dispatched on the network bridge event queue
    ///     - NetworkThread will listen for incoming messages and write outgoing messages on the channel
    ///
    /// # Errors
    /// If the provided protocol is not supported
    pub fn connect(&self, proto: Transport, addr: SocketAddr) -> Result<(), NetworkBridgeErr> {
        match proto {
            Transport::TCP => {
                self.network_input_queue
                    .send(events::DispatchEvent::Connect(addr))?;
                self.waker.wake()?;
                Ok(())
            }
            _other => Err(NetworkBridgeErr::Other("Bad Protocol".to_string())),
        }
    }
}

/// Errors which the NetworkBridge might return, not used for now.
#[derive(Debug)]
pub enum NetworkBridgeErr {
    /// Something went wrong while binding
    Binding(String),
    /// Something went wrong with the thread
    Thread(String),
    /// Something else went wrong
    Other(String),
}

impl<T> From<SendError<T>> for NetworkBridgeErr {
    fn from(error: SendError<T>) -> Self {
        NetworkBridgeErr::Other(format!("SendError: {:?}", error))
    }
}

impl From<io::Error> for NetworkBridgeErr {
    fn from(error: io::Error) -> Self {
        NetworkBridgeErr::Other(format!("io::Error: {:?}", error))
    }
}

impl From<RecvError> for NetworkBridgeErr {
    fn from(error: RecvError) -> Self {
        NetworkBridgeErr::Other(format!("RecvError: {:?}", error))
    }
}

impl From<SerError> for NetworkBridgeErr {
    fn from(error: SerError) -> Self {
        NetworkBridgeErr::Other(format!("SerError: {:?}", error))
    }
}

/*
* Error handling helper functions
*/
pub(crate) fn would_block(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
}

pub(crate) fn interrupted(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Interrupted
}

pub(crate) fn no_buffer_space(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::InvalidData
}

pub(crate) fn connection_reset(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::ConnectionReset
}

pub(crate) fn broken_pipe(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::BrokenPipe
}

/// A module with helper functions for testing network configurations/implementations
pub mod net_test_helpers {
    use crate::prelude::*;
    use std::{
        collections::VecDeque,
        fmt::{Debug, Formatter},
        time::{SystemTime, UNIX_EPOCH},
    };

    /// The number of Ping-Pong messages, used in assertions and Pingers/BigPingers
    pub const PING_COUNT: u64 = 10;

    #[derive(Debug, Clone)]
    struct PingMsg {
        i: u64,
    }

    #[derive(Debug, Clone)]
    struct PongMsg {
        i: u64,
    }

    struct PingPongSer;

    impl PingPongSer {
        const SID: SerId = 42;
    }

    const PING_ID: i8 = 1;
    const PONG_ID: i8 = 2;

    impl Serialiser<PingMsg> for PingPongSer {
        fn ser_id(&self) -> SerId {
            Self::SID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(9)
        }

        fn serialise(&self, v: &PingMsg, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_i8(PING_ID);
            buf.put_u64(v.i);
            Result::Ok(())
        }
    }

    impl Serialisable for PingMsg {
        fn ser_id(&self) -> SerId {
            42
        }

        fn size_hint(&self) -> Option<usize> {
            Some(9)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_i8(PING_ID);
            buf.put_u64(self.i);
            Result::Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Serialisable for PongMsg {
        fn ser_id(&self) -> SerId {
            42
        }

        fn size_hint(&self) -> Option<usize> {
            Some(9)
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_i8(PONG_ID);
            buf.put_u64(self.i);
            Result::Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Serialiser<PongMsg> for PingPongSer {
        fn ser_id(&self) -> SerId {
            Self::SID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(9)
        }

        fn serialise(&self, v: &PongMsg, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_i8(PONG_ID);
            buf.put_u64(v.i);
            Result::Ok(())
        }
    }

    impl Deserialiser<PingMsg> for PingPongSer {
        const SER_ID: SerId = Self::SID;

        fn deserialise(buf: &mut dyn Buf) -> Result<PingMsg, SerError> {
            if buf.remaining() < 9 {
                return Err(SerError::InvalidData(format!(
                    "Serialised typed has 9bytes but only {}bytes remain in buffer.",
                    buf.remaining()
                )));
            }
            match buf.get_i8() {
                PING_ID => {
                    let i = buf.get_u64();
                    Ok(PingMsg { i })
                }
                PONG_ID => Err(SerError::InvalidType(
                    "Found PongMsg, but expected PingMsg.".into(),
                )),
                id => Err(SerError::InvalidType(format!(
                    "Found unknown id {}, but expected PingMsg.",
                    id
                ))),
            }
        }
    }

    impl Deserialiser<PongMsg> for PingPongSer {
        const SER_ID: SerId = Self::SID;

        fn deserialise(buf: &mut dyn Buf) -> Result<PongMsg, SerError> {
            if buf.remaining() < 9 {
                return Err(SerError::InvalidData(format!(
                    "Serialised typed has 9bytes but only {}bytes remain in buffer.",
                    buf.remaining()
                )));
            }
            match buf.get_i8() {
                PONG_ID => {
                    let i = buf.get_u64();
                    Ok(PongMsg { i })
                }
                PING_ID => Err(SerError::InvalidType(
                    "Found PingMsg, but expected PongMsg.".into(),
                )),
                id => Err(SerError::InvalidType(format!(
                    "Found unknown id {}, but expected PingMsg.",
                    id
                ))),
            }
        }
    }

    /// An actor which will send `PingMsg` to `target` over the network and counts the number of
    /// `PongMsg` replies it receives.
    /// Target should be a [PongerAct](PongerAct).
    #[derive(ComponentDefinition)]
    pub struct PingerAct {
        ctx: ComponentContext<PingerAct>,
        target: ActorPath,
        /// The number of `PongMsg` received
        pub count: u64,
        eager: bool,
    }

    impl PingerAct {
        /// Creates a new `PingerAct` sending messages using
        /// [Lazy Serialisation](crate::actors::ActorPath#method.tell)
        pub fn new_lazy(target: ActorPath) -> PingerAct {
            PingerAct {
                ctx: ComponentContext::uninitialised(),
                target,
                count: 0,
                eager: false,
            }
        }

        /// Creates a new `PingerAct` sending messages using
        /// [Eager Serialisation](crate::actors::ActorPath#method.tell_serialised)
        pub fn new_eager(target: ActorPath) -> PingerAct {
            PingerAct {
                ctx: ComponentContext::uninitialised(),
                target,
                count: 0,
                eager: true,
            }
        }
    }

    impl ComponentLifecycle for PingerAct {
        fn on_start(&mut self) -> Handled {
            debug!(self.ctx.log(), "Starting");
            if self.eager {
                self.target
                    .tell_serialised(PingMsg { i: 0 }, self)
                    .expect("serialise");
            } else {
                self.target.tell(PingMsg { i: 0 }, self);
            }
            Handled::Ok
        }
    }

    impl Actor for PingerAct {
        type Message = Never;

        fn receive_local(&mut self, _pong: Self::Message) -> Handled {
            unimplemented!();
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
            match msg.try_deserialise::<PongMsg, PingPongSer>() {
                Ok(pong) => {
                    debug!(self.ctx.log(), "Got msg {:?}", pong);
                    self.count += 1;
                    if self.count < PING_COUNT {
                        if self.eager {
                            self.target
                                .tell_serialised(PingMsg { i: pong.i + 1 }, self)
                                .expect("serialise");
                        } else {
                            self.target.tell(PingMsg { i: pong.i + 1 }, self);
                        }
                    }
                }
                Err(e) => error!(self.ctx.log(), "Error deserialising PongMsg: {:?}", e),
            }
            Handled::Ok
        }
    }

    /// An actor which replies to `PingMsg` with a `PongMsg`
    #[derive(ComponentDefinition)]
    pub struct PongerAct {
        ctx: ComponentContext<PongerAct>,
        eager: bool,
    }

    impl PongerAct {
        /// Creates a new `PongerAct` replying to `PingMsg`'s using
        /// [Lazy Serialisation](crate::actors::ActorPath#method.tell)
        pub fn new_lazy() -> PongerAct {
            PongerAct {
                ctx: ComponentContext::uninitialised(),
                eager: false,
            }
        }

        /// Creates a new `PongerAct` replying to `PingMsg`'s using
        /// [Eager Serialisation](crate::actors::ActorPath#method.tell_serialised)
        pub fn new_eager() -> PongerAct {
            PongerAct {
                ctx: ComponentContext::uninitialised(),
                eager: true,
            }
        }
    }

    ignore_lifecycle!(PongerAct);

    impl Actor for PongerAct {
        type Message = Never;

        fn receive_local(&mut self, _ping: Self::Message) -> Handled {
            unimplemented!();
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
            let sender = msg.sender;
            match_deser! {msg.data; {
                ping: PingMsg [PingPongSer] => {
                    debug!(self.ctx.log(), "Got msg {:?} from {}", ping, sender);
                    let pong = PongMsg { i: ping.i };
                    if self.eager {
                        sender
                            .tell_serialised(pong, self)
                            .expect("PongMsg should serialise");
                    } else {
                        sender.tell(pong, self);
                    }
                },
                !Err(e) => error!(self.ctx.log(), "Error deserialising PingMsg: {:?}", e),
            }}
            Handled::Ok
        }
    }

    /// An actor which forwards all network messages to `forward_to`
    #[derive(ComponentDefinition)]
    pub struct ForwarderAct {
        ctx: ComponentContext<Self>,
        forward_to: ActorPath,
    }
    impl ForwarderAct {
        /// Creates a new `ForwarderAct`
        pub fn new(forward_to: ActorPath) -> Self {
            ForwarderAct {
                ctx: ComponentContext::uninitialised(),
                forward_to,
            }
        }
    }
    ignore_lifecycle!(ForwarderAct);
    impl Actor for ForwarderAct {
        type Message = Never;

        fn receive_local(&mut self, _ping: Self::Message) -> Handled {
            unimplemented!();
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
            debug!(
                self.ctx.log(),
                "Forwarding some msg from {} to {}", msg.sender, self.forward_to
            );
            self.forward_to.forward_with_original_sender(msg, self);
            Handled::Ok
        }
    }

    #[derive(Clone)]
    struct BigPingMsg {
        i: u64,
        data: Vec<u8>,
        sum: u64,
    }

    impl BigPingMsg {
        fn new(i: u64, data_size: usize) -> Self {
            let mut data = Vec::<u8>::with_capacity(data_size);
            let mut sum: u64 = 0;
            // create pseudo random values:
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_millis() as u64;
            for _ in 0..data_size {
                let d = (seed * (2 + i) % 128) as u8;
                data.push(d);
                sum += d as u64;
            }
            BigPingMsg { i, data, sum }
        }

        fn validate(&self) -> () {
            let mut sum: u64 = 0;
            for d in &self.data {
                sum += *d as u64;
            }
            assert_eq!(self.sum, sum, "BigPingMsg invalid, {:?}", self);
        }
    }

    #[derive(Clone)]
    struct BigPongMsg {
        i: u64,
        data: Vec<u8>,
        sum: u64,
    }

    impl BigPongMsg {
        fn new(i: u64, data_size: usize) -> BigPongMsg {
            let mut data = Vec::<u8>::with_capacity(data_size);
            let mut sum: u64 = 0;
            // create pseudo random values:
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_millis() as u64;
            for _ in 0..data_size {
                let d = (seed * (2 + i) % 128) as u8;
                data.push(d);
                sum += d as u64;
            }
            BigPongMsg { i, data, sum }
        }

        fn validate(&self) -> () {
            let mut sum: u64 = 0;
            for d in &self.data {
                sum += *d as u64;
            }
            assert_eq!(self.sum, sum, "BigPongMsg invalid, {:?}", self);
        }
    }

    impl Debug for BigPingMsg {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("BigPingMsg")
                .field("i", &self.i)
                .field("data length", &self.data.len())
                .field("sum", &self.sum)
                .field("data 0", &self.data[0])
                .field("data n", &self.data[&self.data.len() - 1])
                .finish()
        }
    }
    impl Debug for BigPongMsg {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("BigPongMsg")
                .field("i", &self.i)
                .field("data length", &self.data.len())
                .field("sum", &self.sum)
                .field("data 0", &self.data[0])
                .field("data n", &self.data[&self.data.len() - 1])
                .finish()
        }
    }

    struct BigPingPongSer;

    impl BigPingPongSer {
        const SID: SerId = 43;
    }

    impl Serialisable for BigPingMsg {
        fn ser_id(&self) -> SerId {
            43
        }

        fn size_hint(&self) -> Option<usize> {
            Some(32 + self.data.len())
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_i8(PING_ID);
            buf.put_u64(self.i);
            buf.put_u64(self.data.len() as u64);
            for d in &self.data {
                buf.put_u8(*d);
            }
            buf.put_u64(self.sum);
            Result::Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Serialisable for BigPongMsg {
        fn ser_id(&self) -> SerId {
            43
        }

        fn size_hint(&self) -> Option<usize> {
            Some(32 + self.data.len())
        }

        fn serialise(&self, buf: &mut dyn BufMut) -> Result<(), SerError> {
            buf.put_i8(PONG_ID);
            buf.put_u64(self.i);
            buf.put_u64(self.data.len() as u64);
            for d in &self.data {
                buf.put_u8(*d);
            }
            buf.put_u64(self.sum);
            Result::Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }
    }

    impl Deserialiser<BigPingMsg> for BigPingPongSer {
        const SER_ID: SerId = Self::SID;

        fn deserialise(buf: &mut dyn Buf) -> Result<BigPingMsg, SerError> {
            if buf.remaining() < 18 {
                return Err(SerError::InvalidData(format!(
                    "Serialised typed has 18 bytes but only {} bytes remain in buffer.",
                    buf.remaining()
                )));
            }
            match buf.get_i8() {
                PING_ID => {
                    let i = buf.get_u64();
                    let data_len = buf.get_u64() as usize;
                    let mut data = Vec::with_capacity(data_len as usize);
                    for j in 0..data_len {
                        data.insert(j, buf.get_u8());
                    }
                    let sum = buf.get_u64();
                    assert_eq!(buf.remaining(), 0, "Buffer too big! {}", buf.remaining());
                    Ok(BigPingMsg { i, data, sum })
                }
                PONG_ID => Err(SerError::InvalidType(
                    "Found BigPongMsg, but expected BigPingMsg.".into(),
                )),
                id => Err(SerError::InvalidType(format!(
                    "Found unknown id {}, but expected BigPingMsg.",
                    id
                ))),
            }
        }
    }

    impl Deserialiser<BigPongMsg> for BigPingPongSer {
        const SER_ID: SerId = Self::SID;

        fn deserialise(buf: &mut dyn Buf) -> Result<BigPongMsg, SerError> {
            if buf.remaining() < 18 {
                return Err(SerError::InvalidData(format!(
                    "Serialised typed has 18 bytes but only {} bytes remain in buffer.",
                    buf.remaining()
                )));
            }
            match buf.get_i8() {
                PONG_ID => {
                    let i = buf.get_u64();
                    let data_len = buf.get_u64() as usize;
                    let mut data = Vec::with_capacity(data_len as usize);
                    for j in 0..data_len {
                        data.insert(j, buf.get_u8());
                    }
                    let sum = buf.get_u64();
                    assert_eq!(buf.remaining(), 0, "Buffer too big!");
                    Ok(BigPongMsg { i, data, sum })
                }
                PING_ID => Err(SerError::InvalidType(
                    "Found BigPingMsg, but expected BigPongMsg.".into(),
                )),
                id => Err(SerError::InvalidType(format!(
                    "Found unknown id {}, but expected BigPingMsg.",
                    id
                ))),
            }
        }
    }

    /// An actor which will send `BigPingMsg` to `target` over the network and counts the number of
    /// `BigPongMsg` replies it receives.
    ///
    /// The Actors can be configured with different data-sizes and fills each message with
    /// pseudo-random data and a checksum. When receiving a message the actors verify the checksum.
    ///
    /// Target should be a [BigPongerAct](BigPongerAct).
    #[derive(ComponentDefinition)]
    pub struct BigPingerAct {
        ctx: ComponentContext<BigPingerAct>,
        target: ActorPath,
        /// The number of `BigPongMsg` this actor has received
        pub count: u64,
        data_size: usize,
        eager: bool,
        buffer_config: BufferConfig,
        pre_serialised: Option<VecDeque<ChunkRef>>,
    }

    #[allow(dead_code)]
    impl BigPingerAct {
        /// Creates a `BigPingerAct` which will send messages using
        /// [lazy serialisation](crate::actors::ActorPath#tell)
        ///
        /// `target` should be a [BigPongerAct]() and `data_size` will determine the size of the
        /// u8 array generated and embedded in each `BigPingMsg`.
        pub fn new_lazy(target: ActorPath, data_size: usize) -> BigPingerAct {
            BigPingerAct {
                ctx: ComponentContext::uninitialised(),
                target,
                count: 0,
                data_size,
                eager: false,
                buffer_config: BufferConfig::default(),
                pre_serialised: None,
            }
        }

        /// Creates a `BigPingerAct` which will send messages using
        /// [eager serialisation](crate::actors::ActorPath#tell_serialised).
        pub fn new_eager(
            target: ActorPath,
            data_size: usize,
            buffer_config: BufferConfig,
        ) -> BigPingerAct {
            BigPingerAct {
                ctx: ComponentContext::uninitialised(),
                target,
                count: 0,
                data_size,
                eager: true,
                buffer_config,
                pre_serialised: None,
            }
        }

        /// Creates a `BigPingerAct` which will send messages using
        /// [Preserialisation](crate::actors::ActorPath#tell_preserialised).
        pub fn new_preserialised(
            target: ActorPath,
            data_size: usize,
            buffer_config: BufferConfig,
        ) -> BigPingerAct {
            let pre_serialised = VecDeque::with_capacity(PING_COUNT as usize);
            BigPingerAct {
                ctx: ComponentContext::uninitialised(),
                target,
                count: 0,
                data_size,
                eager: true,
                buffer_config,
                pre_serialised: Some(pre_serialised),
            }
        }
    }

    impl ComponentLifecycle for BigPingerAct {
        fn on_start(&mut self) -> Handled {
            let ping = BigPingMsg::new(0, self.data_size);
            if self.eager {
                self.ctx
                    .init_buffers(Some(self.buffer_config.clone()), None);
            }
            debug!(self.ctx.log(), "Starting, sending first ping: {:?}", ping);
            if self.eager {
                if let Some(msgs) = &mut self.pre_serialised {
                    for i in 0..PING_COUNT {
                        msgs.push_back(
                            self.ctx
                                .preserialise(&BigPingMsg::new(i, self.data_size))
                                .expect("serialise"),
                        );
                    }
                    self.target
                        .tell_preserialised(msgs.pop_front().expect("popping"), self)
                        .expect("telling");
                } else {
                    self.target.tell_serialised(ping, self).expect("serialise");
                }
            } else {
                self.target.tell(ping, self);
            }
            Handled::Ok
        }
    }

    impl Actor for BigPingerAct {
        type Message = Never;

        fn receive_local(&mut self, _pong: Self::Message) -> Handled {
            unimplemented!();
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
            match msg.try_deserialise::<BigPongMsg, BigPingPongSer>() {
                Ok(pong) => {
                    debug!(self.ctx.log(), "Got msg {:?}", pong);
                    pong.validate();
                    assert_eq!(
                        pong.data.len(),
                        self.data_size,
                        "Incorrect Pong data-length"
                    );
                    assert_eq!(pong.i, self.count, "Incorrect index of pong!");
                    self.count += 1;
                    if self.count < PING_COUNT {
                        let ping = BigPingMsg::new(pong.i + 1, self.data_size);
                        if self.eager {
                            if let Some(msgs) = &mut self.pre_serialised {
                                self.target
                                    .tell_preserialised(msgs.pop_front().expect("popping"), self)
                                    .expect("telling");
                            } else {
                                self.target.tell_serialised(ping, self).expect("serialise");
                            }
                        } else {
                            self.target.tell(ping, self);
                        }
                    }
                }
                Err(e) => error!(self.ctx.log(), "Error deserialising BigPongMsg: {:?}", e),
            }
            Handled::Ok
        }
    }

    /// An actor which will reply to `BigPingMsg` over the network with `BigPongMsg`
    ///
    /// Verifies the checksum of each `BigPingMsg` and replies with an equally sized `BigPongMsg`.
    #[derive(ComponentDefinition)]
    pub struct BigPongerAct {
        ctx: ComponentContext<BigPongerAct>,
        eager: bool,
        buffer_config: BufferConfig,
    }

    #[allow(dead_code)]
    impl BigPongerAct {
        /// Creates a `BigPongerAct` which will send messages using
        /// [Lazy Serialisation](crate::actors::ActorPath#tell)
        pub fn new_lazy() -> BigPongerAct {
            BigPongerAct {
                ctx: ComponentContext::uninitialised(),
                eager: false,
                buffer_config: BufferConfig::default(),
            }
        }

        /// Creates a `BigPingerAct` which will send messages using
        /// [Eager Serialisation](crate::actors::ActorPath#tell_serialised)
        pub fn new_eager(buffer_config: BufferConfig) -> BigPongerAct {
            BigPongerAct {
                ctx: ComponentContext::uninitialised(),
                eager: true,
                buffer_config,
            }
        }
    }

    impl ComponentLifecycle for BigPongerAct {
        fn on_start(&mut self) -> Handled {
            if self.eager {
                self.ctx
                    .init_buffers(Some(self.buffer_config.clone()), None);
            }
            Handled::Ok
        }
    }

    impl Actor for BigPongerAct {
        type Message = Never;

        fn receive_local(&mut self, _ping: Self::Message) -> Handled {
            unimplemented!();
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
            let sender = msg.sender;
            match_deser! {msg.data; {
                ping: BigPingMsg [BigPingPongSer] => {
                    debug!(self.ctx.log(), "Got msg {:?} from {}", ping, sender);
                    ping.validate();
                    let pong = BigPongMsg::new(ping.i, ping.data.len());
                    if self.eager {
                        sender
                            .tell_serialised(pong, self)
                            .expect("BigPongMsg should serialise");
                    } else {
                        sender.tell(pong, self);
                    }
                },
                !Err(e) => error!(self.ctx.log(), "Error deserialising BigPingMsg: {:?}", e),
            }}
            Handled::Ok
        }
    }
}
