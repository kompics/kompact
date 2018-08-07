use actors::ActorRef;
use actors::Transport;
use crossbeam::sync::ArcCell;
use dispatch::lookup::ActorStore;
use futures;
use futures::sync;
use futures::Future;
use futures::Stream;
use messaging::MsgEnvelope;
use net::events::NetworkError;
use net::events::NetworkEvent;
use serialisation::helpers::deserialise_msg;
use spnl::codec::FrameCodec;
use spnl::frames::Frame;
use std;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio::runtime::TaskExecutor;
use KompicsLogger;

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
    use spnl::frames::Frame;
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

/// Bridge to Tokio land, responsible for network connection management
pub struct Bridge {
    /// Executor belonging to the Tokio runtime
    pub(crate) executor: Option<TaskExecutor>,
    /// Queue of network events emitted by the network layer
    events: sync::mpsc::UnboundedSender<NetworkEvent>,
    /// Core logger; shared with network thread
    log: KompicsLogger,
    /// Shared actor refernce lookup table
    lookup: Arc<ArcCell<ActorStore>>,
    /// Thread blocking on the Tokio runtime
    net_thread: Option<JoinHandle<()>>,
    /// Reference back to the Kompics dispatcher
    dispatcher: Option<ActorRef>,
}

// impl bridge
impl Bridge {
    /// Creates a new [Bridge]
    ///
    /// # Returns
    /// A tuple consisting of the new Bridge object and the network event receiver.
    /// The receiver will allow responding to [NetworkEvent]s for external state management.
    pub fn new(
        lookup: Arc<ArcCell<ActorStore>>,
        log: KompicsLogger,
    ) -> (Self, sync::mpsc::UnboundedReceiver<NetworkEvent>) {
        let (sender, receiver) = sync::mpsc::unbounded();
        let bridge = Bridge {
            executor: None,
            events: sender,
            log,
            lookup,
            net_thread: None,
            dispatcher: None,
        };

        (bridge, receiver)
    }

    /// Sets the dispatcher reference, returning the previously stored one
    pub fn set_dispatcher(&mut self, dispatcher: ActorRef) -> Option<ActorRef> {
        std::mem::replace(&mut self.dispatcher, Some(dispatcher))
    }

    /// Starts the Tokio runtime and binds a [TcpListener] to the provided `addr`
    pub fn start(&mut self, addr: SocketAddr) {
        let network_log = self.log.new(o!("thread" => "tokio-runtime"));
        let (ex, th) = start_network_thread(
            "tokio-runtime".into(),
            network_log,
            addr,
            self.events.clone(),
            self.lookup.clone(),
        );
        self.executor = Some(ex);
        self.net_thread = Some(th);
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
                    let log = self.log.clone();
                    let connect_fut = TcpStream::connect(&addr)
                        .map_err(|_err| {
                            // TODO err
                            ()
                        })
                        .and_then(move |tcp_stream| {
                            let peer_addr = tcp_stream
                                .peer_addr()
                                .expect("stream must have a peer address");
                            let (tx, rx) = sync::mpsc::unbounded();
                            events.unbounded_send(NetworkEvent::Connection(
                                peer_addr,
                                ConnectionState::Connected(tx),
                            ));
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

/// Spawns a TCP server on the provided `TaskExecutor` and `addr`.
///
/// Connection result and errors are propagated on the provided `events`.
fn start_tcp_server(
    executor: TaskExecutor,
    log: KompicsLogger,
    addr: SocketAddr,
    events: sync::mpsc::UnboundedSender<NetworkEvent>,
    lookup: Arc<ArcCell<ActorStore>>,
) -> impl Future<Item = (), Error = ()> {
    let err_log = log.clone();
    let server = TcpListener::bind(&addr).expect("could not bind to address");
    let err_events = events.clone();
    let server = server
        .incoming()
        .map_err(move |e| {
            error!(err_log, "err listening on TCP socket");
            err_events.unbounded_send(NetworkEvent::Connection(addr, ConnectionState::Error(e)));
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
            events.unbounded_send(NetworkEvent::Connection(
                peer_addr,
                ConnectionState::Connected(tx),
            ));
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
    log: KompicsLogger,
    addr: SocketAddr,
    events: sync::mpsc::UnboundedSender<NetworkEvent>,
    lookup: Arc<ArcCell<ActorStore>>,
) -> (TaskExecutor, JoinHandle<()>) {
    use std::thread::Builder;

    let (tx, rx) = sync::oneshot::channel();
    let th = Builder::new()
        .name(name)
        .spawn(move || {
            let mut runtime = Runtime::new().expect("runtime creation in network main thread");
            let executor = runtime.executor();

            runtime.spawn(start_tcp_server(
                executor.clone(),
                log.clone(),
                addr,
                events,
                lookup,
            ));

            let _res = tx.send(executor);
            runtime.shutdown_on_idle().wait().unwrap();
        })
        .expect("TCP serve thread spawning should not fail");
    let executor = rx
        .wait()
        .expect("could not receive executor clone; did network thread panic?");

    (executor, th)
}

/// Returns a future which drives TCP I/O over the [FrameCodec].
///
/// `tx` can be used to relay network events back into the [Bridge]
fn handle_tcp(
    stream: TcpStream,
    rx: sync::mpsc::UnboundedReceiver<Frame>,
    _tx: futures::sync::mpsc::UnboundedSender<NetworkEvent>,
    log: KompicsLogger,
    actor_lookup: Arc<ArcCell<ActorStore>>,
) -> impl Future<Item = (), Error = ()> {
    use futures::Stream;

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
                    let lookup = actor_lookup.get();

                    let (actor, envelope ) = {
                        use bytes::IntoBuf;
                        use dispatch::lookup::ActorLookup;

                        // Consume payload
                        let buf = fr.payload();
                        let mut buf = buf.into_buf();
                        let envelope = deserialise_msg(buf).expect("s11n errors");
                        let actor_ref = match lookup.get_by_actor_path(envelope.dst()) {
                            None => {
                                panic!("Could not find actor reference for destination: {:?}", envelope.dst());
                            },
                            Some(actor) => actor,
                        };

                        (actor_ref, envelope)
                    };
                    debug!(log, "Routing envelope {:?}", envelope);
                    actor.enqueue(MsgEnvelope::Receive(envelope));
                    None
                }
                Frame::Ping(s, id) => {
                    Some(Frame::Pong(s, id))
                }
                Frame::Pong(_, _) => None,
                Frame::Unknown => {
                    None
                }
            }
        })
        .select(
            // Combine above stream with RX side of MPSC channel
            rx
                .map_err(|_err| {
                    () // TODO err
                })
                .map(|frame: Frame| {
                    Some(frame)
                })
        )
        // Forward the combined streams to the frame writer
        .forward(frame_writer)
        .map_err(|_res| {
            // TODO
            ()
        })
        .then(|_| {
            // Connection terminated
            Ok(())
        })
}
