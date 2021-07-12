use super::*;
use actors::Transport;
use arc_swap::ArcSwap;
use dispatch::lookup::ActorStore;

use crate::{
    messaging::DispatchData,
    net::{events::DispatchEvent, frames::*, network_thread::NetworkThreadBuilder},
    prelude::NetworkConfig,
};
use crossbeam_channel::{unbounded as channel, RecvError, SendError, Sender};
use mio::{Interest, Waker};
pub use std::net::SocketAddr;
use std::{io, net::IpAddr, panic, sync::Arc, thread, time::Duration};
use uuid::Uuid;

#[allow(missing_docs)]
pub mod buffers;
pub mod frames;
pub(crate) mod network_channel;
pub(crate) mod network_thread;
pub(crate) mod udp_state;

/// The state of a connection
#[derive(Clone, Debug)]
pub enum ConnectionState {
    /// Newly created
    New,
    /// Initialising the connection
    Initializing,
    /// Connected
    Connected(SessionId),
    /// Closed gracefully, by request on either side
    Closed(SessionId),
    /// Unexpected lost connection
    Lost(SessionId),
    /// Blocked by request from component through NetworkStatusPort
    Blocked,
    // Threw an error
    // Error(std::io::Error),
}

pub(crate) enum Protocol {
    Tcp,
    Udp,
}
impl From<Transport> for Protocol {
    fn from(t: Transport) -> Self {
        match t {
            Transport::Tcp => Protocol::Tcp,
            Transport::Udp => Protocol::Udp,
            _ => unimplemented!("Unsupported Protocol"),
        }
    }
}

/// Session identifier, part of the `NetMessage` struct. Managed by Kompact internally, may be read
/// by users to detect session loss, indicated by different SessionId's of NetMessages.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(Uuid);

impl SessionId {
    /// Generates a new SessionId
    pub fn new_unique() -> SessionId {
        SessionId(Uuid::new_v4())
    }

    /// For Serialisation of SessionId's
    ///
    /// **Note: User code should not rely on the internal representation of SessionId's**
    pub fn as_u128(&self) -> u128 {
        self.0.as_u128()
    }

    /// For Deserialisation of SessionId's
    ///
    /// **Note: User code should not rely on the internal representation of SessionId's**
    pub fn from_u128(v: u128) -> SessionId {
        SessionId(Uuid::from_u128(v))
    }
}

/// Events on the network level
pub mod events {

    use crate::{messaging::DispatchData, net::SocketAddr};
    use std::net::IpAddr;
    /// BridgeEvents emitted to the network `Bridge`
    #[derive(Debug)]
    pub enum DispatchEvent {
        /// Send the `SerialisedFrame` to receiver associated with the `SocketAddr`
        SendTcp(SocketAddr, DispatchData),
        /// Send the `SerialisedFrame` to receiver associated with the `SocketAddr`
        SendUdp(SocketAddr, DispatchData),
        /// Tells the network thread to Stop, will gracefully shutdown all channels.
        Stop,
        /// Tells the network thread to Die as soon as possible, without graceful shutdown.
        Kill,
        /// Tells the `NetworkThread` to open up a channel to the `SocketAddr`
        Connect(SocketAddr),
        /// Acknowledges a closed channel, required to ensure FIFO ordering under connection loss
        ClosedAck(SocketAddr),
        /// Tells the `NetworkThread` to gracefully close the channel to the `SocketAddr`
        Close(SocketAddr),
        /// Tells the `NetworkThread` to block the `SocketAddr`
        BlockSocket(SocketAddr),
        /// Tells the `NetworkThread` to block the `IpAddr`
        BlockIpAddr(IpAddr),
        /// Tells the `NetworkThread` to block the `SocketAddr`
        UnblockSocket(SocketAddr),
        /// Tells the `NetworkThread` to block the `IpAddr`
        UnblockIpAddr(IpAddr),
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
    bound_address: Option<SocketAddr>,
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
        match NetworkThreadBuilder::new(
            network_thread_log,
            addr,
            lookup,
            receiver,
            shutdown_p,
            dispatcher_ref.clone(),
            network_config.clone(),
        ) {
            Ok(mut network_thread_builder) => {
                let bound_address = network_thread_builder.address;
                let waker = network_thread_builder
                    .take_waker()
                    .expect("NetworkThread poll error");

                let (started_p, started_f) = promise();
                run_network_thread(network_thread_builder, bridge_log.clone(), started_p)
                    .expect("Failed to spawn NetworkThread");
                started_f
                    .wait_timeout(Duration::from_millis(network_config.get_boot_timeout()))
                    .expect("NetworkThread time-out during boot sequence");

                let bridge = Bridge {
                    // cfg: BridgeConfig::default(),
                    log: bridge_log,
                    // lookup,
                    network_input_queue: sender,
                    waker,
                    dispatcher: Some(dispatcher_ref),
                    bound_address: Some(bound_address),
                    shutdown_future: shutdown_f,
                };

                (bridge, bound_address)
            }
            Err(e) => {
                panic!("Failed to build a Network Thread, error: {:?}", e);
            }
        }
    }

    /// Sets the dispatcher reference, returning the previously stored one
    pub fn set_dispatcher(&mut self, dispatcher: DispatcherRef) -> Option<DispatcherRef> {
        std::mem::replace(&mut self.dispatcher, Some(dispatcher))
    }

    /// Stops the bridge gracefully
    pub fn stop(self) -> Result<(), NetworkBridgeErr> {
        debug!(self.log, "Stopping NetworkBridge...");
        self.network_input_queue.send(DispatchEvent::Stop)?;
        self.waker.wake()?;
        self.shutdown_future.wait(); // should block until something is sent
        debug!(self.log, "Stopped NetworkBridge.");
        Ok(())
    }

    /// Kills the Network
    pub fn kill(self) -> Result<(), NetworkBridgeErr> {
        debug!(self.log, "Killing NetworkBridge...");
        self.network_input_queue.send(DispatchEvent::Kill)?;
        self.waker.wake()?;
        self.shutdown_future.wait(); // should block until something is sent
        debug!(self.log, "Stopped NetworkBridge.");
        Ok(())
    }

    /// Returns the local address if already bound
    pub fn local_addr(&self) -> &Option<SocketAddr> {
        &self.bound_address
    }

    /// Forwards `serialized` to the NetworkThread and makes sure that it will wake up.
    pub(crate) fn route(
        &self,
        addr: SocketAddr,
        data: DispatchData,
        protocol: Protocol,
    ) -> Result<(), NetworkBridgeErr> {
        match protocol {
            Protocol::Tcp => {
                let _ = self
                    .network_input_queue
                    .send(DispatchEvent::SendTcp(addr, data))?;
            }
            Protocol::Udp => {
                let _ = self
                    .network_input_queue
                    .send(DispatchEvent::SendUdp(addr, data))?;
            }
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
            Transport::Tcp => {
                self.network_input_queue
                    .send(events::DispatchEvent::Connect(addr))?;
                self.waker.wake()?;
                Ok(())
            }
            _other => Err(NetworkBridgeErr::Other("Bad Protocol".to_string())),
        }
    }

    /// Acknowledges a closed channel, required to ensure FIFO ordering under connection loss
    pub fn ack_closed(&self, addr: SocketAddr) -> Result<(), NetworkBridgeErr> {
        self.network_input_queue
            .send(events::DispatchEvent::ClosedAck(addr))?;
        self.waker.wake()?;
        Ok(())
    }

    /// Requests that the NetworkThread should be closed
    pub fn close_channel(&self, addr: SocketAddr) -> Result<(), NetworkBridgeErr> {
        self.network_input_queue
            .send(events::DispatchEvent::Close(addr))?;
        self.waker.wake()?;
        Ok(())
    }

    /// Requests the NetworkThread to block the socket addr
    pub fn block_socket(&self, addr: SocketAddr) -> Result<(), NetworkBridgeErr> {
        self.network_input_queue
            .send(events::DispatchEvent::BlockSocket(addr))?;
        self.waker.wake()?;
        Ok(())
    }

    /// Requests the NetworkThread to block the ip address ip_addr
    pub fn block_ip(&self, ip_addr: IpAddr) -> Result<(), NetworkBridgeErr> {
        self.network_input_queue
            .send(events::DispatchEvent::BlockIpAddr(ip_addr))?;
        self.waker.wake()?;
        Ok(())
    }

    /// Requests the NetworkThread to unblock the socket addr
    pub fn unblock_socket(&self, addr: SocketAddr) -> Result<(), NetworkBridgeErr> {
        self.network_input_queue
            .send(events::DispatchEvent::UnblockSocket(addr))?;
        self.waker.wake()?;
        Ok(())
    }

    /// Requests the NetworkThread to unblock the ip address ip_addr
    pub fn unblock_ip(&self, ip_addr: IpAddr) -> Result<(), NetworkBridgeErr> {
        self.network_input_queue
            .send(events::DispatchEvent::UnblockIpAddr(ip_addr))?;
        self.waker.wake()?;
        Ok(())
    }
}

fn run_network_thread(
    builder: NetworkThreadBuilder,
    logger: KompactLogger,
    started_promise: KPromise<()>,
) -> async_std::io::Result<()> {
    thread::Builder::new()
        .name("network_thread".to_string())
        .spawn(move || {
            if let Err(e) = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                let network_thread = builder.build();
                let _ = started_promise
                    .complete()
                    .expect("NetworkThread started but failed to fulfil promise");
                network_thread.run()
            })) {
                if let Some(error_msg) = e.downcast_ref::<&str>() {
                    error!(logger, "NetworkThread panicked with: {:?}", error_msg);
                } else if let Some(error_msg) = e.downcast_ref::<String>() {
                    error!(logger, "NetworkThread panicked with: {:?}", error_msg);
                } else {
                    error!(
                        logger,
                        "NetworkThread panicked with a non-string message with type id={:?}",
                        e.type_id()
                    );
                };
            };
        })
        .map(|_| ())
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
    err.kind() == io::ErrorKind::InvalidInput
}

pub(crate) fn connection_reset(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::ConnectionReset
}

pub(crate) fn broken_pipe(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::BrokenPipe
}

pub(crate) fn out_of_buffers(err: &SerError) -> bool {
    matches!(err, SerError::NoBuffersAvailable(_))
}

/// A module with helper functions for testing network configurations/implementations
pub mod net_test_helpers {
    use crate::prelude::*;
    use crossbeam_channel::Sender;
    use std::{
        cmp::Ordering,
        collections::VecDeque,
        fmt::{Debug, Formatter},
        net::IpAddr,
        time::{Duration, SystemTime, UNIX_EPOCH},
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

    #[derive(Debug, Clone)]
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
        promise: Option<KPromise<()>>,
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
                promise: None,
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
                promise: None,
            }
        }

        /// Creates a future which will be completed by the Pinger when it has received pongs for
        /// all the pings it sent
        pub fn completion_future(&mut self) -> KFuture<()> {
            let (promise, future) = promise();
            self.promise = Some(promise);
            future
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
                    match self.count.cmp(&PING_COUNT) {
                        Ordering::Less if self.eager => {
                            self.target
                                .tell_serialised(PingMsg { i: pong.i + 1 }, self)
                                .expect("serialise");
                        }
                        Ordering::Less => {
                            self.target.tell(PingMsg { i: pong.i + 1 }, self);
                        }
                        Ordering::Equal if self.promise.is_some() => {
                            self.promise
                                .take()
                                .unwrap()
                                .complete()
                                .expect("Failed to fulfil promise");
                        }
                        _ => (), // ignore
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
        /// number of `PingMsg` received
        pub count: u64,
    }

    impl PongerAct {
        /// Creates a new `PongerAct` replying to `PingMsg`'s using
        /// [Lazy Serialisation](crate::actors::ActorPath#method.tell)
        pub fn new_lazy() -> PongerAct {
            PongerAct {
                ctx: ComponentContext::uninitialised(),
                eager: false,
                count: 0,
            }
        }

        /// Creates a new `PongerAct` replying to `PingMsg`'s using
        /// [Eager Serialisation](crate::actors::ActorPath#method.tell_serialised)
        pub fn new_eager() -> PongerAct {
            PongerAct {
                ctx: ComponentContext::uninitialised(),
                eager: true,
                count: 0,
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
            match_deser! {
                (msg.data) {
                    msg(ping): PingMsg [using PingPongSer] => {
                        debug!(self.ctx.log(), "Got msg {:?} from {}", ping, sender);
                        self.count += 1;
                        let pong = PongMsg { i: ping.i };
                        if self.eager {
                            sender
                                .tell_serialised(pong, self)
                                .expect("PongMsg should serialise");
                        } else {
                            sender.tell(pong, self);
                        }
                    },
                    err(e) => error!(self.ctx.log(), "Error deserialising PingMsg: {:?}", e),
                }
            }
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
        promise: Option<KPromise<()>>,
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
                promise: None,
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
                promise: None,
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
                promise: None,
            }
        }

        /// Creates a future which will be completed by the Pinger when it has received pongs for
        /// all the pings it sent
        pub fn completion_future(&mut self) -> KFuture<()> {
            let (promise, future) = promise();
            self.promise = Some(promise);
            future
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
                    assert_eq!(pong.i, self.count % PING_COUNT, "Incorrect index of pong!");
                    self.count += 1;
                    if self.count % PING_COUNT > 0 {
                        if self.eager {
                            if let Some(msgs) = &mut self.pre_serialised {
                                self.target
                                    .tell_preserialised(msgs.pop_front().expect("popping"), self)
                                    .expect("telling");
                            } else {
                                let ping = BigPingMsg::new(pong.i + 1, self.data_size);
                                self.target.tell_serialised(ping, self).expect("serialise");
                            }
                        } else {
                            let ping = BigPingMsg::new(pong.i + 1, self.data_size);
                            self.target.tell(ping, self);
                        }
                    } else if let Some(promise) = self.promise.take() {
                        promise.complete().expect("Failed to fulfil promise");
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
            match_deser! {
                (msg.data) {
                    msg(ping): BigPingMsg [using BigPingPongSer] => {
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
                    err(e) => error!(self.ctx.log(), "Error deserialising BigPingMsg: {:?}", e),
                }
            }
            Handled::Ok
        }
    }

    /// Actor which can subscribe to the `NetworkStatusPort` and maintains a counter of how many
    /// of each kind of NetworkStatusUpdate has been received.
    #[derive(ComponentDefinition)]
    pub struct NetworkStatusCounter {
        ctx: ComponentContext<NetworkStatusCounter>,
        /// The NetworkStatusPort, needs to be exposed to allow connecting to it
        pub network_status_port: RequiredPort<NetworkStatusPort>,
        /// Counts the number of connection_established messages received
        pub connection_established: u32,
        /// Counts the number of connection_lost messages received
        pub connection_lost: u32,
        /// Counts the number of connection_dropped messages received
        pub connection_dropped: u32,
        /// Counts the number of connection_closed messages received
        pub connection_closed: u32,
        /// Contains all `SystemPath`'s received in a `ConnectionEstablished`
        pub connected_systems: Vec<(SystemPath, SessionId)>,
        /// Contains all `SystemPath`'s received in a `ConnectionLost` or `ConnectionClosed` message
        pub disconnected_systems: Vec<(SystemPath, SessionId)>,
        /// The blocked SystemPaths
        pub blocked_systems: Vec<SystemPath>,
        /// The blocked ip addresses
        pub blocked_ip: Vec<IpAddr>,
        /// Counts the number of `SoftConnectionLimitExceeded` messages received
        pub soft_connection_limit_exceeded: u32,
        /// Counts the number of `HardConnectionLimitReached` messages received
        pub hard_connection_limit_reached: u32,
        /// Counts the number of network_out_of_buffers messages received
        pub network_out_of_buffers: u32,
        network_status_queue_sender: Option<Sender<NetworkStatus>>,
        started_promise: Option<KPromise<()>>,
    }

    impl NetworkStatusCounter {
        /// Creates a new uninitialised NetworkStatusCounter with all counters set to 0
        pub fn new() -> NetworkStatusCounter {
            Self {
                ctx: ComponentContext::uninitialised(),
                network_status_port: RequiredPort::<NetworkStatusPort>::uninitialised(),
                connection_established: 0,
                connection_lost: 0,
                connection_dropped: 0,
                connection_closed: 0,
                connected_systems: Vec::new(),
                disconnected_systems: Vec::new(),
                blocked_systems: Vec::new(),
                blocked_ip: Vec::new(),
                soft_connection_limit_exceeded: 0,
                hard_connection_limit_reached: 0,
                network_out_of_buffers: 0,
                network_status_queue_sender: None,
                started_promise: None,
            }
        }

        /// Sets a sender which the NetworkStatusCounter will foward all the NetworkStatus events to
        pub fn set_status_sender(&mut self, sender: Sender<NetworkStatus>) {
            self.network_status_queue_sender = Some(sender);
        }

        /// triggers the given `request` on the NetworkStatusPort
        pub fn send_status_request(&mut self, request: NetworkStatusRequest) {
            debug!(self.ctx.log(), "Sending Status Request. {:?}", request);
            self.network_status_port.trigger(request);
        }

        /// Creates a future that will be fulfilled when the component starts
        pub fn started_future(&mut self) -> KFuture<()> {
            let (promise, future) = promise();
            self.started_promise = Some(promise);
            future
        }
    }

    impl ComponentLifecycle for NetworkStatusCounter {
        fn on_start(&mut self) -> Handled {
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

    impl Actor for NetworkStatusCounter {
        type Message = ();

        fn receive_local(&mut self, _msg: Self::Message) -> Handled {
            unimplemented!()
        }

        fn receive_network(&mut self, _msg: NetMessage) -> Handled {
            unimplemented!("Ignoring network messages.");
        }
    }

    impl Require<NetworkStatusPort> for NetworkStatusCounter {
        fn handle(&mut self, event: <NetworkStatusPort as Port>::Indication) -> Handled {
            debug!(
                self.ctx.log(),
                "Got NetworkStatusPort indication. {:?}", event
            );
            if let Some(sender) = &self.network_status_queue_sender {
                sender
                    .send(event.clone())
                    .expect("StatusCounter to send NetworkStatus");
            }
            match event {
                NetworkStatus::ConnectionEstablished(system_path, session) => {
                    self.connection_established += 1;
                    self.connected_systems.push((system_path, session));
                }
                NetworkStatus::ConnectionLost(system_path, session) => {
                    self.connection_lost += 1;
                    self.disconnected_systems.push((system_path, session));
                }
                NetworkStatus::ConnectionDropped(_) => {
                    self.connection_dropped += 1;
                }
                NetworkStatus::ConnectionClosed(system_path, session) => {
                    self.connection_closed += 1;
                    self.disconnected_systems.push((system_path, session));
                }
                NetworkStatus::BlockedSystem(sys_path) => self.blocked_systems.push(sys_path),
                NetworkStatus::BlockedIp(ip_addr) => self.blocked_ip.push(ip_addr),
                NetworkStatus::UnblockedSystem(sys_path) => {
                    self.blocked_systems.retain(|s| s != &sys_path)
                }
                NetworkStatus::UnblockedIp(ip_addr) => self.blocked_ip.retain(|ip| ip != &ip_addr),
                NetworkStatus::SoftConnectionLimitExceeded => {
                    self.soft_connection_limit_exceeded += 1
                }
                NetworkStatus::HardConnectionLimitReached => {
                    self.hard_connection_limit_reached += 1
                }
            }
            Handled::Ok
        }
    }

    /// An actor that continuously sends `PingMsg` to a `target` over the network.
    /// Target should be a [PongerAct](PongerAct).
    #[derive(ComponentDefinition)]
    pub struct PingStream {
        ctx: ComponentContext<PingStream>,
        target: ActorPath,
        period: Duration,
        timer: Option<ScheduledTimer>,
        /// Contains all unique `(SystemPath, SessionId)`'s the PingStream has received Pong's from
        pub pong_system_paths: Vec<(SystemPath, SessionId)>,
        ping_count: u64,
        pong_count: u64,
    }

    impl PingStream {
        /// creates a `PingStream` actor that sends `PingMsg` to `target` every `period`
        /// Target should be a [PongerAct](PongerAct).
        pub fn new(target: ActorPath, period: Duration) -> Self {
            Self {
                ctx: ComponentContext::uninitialised(),
                target,
                period,
                timer: None,
                ping_count: 0,
                pong_count: 0,
                pong_system_paths: Vec::new(),
            }
        }

        /// method to start pinging
        pub fn start_pinging(&mut self) {
            let timer =
                self.schedule_periodic(Duration::from_millis(0), self.period, move |p, _| {
                    p.ping();
                    Handled::Ok
                });
            self.timer = Some(timer);
        }

        /// method to stop pinging
        pub fn stop_pinging(&mut self) {
            let timer = self.timer.take().expect("No timer");
            self.cancel_timer(timer);
        }

        fn ping(&mut self) {
            self.ping_count += 1;
            self.target
                .tell_serialised(PingMsg { i: self.ping_count }, self)
                .expect("serialise");
        }
    }

    impl ComponentLifecycle for PingStream {
        fn on_start(&mut self) -> Handled {
            debug!(self.ctx.log(), "Starting");
            self.start_pinging();
            Handled::Ok
        }
    }

    impl Actor for PingStream {
        type Message = Never;

        fn receive_local(&mut self, _: Self::Message) -> Handled {
            unimplemented!()
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
            let sender = msg.sender.clone();
            let session = msg.session;
            match msg.try_deserialise::<PongMsg, PingPongSer>() {
                Ok(pong) => {
                    debug!(self.ctx.log(), "Got msg {:?}", pong);
                    self.pong_count += 1;
                    if let Some(session_id) = session {
                        if !self
                            .pong_system_paths
                            .contains(&(sender.system().clone(), session_id))
                        {
                            self.pong_system_paths
                                .push((sender.system().clone(), session_id))
                        }
                    }
                }
                Err(e) => error!(self.ctx.log(), "Error deserialising PongMsg: {:?}", e),
            }
            Handled::Ok
        }
    }
}
