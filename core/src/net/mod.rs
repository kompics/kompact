use super::*;
use actors::ActorRef;
use actors::Transport;
use arc_swap::ArcSwap;
use dispatch::lookup::ActorStore;
use futures;
use futures::stream::Stream;
use futures::sync;
use futures::Future;
use messaging::MsgEnvelope;
use net::events::NetworkError;
use net::events::NetworkEvent;
use serialisation::helpers::deserialise_msg;
use spaniel::codec::FrameCodec;
use spaniel::frames::Frame;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::runtime::TaskExecutor;
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};
use tokio_retry;
use tokio_retry::strategy::ExponentialBackoff;

use tokio::net::tcp::ConnectFuture;
use tokio_retry::Retry;

#[derive(Debug)]
pub enum ConnectionState {
    New,
    Initializing,
    Connected(sync::mpsc::UnboundedSender<Frame>),
    Closed,
    Error(std::io::Error),
}

pub mod events {
    use std;

    use super::ConnectionState;
    use spaniel::frames::Frame;
    use std::net::SocketAddr;

    /// Network events emitted by the network `Bridge`
    #[derive(Debug)]
    pub enum NetworkEvent {
        Connection(SocketAddr, ConnectionState),
        Data(Frame),
    }

    #[derive(Debug)]
    pub enum NetworkError {
        UnsupportedProtocol,
        MissingExecutor,
        Io(std::io::Error),
    }

    impl From<std::io::Error> for NetworkError {
        fn from(other: std::io::Error) -> Self {
            NetworkError::Io(other)
        }
    }
}

pub struct BridgeConfig {
    retry_strategy: RetryStrategy,
}

impl BridgeConfig {
    pub fn new() -> Self {
        BridgeConfig::default()
    }
}

enum RetryStrategy {
    ExponentialBackoff { base_ms: u64, num_tries: usize },
}

impl Default for BridgeConfig {
    fn default() -> Self {
        let retry_strategy = RetryStrategy::ExponentialBackoff {
            base_ms: 100,
            num_tries: 5,
        };
        BridgeConfig { retry_strategy }
    }
}

/// Bridge to Tokio land, responsible for network connection management
pub struct Bridge {
    /// Network-specific configuration
    cfg: BridgeConfig,
    /// Executor belonging to the Tokio runtime
    pub(crate) executor: Option<TaskExecutor>,
    /// Queue of network events emitted by the network layer
    events: sync::mpsc::UnboundedSender<NetworkEvent>,
    /// Core logger; shared with network thread
    log: KompactLogger,
    /// Shared actor reference lookup table
    lookup: Arc<ArcSwap<ActorStore>>,
    /// Tokio Runtime
    tokio_runtime: Option<Runtime>,
    /// Reference back to the Kompact dispatcher
    dispatcher: Option<ActorRef>,
    /// Socket the network actually bound on
    bound_addr: Option<SocketAddr>,
}

// impl bridge
impl Bridge {
    /// Creates a new [Bridge]
    ///
    /// # Returns
    /// A tuple consisting of the new Bridge object and the network event receiver.
    /// The receiver will allow responding to [NetworkEvent]s for external state management.
    pub fn new(
        lookup: Arc<ArcSwap<ActorStore>>,
        log: KompactLogger,
    ) -> (Self, sync::mpsc::UnboundedReceiver<NetworkEvent>) {
        let (sender, receiver) = sync::mpsc::unbounded();
        let bridge = Bridge {
            cfg: BridgeConfig::default(),
            executor: None,
            events: sender,
            log,
            lookup,
            tokio_runtime: None,
            dispatcher: None,
            bound_addr: None,
        };

        (bridge, receiver)
    }

    /// Sets the dispatcher reference, returning the previously stored one
    pub fn set_dispatcher(&mut self, dispatcher: ActorRef) -> Option<ActorRef> {
        std::mem::replace(&mut self.dispatcher, Some(dispatcher))
    }

    /// Starts the Tokio runtime and binds a [TcpListener] to the provided `addr`
    pub fn start(&mut self, addr: SocketAddr) -> Result<(), NetworkBridgeErr> {
        let network_log = self.log.new(o!("thread" => "tokio-runtime"));
        let nthread = start_network_thread(
            "tokio-runtime".into(),
            network_log,
            addr,
            self.events.clone(),
            self.lookup.clone(),
        )?;
        let executor = nthread.tokio_runtime.executor();
        self.tokio_runtime = Some(nthread.tokio_runtime);
        self.executor = Some(executor);
        self.bound_addr = Some(nthread.bound_addr);
        Ok(())
    }

    pub fn stop(mut self) -> Result<(), NetworkBridgeErr> {
        debug!(self.log, "Stopping NetworkBridge...");
        if let Some(runtime) = self.tokio_runtime.take() {
            let _ = self.executor.take();
            runtime.shutdown_now().wait().map_err(|_| {
                NetworkBridgeErr::Other("Tokio runtime failed to shut down!".to_string())
            })?;
        }
        debug!(self.log, "Stopped NetworkBridge.");
        Ok(())
    }

    pub fn local_addr(&self) -> &Option<SocketAddr> {
        &self.bound_addr
    }

    /// Attempts to establish a TCP connection to the provided `addr`.
    ///
    /// # Side effects
    /// When the connection is successul:
    ///     - a `ConnectionState::Connected` is dispatched on the network bridge event queue
    ///     - a new task is spawned on the Tokio runtime for driving the TCP connection's  I/O
    ///
    /// # Errors
    /// If the provided protocol is not supported or if the Tokio runtime's executor has not been
    /// set.
    pub fn connect(&mut self, proto: Transport, addr: SocketAddr) -> Result<(), NetworkError> {
        match proto {
            Transport::TCP => {
                if let Some(ref executor) = self.executor {
                    let lookup = self.lookup.clone();
                    let events = self.events.clone();
                    let err_events = events.clone();
                    let log = self.log.clone();
                    let err_log = log.clone();
                    let err_log2 = log.clone();

                    let retry_strategy = match self.cfg.retry_strategy {
                        RetryStrategy::ExponentialBackoff { base_ms, num_tries } => {
                            use tokio_retry::strategy::jitter;
                            ExponentialBackoff::from_millis(base_ms)
                                .map(jitter)
                                .take(num_tries)
                        }
                    };

                    let connect_fut = Retry::spawn(retry_strategy, TcpConnecter(addr.clone()))
                        .map_err(move |why| match why {
                            tokio_retry::Error::TimerError(terr) => {
                                error!(err_log, "TimerError connecting to {:?}: {:?}", addr, terr);
                            }
                            tokio_retry::Error::OperationError(err) => {
                                err_events.unbounded_send(NetworkEvent::Connection(
                                    addr,
                                    ConnectionState::Error(err),
                                )).unwrap_or_else(|e| {
                                    error!(err_log, "OperationError connecting to {:?} could not be sent: {:?}", addr, e);
                                });
                            }
                        })
                        .and_then(move |tcp_stream| {
                            let peer_addr = tcp_stream
                                .peer_addr()
                                .expect("stream must have a peer address");
                            let (tx, rx) = sync::mpsc::unbounded();
                            events.unbounded_send(NetworkEvent::Connection(
                                peer_addr,
                                ConnectionState::Connected(tx),
                            )).unwrap_or_else(|e| {
                                error!(err_log2,"Connected to {:?} could not be sent: {:?}", peer_addr, e);
                            });
                            handle_tcp(tcp_stream, rx, events.clone(), log, lookup)
                        });
                    executor.spawn(connect_fut);
                    Ok(())
                } else {
                    Err(NetworkError::MissingExecutor)
                }
            }
            _other => Err(NetworkError::UnsupportedProtocol),
        }
    }
}

struct TcpConnecter(SocketAddr);

impl tokio_retry::Action for TcpConnecter {
    type Future = ConnectFuture;
    type Item = TcpStream;
    type Error = std::io::Error;

    fn run(&mut self) -> Self::Future {
        TcpStream::connect(&self.0)
    }
}

/// Spawns a TCP server on the provided `TaskExecutor` and `addr`.
///
/// Connection result and errors are propagated on the provided `events`.
fn start_tcp_server(
    executor: TaskExecutor,
    log: KompactLogger,
    addr: SocketAddr,
    addr_tx: sync::oneshot::Sender<SocketAddr>,
    events: sync::mpsc::UnboundedSender<NetworkEvent>,
    lookup: Arc<ArcSwap<ActorStore>>,
) -> impl Future<Item = (), Error = ()> {
    let err_log = log.clone();
    let err_log2 = log.clone();
    let server_listener = TcpListener::bind(&addr).expect("could not bind to address");
    let actual_addr = server_listener
        .local_addr()
        .expect("could not get real address");
    addr_tx
        .send(actual_addr)
        .expect("could not communicate real address");
    let err_events = events.clone();
    let server = server_listener
        .incoming()
        .map_err(move |e| {
            error!(err_log, "err listening on TCP socket");
            err_events
                .unbounded_send(NetworkEvent::Connection(addr, ConnectionState::Error(e)))
                .unwrap_or_else(|e| {
                    error!(
                        err_log,
                        "Connection error to {:?} could not be sent: {:?}", addr, e
                    );
                });
        })
        .for_each(move |tcp_stream| {
            debug!(log, "connected TCP client at {:?}", tcp_stream);
            let lookup = lookup.clone();
            let peer_addr = tcp_stream
                .peer_addr()
                .expect("stream must have a peer address");
            let (tx, rx) = sync::mpsc::unbounded();
            executor.spawn(handle_tcp(
                tcp_stream,
                rx,
                events.clone(),
                log.clone(),
                lookup,
            ));
            events
                .unbounded_send(NetworkEvent::Connection(
                    peer_addr,
                    ConnectionState::Connected(tx),
                ))
                .unwrap_or_else(|e| {
                    error!(
                        err_log2,
                        "Connected to {:?} could not be sent: {:?}", peer_addr, e
                    );
                });
            Ok(())
        });
    server
}

/// Spawns a new thread responsible for driving the Tokio runtime to completion.
///
/// # Returns
/// A tuple consisting of the runtime's executor and a handle to the network thread.
fn start_network_thread(
    name: String,
    log: KompactLogger,
    addr: SocketAddr,
    events: sync::mpsc::UnboundedSender<NetworkEvent>,
    lookup: Arc<ArcSwap<ActorStore>>,
) -> Result<NetworkThread, NetworkBridgeErr> {
    let (addr_tx, addr_rx) = sync::oneshot::channel();
    let mut runtime = RuntimeBuilder::new()
        .name_prefix(&name)
        .core_threads(1)
        .build()
        .map_err(|e| NetworkBridgeErr::Tokio(e))?;

    //Runtime::new().map_err(|e| NetworkBridgeErr::Tokio(e))?;
    runtime.spawn(start_tcp_server(
        runtime.executor(),
        log.clone(),
        addr,
        addr_tx,
        events,
        lookup,
    ));

    //let (shutdown_tx, shutdown_rx) = sync::oneshot::channel::<()>();
    // let th = Builder::new()
    //     .name(name)
    //     .spawn(move || {
    //         let mut runtime = Runtime::new().expect("Could not create Tokio Runtime!");
    //         let executor = runtime.executor();

    //         runtime.spawn(start_tcp_server(
    //             executor.clone(),
    //             log.clone(),
    //             addr,
    //             addr_tx,
    //             events,
    //             lookup,
    //         ));
    //         tx.send(executor).expect("Onceshot channel died!");
    //         //runtime.shutdown_on_idle().wait().unwrap();
    //         // shutdown_rx.wait().expect("Unable to wait for receiver");
    //         // runtime
    //         //     .shutdown_now()
    //         //     .wait()
    //         //     .expect("Unable to wait for shutdown");
    //         runtime
    //             .block_on(shutdown_rx)
    //             .expect("Unable to shutdown tokio runtime!");
    //     })
    //     .map_err(|_| NetworkBridgeErr::Thread("TCP server thread spawning failed!".to_string()))?;

    let bound_addr = addr_rx.wait().map_err(|_| {
        NetworkBridgeErr::Binding(format!("Could not get true bind address for {}", addr))
    })?;
    // let executor = rx.wait().map_err(|_| {
    //     NetworkBridgeErr::Thread(
    //         "Could not receive executor; network thread may have panicked!".to_string(),
    //     )
    // })?;
    Ok(NetworkThread {
        tokio_runtime: runtime,
        bound_addr,
    })
}

struct NetworkThread {
    tokio_runtime: Runtime,
    bound_addr: SocketAddr,
}

#[derive(Debug)]
pub enum NetworkBridgeErr {
    Tokio(tokio::io::Error),
    Binding(String),
    Thread(String),
    Other(String),
}

/// Returns a future which drives TCP I/O over the [FrameCodec].
///
/// `tx` can be used to relay network events back into the [Bridge]
fn handle_tcp(
    stream: TcpStream,
    rx: sync::mpsc::UnboundedReceiver<Frame>,
    _tx: futures::sync::mpsc::UnboundedSender<NetworkEvent>,
    log: KompactLogger,
    actor_lookup: Arc<ArcSwap<ActorStore>>,
) -> impl Future<Item = (), Error = ()> {
    let err_log = log.clone();
    let err_log2 = log.clone();

    let transport = FrameCodec::new(stream);
    let (frame_writer, frame_reader) = transport.split();

    frame_reader
        .map(move |frame: Frame| {
            // Handle frame from protocol; yielding `Some(Frame)` if a response `Frame` should be sent
            match frame {
                Frame::StreamRequest(_) => None,
                Frame::CreditUpdate(_) => None,
                Frame::Data(fr) => {
                    // TODO Assert expected from stream & conn?
                    let _seq_num = fr.seq_num;
                    let lookup = actor_lookup.lease();

                    {
                        use bytes::IntoBuf;
                        use dispatch::lookup::ActorLookup;

                        // Consume payload
                        let buf = fr.payload();
                        let buf = buf.into_buf();
                        let envelope = deserialise_msg(buf).expect("s11n errors");
                        match lookup.get_by_actor_path(envelope.dst()) {
                            None => {
                                error!(
                                    log,
                                    "Could not find actor reference for destination: {:?}",
                                    envelope.dst()
                                );
                                None
                            }
                            Some(actor) => {
                                debug!(log, "Routing envelope {:?}", envelope);
                                actor.enqueue(MsgEnvelope::Receive(envelope));
                                None
                            }
                        }
                    }
                }
                Frame::Ping(s, id) => Some(Frame::Pong(s, id)),
                Frame::Pong(_, _) => None,
                Frame::Unknown => None,
            }
        })
        .select(
            // Combine above stream with RX side of MPSC channel
            rx.map_err(move |err| {
                error!(err_log, "ERROR receiving MPSC: {:?}", err);
                ()
            })
            .map(|frame: Frame| Some(frame)),
        )
        // Forward the combined streams to the frame writer
        .forward(frame_writer)
        .map_err(move |err| {
            error!(err_log2, "TCP conn err: {:?}", err);
            ()
        })
        .then(|_| {
            // Connection terminated
            Ok(())
        })
}
