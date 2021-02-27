use super::*;
use crate::{
    messaging::{DispatchData, DispatchEnvelope, MsgEnvelope, SerialisedFrame},
    net::buffers::ChunkRef,
};
use std::{
    convert::TryFrom,
    error::Error,
    fmt::{self, Debug},
    net::{AddrParseError, IpAddr, SocketAddr},
    ops::Div,
    str::FromStr,
};
use uuid::Uuid;

/// Transport protocol to use for delivering messages
/// sent to an [ActorPath](ActorPath)
///
/// # Note
///
/// Dispatcher implementations are not required to implement all protocols.
/// Check your concrete implementation, before selecting an arbitrary protocol.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Transport {
    /// Local reflection only, no network messages involved
    Local = 0b00,
    /// Send messages over TCP
    Tcp = 0b01,
    /// Send messages as UDP datagrams
    Udp = 0b10,
}

impl Transport {
    /// Returns `true` if this is an instance of [Transport::Local](Transport::Local)
    pub fn is_local(&self) -> bool {
        matches!(*self, Transport::Local)
    }

    /// Returns `true` if this is *not* an instance of [Transport::Local](Transport::Local)
    pub fn is_remote(&self) -> bool {
        !self.is_local()
    }
}

impl fmt::Display for Transport {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            &Transport::Local => write!(fmt, "local"),
            &Transport::Tcp => write!(fmt, "tcp"),
            &Transport::Udp => write!(fmt, "udp"),
        }
    }
}

impl FromStr for Transport {
    type Err = TransportParseError;

    fn from_str(s: &str) -> Result<Transport, TransportParseError> {
        match s {
            "local" => Ok(Transport::Local),
            "tcp" => Ok(Transport::Tcp),
            "udp" => Ok(Transport::Udp),
            _ => Err(TransportParseError),
        }
    }
}

/// Error type for parsing the [Transport](Transport) from a string
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct TransportParseError;

impl fmt::Display for TransportParseError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.write_str("TransportParseError")
    }
}

impl Error for TransportParseError {
    fn description(&self) -> &str {
        "Transport must be one of [local,tcp,udp]"
    }
}

/// Error type for parsing [paths](ActorPath) from a string
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PathParseError {
    /// The format is wrong
    Form(String),
    /// The transport protocol was invalid
    Transport(TransportParseError),
    /// The network address was invalid
    Addr(AddrParseError),
    /// The path contains a disallowed character
    IllegalCharacter(char),
}

impl fmt::Display for PathParseError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathParseError::Form(s) => write!(fmt, "Invalid formatting: {}", s),
            PathParseError::Transport(e) => write!(fmt, "Could not parse transport: {}", e),
            PathParseError::Addr(e) => write!(fmt, "Could not parse address: {}", e),
            PathParseError::IllegalCharacter(c) => {
                write!(fmt, "The path contains an illegal character: {}", c)
            }
        }
    }
}

impl Error for PathParseError {
    fn description(&self) -> &str {
        "Path could not be parsed"
    }

    fn cause(&self) -> Option<&dyn Error> {
        match self {
            &PathParseError::Form(_) => None,
            &PathParseError::Transport(ref e) => Some(e),
            &PathParseError::Addr(ref e) => Some(e),
            &PathParseError::IllegalCharacter(_) => None,
        }
    }
}

impl From<TransportParseError> for PathParseError {
    fn from(e: TransportParseError) -> PathParseError {
        PathParseError::Transport(e)
    }
}

impl From<AddrParseError> for PathParseError {
    fn from(e: AddrParseError) -> PathParseError {
        PathParseError::Addr(e)
    }
}

/// The part of an [ActorPath](ActorPath) that refers to the [KompactSystem](KompactSystem)
///
/// As a URI, a `SystemPath` looks like `"tcp://127.0.0.1:8080"`, for example.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SystemPath {
    protocol: Transport,
    // TODO address could be IPv4, IPv6, or a domain name (not supported yet)
    address: IpAddr,
    port: u16,
}

impl SystemPath {
    /// Construct a new system path from individual parts
    pub fn new(protocol: Transport, address: IpAddr, port: u16) -> SystemPath {
        SystemPath {
            protocol,
            address,
            port,
        }
    }

    /// Construct a new system path from individual parts using a [SocketAddr](std::net::SocketAddr)
    pub fn with_socket(protocol: Transport, socket: SocketAddr) -> SystemPath {
        SystemPath {
            protocol,
            address: socket.ip(),
            port: socket.port(),
        }
    }

    /// Returns a reference to the [Transport](Transport) protocol associated with with this system path
    pub fn protocol(&self) -> Transport {
        self.protocol
    }

    /// Returns a reference to the IP address associated with with this system path
    pub fn address(&self) -> &IpAddr {
        &self.address
    }

    /// Returns the port associated with with this system path
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Create a named path starting with this system path and ending with the given string
    ///
    /// Paths created with this function will be validated to be a valid lookup path,
    /// and an error will be returned if they are not.
    pub fn into_named_with_string(self, path: &str) -> Result<NamedPath, PathParseError> {
        let parsed = parse_path(path);
        self.into_named_with_vec(parsed)
    }

    /// Returns the SocketAddr corresponding to the SystemPath
    pub fn socket_address(&self) -> SocketAddr {
        SocketAddr::new(self.address, self.port)
    }

    /// Create a named path starting with this system path and ending with the sequence of path segments
    ///
    /// Paths created with this function will be validated to be a valid lookup path,
    /// and an error will be returned if they are not.
    pub fn into_named_with_vec(self, path: Vec<String>) -> Result<NamedPath, PathParseError> {
        validate_lookup_path(&path)?;
        let named = NamedPath::with_system(self, path);
        Ok(named)
    }

    /// Create a unique path starting with this system path and ending with the given `id`
    pub fn into_unique(self, id: Uuid) -> UniquePath {
        UniquePath::with_system(self, id)
    }
}

impl fmt::Display for SystemPath {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "{}://{}:{}", self.protocol, self.address, self.port)
    }
}

/// Methods for things that contain a [SystemPath](SystemPath)
pub trait SystemField {
    /// Returns a reference to the system path
    fn system(&self) -> &SystemPath;

    /// Returns the transport protocol used in the system path
    fn protocol(&self) -> Transport {
        self.system().protocol()
    }

    /// Returns the address used in the system path
    fn address(&self) -> &IpAddr {
        &self.system().address()
    }

    /// Returns the port used in the system path
    fn port(&self) -> u16 {
        self.system().port()
    }
}

impl SystemField for SystemPath {
    fn system(&self) -> &SystemPath {
        self
    }
}

/// A factory trait for things that have associated [actor paths](ActorPath)
pub trait ActorPathFactory {
    /// Returns the associated actor path
    fn actor_path(&self) -> ActorPath;
}

/// A temporary combination of an [ActorPath](ActorPath)
/// and something that can [dispatch](Dispatching) stuff
///
/// This can be used when you want to use a different actor path
/// than the one of the current component for a [tell](ActorPath::tell)
/// but still need to use the component's dispatcher for the message.
/// This is useful for forwarding components, for example, when trying to
/// preserve the original sender.
///
/// See also [using_dispatcher](ActorPath::using_dispatcher).
pub struct DispatchingPath<'a, 'b> {
    path: &'a ActorPath,
    ctx: &'b dyn Dispatching,
}

impl<'a, 'b> Dispatching for DispatchingPath<'a, 'b> {
    fn dispatcher_ref(&self) -> DispatcherRef {
        self.ctx.dispatcher_ref()
    }
}

impl<'a, 'b> ActorPathFactory for DispatchingPath<'a, 'b> {
    fn actor_path(&self) -> ActorPath {
        self.path.clone()
    }
}

/// An actor path is a serialisable, possibly remote reference to
/// an actor
///
/// Any message sent via an actor path *might* go over the network,
/// and must be treated as fallible.
/// It must also be [serialisable](Serialisable).
#[derive(Clone, Debug)]
#[repr(u8)]
#[derive(PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ActorPath {
    /// A unique actor path identifies a concrete instance of an actor
    ///
    /// Unique actor paths use the component's unique id internally.
    ///
    /// Unique actor paths become invalid when a component is
    /// replaced with a new instance of the same type.
    ///
    /// A unique path may look something like `"tcp://127.0.0.1:8080#1e555f40-de1d-4aee-8202-64fdc27edfa8"`, for example.
    Unique(UniquePath),
    /// A named actor path identifies a service, rather than a concrete actor instance
    ///
    /// Named paths must be [registered](KompactSystem::register_by_alias) to a particular actor,
    /// and their registration can be changed over time,
    /// as actors fail and are replaced, for example.
    ///
    /// Named paths may be described hierarchically, similar to URLs.
    ///
    /// A named path may look something like `"tcp://127.0.0.1:8080/my-actor-group/my-actor"`, for example.
    Named(NamedPath),
}

impl ActorPath {
    /// Send message `m` to the actor designated by this path
    ///
    /// The `from` field is used as a source,
    /// and the `ActorPath` it resolved to will be supplied at the destination.
    ///
    /// This function ensures that the protocol of the source path matches the
    /// protocol of this path. If you would like the sender path to have a different
    /// transport protocol, use [tell_with_sender](ActorPath::tell_with_sender) instead.
    ///
    /// Serialisation of `m` happens lazily in the dispatcher,
    /// and only if it really goes over the network. If this actor path
    /// turns out to be local, `m` will be moved to the heap and send locally instead.
    ///
    /// In fact, this method always moves `m` onto the heap (unless it already is allocated there),
    /// to facilitate this lazy serialisation.
    /// As this method has some overhead in the case where it is certain
    /// that `m` will definitely go over the network, you can use
    /// [tell_serialised](ActorPath::tell_serialised) to force eager serialisation instead.
    pub fn tell<S, B>(&self, m: B, from: &S) -> ()
    where
        S: ActorPathFactory + Dispatching,
        B: Into<Box<dyn Serialisable>>,
    {
        let mut src = from.actor_path();
        src.set_protocol(self.protocol());
        self.tell_with_sender(m, from, src)
    }

    /// Send message `m` to the actor designated by this path
    ///
    /// The `from` field is used as a source and will be supplied at the destination.
    ///
    /// Serialisation of `m` happens lazily in the dispatcher,
    /// and only if it really goes over the network. If this actor path
    /// turns out to be local, `m` will be moved to the heap and send locally instead.
    ///
    /// In fact, this method always moves `m` onto the heap (unless it already is allocated there),
    /// to facilitate this lazy serialisation.
    /// As this method has some overhead in the case where it is certain
    /// that `m` will definitely go over the network, you can use
    /// [tell_serialised](ActorPath::tell_serialised) to force eager serialisation instead.
    pub fn tell_with_sender<B, D>(&self, m: B, dispatch: &D, from: ActorPath) -> ()
    where
        B: Into<Box<dyn Serialisable>>,
        D: Dispatching,
    {
        let msg: Box<dyn Serialisable> = m.into();
        let dst = self.clone();
        let env = DispatchEnvelope::Msg {
            src: from.clone(),
            dst: dst.clone(),
            msg: DispatchData::Lazy(msg, from, dst),
        };
        dispatch.dispatcher_ref().enqueue(MsgEnvelope::Typed(env))
    }

    /// Send message `m` to the actor designated by this path
    ///
    /// This function has the same effect as [tell](ActorPath::tell),
    /// but serialises eagerly into a Pooled buffer (pre-allocated and bounded).
    pub fn tell_serialised<CD, B>(&self, m: B, from: &CD) -> Result<(), SerError>
    where
        CD: ComponentTraits + ComponentLifecycle,
        B: Serialisable + 'static,
    {
        let mut src = from.actor_path();
        src.set_protocol(self.protocol());
        self.tell_serialised_with_sender(m, from, src)
    }

    /// Send message `m` to the actor designated by this path
    ///
    /// This function has the same effect as [tell](ActorPath::tell),
    /// but serialises eagerly into a Pooled buffer (pre-allocated and bounded).
    pub fn tell_serialised_with_sender<CD, B>(
        &self,
        m: B,
        dispatch: &CD,
        from: ActorPath,
    ) -> Result<(), SerError>
    where
        CD: ComponentTraits + ComponentLifecycle,
        B: Serialisable + 'static,
    {
        if self.protocol() == Transport::Local {
            // No need to serialize!
            self.tell_with_sender(m, dispatch, from);
            Ok(())
        } else {
            dispatch.ctx().with_buffer(|buffer| {
                let mut buf = buffer.get_buffer_encoder()?;
                let msg =
                    crate::serialisation::ser_helpers::serialise_msg(&from, &self, &m, &mut buf)?;
                let env = DispatchEnvelope::Msg {
                    src: from,
                    dst: self.clone(),
                    msg: DispatchData::Serialised(SerialisedFrame::ChunkLease(msg)),
                };
                dispatch.dispatcher_ref().enqueue(MsgEnvelope::Typed(env));
                Ok(())
            })
        }
    }

    /// Send prepared message `content` to the actor designated by this path
    ///
    /// This function has the same effect as [tell_serialised](ActorPath::tell_serialised), but
    /// requires the content to be pre-serialised into a [ChunkRef](net::buffers::ChunkRef).
    ///
    /// Use [self.ctx.preserialise(m)](component::ComponentContext#preserialise)
    /// to preserialise data into a `ChunkRef`
    pub fn tell_preserialised<CD>(&self, content: ChunkRef, from: &CD) -> Result<(), SerError>
    where
        CD: ComponentTraits + ComponentLifecycle,
    {
        let mut src = from.actor_path();
        src.set_protocol(self.protocol());
        self.tell_preserialised_with_sender(content, from, src)
    }

    /// Send preserialised message `content` to the actor designated by this path with specified `from`
    ///
    /// This function has the same effect as [tell_serialised](ActorPath::tell_serialised), but
    /// requires the content to be pre-serialised into a [ChunkRef](net::buffers::ChunkRef).
    ///
    /// Use [self.ctx.preserialise(m)](component::ComponentContext#preserialise) to preserialise data into a `ChunkRef`
    pub fn tell_preserialised_with_sender<CD: ComponentTraits + ComponentLifecycle>(
        &self,
        content: ChunkRef,
        dispatch: &CD,
        from: ActorPath,
    ) -> Result<(), SerError> {
        dispatch.ctx().with_buffer(|buffer| {
            let mut buf = buffer.get_buffer_encoder()?;
            let msg = crate::serialisation::ser_helpers::serialise_msg_with_preserialised(
                &from, &self, content, &mut buf,
            )?;
            let env = DispatchEnvelope::Msg {
                src: from,
                dst: self.clone(),
                msg: DispatchData::Serialised(SerialisedFrame::ChunkRef(msg)),
            };
            dispatch.dispatcher_ref().enqueue(MsgEnvelope::Typed(env));
            Ok(())
        })
    }

    /// Forwards the still serialised message to this path without changing the sender
    ///
    /// This can be used for routing protocls where the final recipient is supposed to reply
    /// to the original sender, not the intermediaries.
    pub fn forward_with_original_sender<D>(
        &self,
        mut serialised_message: NetMessage,
        dispatcher: &D,
    ) -> ()
    where
        D: Dispatching,
    {
        serialised_message.receiver = self.clone();
        let env = DispatchEnvelope::ForwardedMsg {
            msg: serialised_message,
        };
        dispatcher.dispatcher_ref().enqueue(MsgEnvelope::Typed(env));
    }

    /// Forwards the still serialised message to this path replacing the sender with the given one
    pub fn forward_with_sender<D>(
        &self,
        mut serialised_message: NetMessage,
        dispatch: &D,
        from: ActorPath,
    ) -> ()
    where
        D: Dispatching,
    {
        serialised_message.receiver = self.clone();
        serialised_message.sender = from;
        let env = DispatchEnvelope::ForwardedMsg {
            msg: serialised_message,
        };
        dispatch.dispatcher_ref().enqueue(MsgEnvelope::Typed(env));
    }

    /// Returns a temporary combination of an [ActorPath](ActorPath)
    /// and something that can [dispatch](Dispatching) stuff
    ///
    /// This can be used when you want to use a different actor path
    /// than the one of the current component for a [tell](ActorPath::tell)
    /// but still need to use the component's dispatcher for the message.
    /// This is useful for forwarding components, for example, when trying to
    /// preserve the original sender.
    pub fn using_dispatcher<'a, 'b>(
        &'a self,
        disp: &'b dyn Dispatching,
    ) -> DispatchingPath<'a, 'b> {
        DispatchingPath {
            path: self,
            ctx: disp,
        }
    }

    fn system_mut(&mut self) -> &mut SystemPath {
        match self {
            ActorPath::Unique(ref mut up) => up.system_mut(),
            ActorPath::Named(ref mut np) => np.system_mut(),
        }
    }

    /// Change the transport protocol for this actor path
    pub fn set_protocol(&mut self, proto: Transport) {
        self.system_mut().protocol = proto;
    }

    /// Sets the transport protocol for this actor path to UDP
    pub fn via_udp(&mut self) {
        self.set_protocol(Transport::Udp);
    }

    /// Sets the transport protocol for this actor path to TCP
    pub fn via_tcp(&mut self) {
        self.set_protocol(Transport::Tcp);
    }

    /// Sets the transport protocol for this actor path to LOCAL
    pub fn via_local(&mut self) {
        self.set_protocol(Transport::Local);
    }

    /// If this path is a named path, return the underlying named path
    ///
    /// Otherwise return `None`.
    pub fn named(self) -> Option<NamedPath> {
        match self {
            ActorPath::Named(p) => Some(p),
            _ => None,
        }
    }

    /// Return the underlying named path
    ///
    /// Panics if this is not actually a named path variant.
    pub fn unwrap_named(self) -> NamedPath {
        self.named().unwrap()
    }

    /// If this path is a unique path, return the underlying unique path
    ///
    /// Otherwise return `None`.
    pub fn unique(self) -> Option<UniquePath> {
        match self {
            ActorPath::Unique(p) => Some(p),
            _ => None,
        }
    }

    /// Return the underlying unique path
    ///
    /// Panics if this is not actually a uniqe path variant.
    pub fn unwrap_unique(self) -> UniquePath {
        self.unique().unwrap()
    }
}

impl SystemField for ActorPath {
    fn system(&self) -> &SystemPath {
        match self {
            &ActorPath::Unique(ref up) => up.system(),
            &ActorPath::Named(ref np) => np.system(),
        }
    }
}

impl From<(SystemPath, Uuid)> for ActorPath {
    fn from(t: (SystemPath, Uuid)) -> ActorPath {
        ActorPath::Unique(UniquePath {
            system: t.0,
            id: t.1,
        })
    }
}

impl From<UniquePath> for ActorPath {
    fn from(p: UniquePath) -> ActorPath {
        ActorPath::Unique(p)
    }
}

impl From<NamedPath> for ActorPath {
    fn from(p: NamedPath) -> ActorPath {
        ActorPath::Named(p)
    }
}

/// Separator for segments in named actor paths
///
/// # Example
///
/// `tcp://127.0.0.1:8080/segment1/segment2`
pub const PATH_SEP: char = '/';
/// Separator for the id in unique actor paths
///
/// # Example
///
/// `tcp://127.0.0.1:8080#1e555f40-de1d-4aee-8202-64fdc27edfa8`
pub const UNIQUE_PATH_SEP: char = '#';
/// Marker for broadcast routing
///
/// # Example
///
/// `tcp://127.0.0.1:8080/segment1/*`
/// will route to all actors whose path starts with `segment1`.
pub const BROADCAST_MARKER: char = '*';
/// Marker for select routing
///
/// # Example
///
/// `tcp://127.0.0.1:8080/segment1/?`
/// will route to some actor whose path starts with `segment1`.
pub const SELECT_MARKER: char = '?';

impl fmt::Display for ActorPath {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            &ActorPath::Named(ref np) => {
                let path = np.path.iter().fold(String::new(), |mut acc, arg| {
                    acc.push(PATH_SEP);
                    acc.push_str(arg);
                    acc
                });
                write!(fmt, "{}{}", np.system, path)
            }
            &ActorPath::Unique(ref up) => write!(fmt, "{}{}{}", up.system, UNIQUE_PATH_SEP, up.id),
        }
    }
}

impl TryFrom<String> for ActorPath {
    type Error = PathParseError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let p = ActorPath::from_str(&s)?;
        Ok(p)
    }
}

impl FromStr for ActorPath {
    type Err = PathParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.contains(UNIQUE_PATH_SEP) {
            let p = UniquePath::from_str(s)?;
            Ok(ActorPath::Unique(p))
        } else {
            let p = NamedPath::from_str(s)?;
            Ok(ActorPath::Named(p))
        }
    }
}

/// A unique actor path identifies a concrete instance of an actor
///
/// Unique actor paths use the component's unique id internally.
///
/// Unique actor paths become invalid when a component is
/// replaced with a new instance of the same type.
///
/// A unique path may look something like `"tcp://127.0.0.1:8080#1e555f40-de1d-4aee-8202-64fdc27edfa8"`, for example.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UniquePath {
    system: SystemPath,
    id: Uuid,
}

impl UniquePath {
    /// Construct a new unique path from parts
    pub fn new(protocol: Transport, address: IpAddr, port: u16, id: Uuid) -> UniquePath {
        UniquePath {
            system: SystemPath::new(protocol, address, port),
            id,
        }
    }

    /// Construct a new unique path from a system path and an id
    pub fn with_system(system: SystemPath, id: Uuid) -> UniquePath {
        UniquePath { system, id }
    }

    /// Construct a new unique path with a [socket](std::net::SocketAddr) and an id
    pub fn with_socket(protocol: Transport, socket: SocketAddr, id: Uuid) -> UniquePath {
        UniquePath {
            system: SystemPath::with_socket(protocol, socket),
            id,
        }
    }

    /// Returns a reference to this path's unique id
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Returns a mutable reference to this path's system path part
    pub fn system_mut(&mut self) -> &mut SystemPath {
        &mut self.system
    }
}

impl TryFrom<String> for UniquePath {
    type Error = PathParseError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let p = UniquePath::from_str(&s)?;
        Ok(p)
    }
}

impl FromStr for UniquePath {
    type Err = PathParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split("://").collect();
        // parts: [tcp]://[IP:port#id]
        if parts.len() != 2 {
            return Err(PathParseError::Form(s.to_string()));
        }
        let proto: Transport = parts[0].parse()?;
        let parts: Vec<&str> = parts[1].split(UNIQUE_PATH_SEP).collect();
        // parts: [IP:port]#[UUID]
        if parts.len() != 2 {
            return Err(PathParseError::Form(s.to_string()));
        }
        let socket = SocketAddr::from_str(parts[0])?;
        let uuid =
            Uuid::from_str(parts[1]).map_err(|_parse_err| PathParseError::Form(s.to_string()))?;

        Ok(UniquePath::with_socket(proto, socket, uuid))
    }
}

impl SystemField for UniquePath {
    fn system(&self) -> &SystemPath {
        &self.system
    }
}

/// A named actor path identifies a service, rather than a concrete actor instance
///
/// Named paths must be [registered](KompactSystem::register_by_alias) to a particular actor,
/// and their registration can be changed over time,
/// as actors fail and are replaced, for example.
///
/// Named paths may be described hierarchically, similar to URLs.
///
/// A named path may look something like `"tcp://127.0.0.1:8080/my-actor-group/my-actor"`, for example.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NamedPath {
    system: SystemPath,
    path: Vec<String>,
}

impl NamedPath {
    /// Construct a new named path from parts
    ///
    /// Make sure that none of the elements of the `path` vector contain slashes (i.e., `/`),
    /// as those will be the path separators for the individual elements later.
    pub fn new(protocol: Transport, address: IpAddr, port: u16, path: Vec<String>) -> NamedPath {
        debug_assert!(
            crate::actors::validate_lookup_path(&path).is_ok(),
            "Path contains illegal characters: {:?}",
            path
        );
        NamedPath {
            system: SystemPath::new(protocol, address, port),
            path,
        }
    }

    /// Construct a new named path from a [socket](std::net::SocketAddr) and vector of path elements
    ///
    /// Make sure that none of the elements of the `path` vector contain slashes (i.e., `/`),
    /// as those will be the path separators for the individual elements later.
    pub fn with_socket(protocol: Transport, socket: SocketAddr, path: Vec<String>) -> NamedPath {
        debug_assert!(
            crate::actors::validate_lookup_path(&path).is_ok(),
            "Path contains illegal characters: {:?}",
            path
        );
        NamedPath {
            system: SystemPath::with_socket(protocol, socket),
            path,
        }
    }

    /// Construct a new named path from a system path and vector of path elements
    ///
    /// Make sure that none of the elements of the `path` vector contain slashes (i.e., `/`),
    /// as those will be the path separators for the individual elements later.
    pub fn with_system(system: SystemPath, path: Vec<String>) -> NamedPath {
        debug_assert!(
            crate::actors::validate_lookup_path(&path).is_ok(),
            "Path contains illegal characters: {:?}",
            path
        );
        NamedPath { system, path }
    }

    /// Returns a reference to the path vector
    pub fn path_ref(&self) -> &[String] {
        &self.path
    }

    /// Returns a copy to the path vector
    pub fn clone_path(&self) -> Vec<String> {
        self.path.clone()
    }

    /// Return just the path vector, discarding the SystemPath
    pub fn into_path(self) -> Vec<String> {
        self.path
    }

    /// Returns a mutable reference to this path's system path part
    pub fn system_mut(&mut self) -> &mut SystemPath {
        &mut self.system
    }

    /// Add the given `segment` to the path, if it is valid to do so
    pub fn push(&mut self, segment: String) -> Result<(), PathParseError> {
        if let Some(last_segment) = self.path.last() {
            validate_lookup_path_segment(last_segment, false)?;
        }
        validate_lookup_path_segment(&segment, true)?;
        self.path.push(segment);
        Ok(())
    }

    /// Add the given relative `path` to this path, if it is valid to do so
    pub fn append(mut self, path: &str) -> Result<Self, PathParseError> {
        if let Some(last_segment) = self.path.last() {
            validate_lookup_path_segment(last_segment, false)?;
        }
        let mut segments = parse_path(path);
        self.path.append(&mut segments);
        validate_lookup_path(&self.path)?;
        Ok(self)
    }
}

impl SystemField for NamedPath {
    fn system(&self) -> &SystemPath {
        &self.system
    }
}

impl TryFrom<String> for NamedPath {
    type Error = PathParseError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        NamedPath::from_str(&s)
    }
}

impl FromStr for NamedPath {
    type Err = PathParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s1: Vec<&str> = s.split("://").collect();
        if s1.len() != 2 {
            return Err(PathParseError::Form(s.to_string()));
        }
        let proto: Transport = s1[0].parse()?;
        let mut s2: Vec<&str> = s1[1].split(PATH_SEP).collect();
        if s2.is_empty() {
            return Err(PathParseError::Form(s.to_string()));
        }
        let socket = SocketAddr::from_str(s2[0])?;
        let path: Vec<String> = if s2.len() > 1 {
            s2.split_off(1).into_iter().map(|v| v.to_string()).collect()
        } else {
            Vec::default()
        };
        validate_lookup_path(&path)?;
        Ok(NamedPath::with_socket(proto, socket, path))
    }
}

/// Some syntactic sugar for [append](NamedPath::append)
///
/// Allows the pretty `path / "segment" / "*"` syntax
/// to create sub-paths of `path`.
impl Div<&str> for NamedPath {
    type Output = Self;

    fn div(self, rhs: &str) -> Self::Output {
        self.append(rhs).expect("illegal path")
    }
}
/// Some syntactic sugar for [push](NamedPath::push)
///
/// Allows the pretty `path / '*'` syntax
/// to create a broadcast variant of `path`.
impl Div<char> for NamedPath {
    type Output = Self;

    fn div(mut self, rhs: char) -> Self::Output {
        self.push(rhs.to_string()).expect("illegal path");
        self
    }
}

/// Split the `&str` into segments using [PATH_SEP](crate::constants::PATH_SEP)
pub fn parse_path(s: &str) -> Vec<String> {
    s.split(PATH_SEP)
        .into_iter()
        .map(|v| v.to_string())
        .collect()
}

/// Check that the given path is valid as a lookup path
///
/// Lookup paths must not contain the following characters:
/// - [PATH_SEP](crate::constants::PATH_SEP)
/// - [UNIQUE_PATH_SEP](crate::constants::UNIQUE_PATH_SEP)
/// - [BROADCAST_MARKER](crate::constants::BROADCAST_MARKER) unless as the only item in the last string
/// - [SELECT_MARKER](crate::constants::SELECT_MARKER) unless as the only item in the last string
pub fn validate_lookup_path(path: &[String]) -> Result<(), PathParseError> {
    let len = path.len();
    for (index, segment) in path.iter().enumerate() {
        validate_lookup_path_segment(segment, (index + 1) == len)?;
    }
    Ok(())
}

/// Check that the given path is valid as an insert path
///
/// Insert paths must not contain the following characters:
/// - [PATH_SEP](crate::constants::PATH_SEP)
/// - [UNIQUE_PATH_SEP](crate::constants::UNIQUE_PATH_SEP)
/// - [BROADCAST_MARKER](crate::constants::BROADCAST_MARKER)
/// - [SELECT_MARKER](crate::constants::SELECT_MARKER)
pub fn validate_insert_path(path: &[String]) -> Result<(), PathParseError> {
    for segment in path.iter() {
        validate_insert_path_segment(segment)?;
    }
    Ok(())
}

pub(crate) fn validate_lookup_path_segment(
    segment: &str,
    is_last: bool,
) -> Result<(), PathParseError> {
    if segment.is_empty() {
        return Err(PathParseError::Form(
            "Path segments may not be empty!".to_string(),
        ));
    }
    let len = segment.len();
    for c in segment.chars() {
        match c {
            PATH_SEP => return Err(PathParseError::IllegalCharacter(PATH_SEP)),
            BROADCAST_MARKER if !is_last && len == 1 => {
                return Err(PathParseError::IllegalCharacter(BROADCAST_MARKER))
            }
            SELECT_MARKER if !is_last && len == 1 => {
                return Err(PathParseError::IllegalCharacter(SELECT_MARKER))
            }
            UNIQUE_PATH_SEP => return Err(PathParseError::IllegalCharacter(UNIQUE_PATH_SEP)),
            _ => (), // ok
        }
    }
    Ok(())
}
pub(crate) fn validate_insert_path_segment(segment: &str) -> Result<(), PathParseError> {
    if segment.is_empty() {
        return Err(PathParseError::Form(
            "Path segments may not be empty!".to_string(),
        ));
    }
    for c in segment.chars() {
        match c {
            PATH_SEP => return Err(PathParseError::IllegalCharacter(PATH_SEP)),
            BROADCAST_MARKER => return Err(PathParseError::IllegalCharacter(BROADCAST_MARKER)),
            SELECT_MARKER => return Err(PathParseError::IllegalCharacter(SELECT_MARKER)),
            UNIQUE_PATH_SEP => return Err(PathParseError::IllegalCharacter(UNIQUE_PATH_SEP)),
            _ => (), // ok
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "local://127.0.0.1:0/test_actor";

    #[test]
    fn actor_path_strings() {
        let ap = ActorPath::from_str(PATH).expect("a proper path");
        println!("Got ActorPath={}", ap);

        let s = ap.to_string();
        assert_eq!(PATH, &s);
        let ap2: ActorPath = s.parse().expect("a proper path");
        assert_eq!(ap, ap2);
    }

    #[test]
    fn actor_path_unique_strings() {
        let ref1 = ActorPath::Unique(UniquePath::new(
            Transport::Local,
            "127.0.0.1".parse().expect("hardcoded IP"),
            8080,
            Uuid::new_v4(),
        ));
        let ref1_string = ref1.to_string();
        let ref1_deser = ActorPath::from_str(&ref1_string).expect("a proper path");
        let ref1_deser2: ActorPath = ref1_string.parse().expect("a proper path");
        assert_eq!(ref1, ref1_deser);
        assert_eq!(ref1, ref1_deser2);
    }

    #[test]
    fn actor_path_named_strings() {
        let ref1 = ActorPath::Named(NamedPath::new(
            Transport::Local,
            "127.0.0.1".parse().expect("hardcoded IP"),
            8080,
            vec!["test".to_string(), "path".to_string()],
        ));
        let ref1_string = ref1.to_string();
        let ref1_deser = ActorPath::from_str(&ref1_string).expect("a proper path");
        let ref1_deser2: ActorPath = ref1_string.parse().expect("a proper path");
        assert_eq!(ref1, ref1_deser);
        assert_eq!(ref1, ref1_deser2);
    }
}
