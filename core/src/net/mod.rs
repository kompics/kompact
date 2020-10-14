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
            SerialisedFrame::Chunk(chunk) => match protocol {
                Protocol::TCP => {
                    self.network_input_queue
                        .send(events::DispatchEvent::SendTCP(
                            addr,
                            SerialisedFrame::Chunk(chunk),
                        ))?;
                }
                Protocol::UDP => {
                    self.network_input_queue
                        .send(events::DispatchEvent::SendUDP(
                            addr,
                            SerialisedFrame::Chunk(chunk),
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
