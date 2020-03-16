use super::*;
use crate::messaging::{DispatchData, DispatchEnvelope, MsgEnvelope, PathResolvable};
use std::{
    convert::TryFrom,
    error::Error,
    fmt::{self, Debug},
    net::{AddrParseError, IpAddr, SocketAddr},
    str::FromStr,
};
use uuid::Uuid;

use crate::net::frames::{FrameHead, FrameType, FRAME_HEAD_LEN};
use bytes::Buf;

use std::ops::DerefMut;
use crate::net::buffer::EncodeBuffer;

/// Transport protocol to use for delivering messages
/// sent to an [ActorPath](ActorPath)
///
/// # Note
///
/// Dispatcher implementations are not required to implement all protocols.
/// Check your concrete implementation, before selecting an arbitrary protocol.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Transport {
    /// Local reflection only, no network messages involved
    LOCAL = 0b00,
    /// Send messages over TCP
    TCP = 0b01,
    /// Send messages as UDP datagrams
    UDP = 0b10,
}

// impl Transport
impl Transport {
    /// Returns `true` if this is an instance of [Transport::LOCAL](Transport::LOCAL)
    pub fn is_local(&self) -> bool {
        match *self {
            Transport::LOCAL => true,
            _ => false,
        }
    }

    /// Returns `true` if this is *not* an instance of [Transport::LOCAL](Transport::LOCAL)
    pub fn is_remote(&self) -> bool {
        !self.is_local()
    }
}

impl fmt::Display for Transport {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            &Transport::LOCAL => write!(fmt, "local"),
            &Transport::TCP => write!(fmt, "tcp"),
            &Transport::UDP => write!(fmt, "udp"),
        }
    }
}

impl FromStr for Transport {
    type Err = TransportParseError;

    fn from_str(s: &str) -> Result<Transport, TransportParseError> {
        match s {
            "local" => Ok(Transport::LOCAL),
            "tcp" => Ok(Transport::TCP),
            "udp" => Ok(Transport::UDP),
            _ => Err(TransportParseError(())),
        }
    }
}

/// Error type for parsing the [Transport](Transport) from a string
#[derive(Clone, Debug)]
pub struct TransportParseError(());

impl fmt::Display for TransportParseError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.write_str(&self.to_string())
    }
}

impl Error for TransportParseError {
    fn description(&self) -> &str {
        "Transport must be one of [local,tcp,udp]"
    }
}

/// Error type for parsing [paths](ActorPath) from a string
#[derive(Clone, Debug)]
pub enum PathParseError {
    /// The format is wrong
    Form(String),
    /// The transport protocol was invalid
    Transport(TransportParseError),
    /// The network address was invalid
    Addr(AddrParseError),
}

impl fmt::Display for PathParseError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.write_str(&self.to_string())
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
}

impl fmt::Display for SystemPath {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "{}://{}:{}", self.protocol, self.address, self.port)
    }
}

pub(crate) trait SystemField {
    fn system(&self) -> &SystemPath;

    fn protocol(&self) -> Transport {
        self.system().protocol()
    }
    fn address(&self) -> &IpAddr {
        &self.system().address()
    }
    fn port(&self) -> u16 {
        self.system().port()
    }
}

impl SystemField for SystemPath {
    fn system(&self) -> &SystemPath {
        self
    }
}

/// A factory trait for things can produce [PathResolvable](PathResolvable) instances
///
/// Only makes sense for types that can [dispatch](Dispatching) messages.
pub trait ActorSource: Dispatching {
    /// Returns the associated path resolvable
    fn path_resolvable(&self) -> PathResolvable;
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

impl<'a, 'b> ActorSource for DispatchingPath<'a, 'b> {
    fn path_resolvable(&self) -> PathResolvable {
        PathResolvable::Path(self.path.clone())
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
#[derive(PartialEq, Eq)]
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
    /// Serialisation of `m` happens lazily in the dispatcher,
    /// and only if it really goes over the network. If this actor path
    /// turns out to be local, `m` will be moved to the heap and send locally instead.
    ///
    /// In fact, this method always moves `m` onto the heap (unless it already is allocated there),
    /// to facility this lazy serialisation.
    /// As this method has some overhead in the case where it sure
    /// that `m` will definitely go over the network, you can use
    /// [tell_ser](ActorPath::tell_ser) to force eager serialisation instead.
    pub fn tell<S, B>(&self, m: B, from: &S) -> ()
        where
            S: ActorSource,
            B: Into<Box<dyn Serialisable>>,
    {
        let msg: Box<dyn Serialisable> = m.into();
        let src = from.path_resolvable();
        let dst = self.clone();
        let env = DispatchEnvelope::Msg {
            src,
            dst,
            msg: DispatchData::Lazy(msg),
        };
        from.dispatcher_ref().enqueue(MsgEnvelope::Typed(env))
    }

    /// Same as [tell](ActorPath::tell), but serialises eagerly into a Pooled buffer (pre-allocated and bounded)
    pub fn tell_serialised<CD, B>(&self, m: B, from: &CD) -> Result<(), SerError>
        where
            CD: ComponentDefinition + Sized + 'static,
            B: Serialisable + 'static,
    {
        if self.protocol() == Transport::LOCAL {
            // No need to serialize!
            self.tell(m, from);
            return Ok(());
        }
        //let mut buf = encode_buffer.deref_mut();
        let mut buf_ref = from.ctx().get_buffer().borrow_mut();
        let buf = &mut EncodeBuffer::get_buffer_encoder(buf_ref.deref_mut());
        // Reserve space for the header:
        buf.pad(FRAME_HEAD_LEN as usize);

        from.actor_path().serialise(buf)?; // src
        self.clone().serialise(buf)?; // dst
        buf.put_ser_id(m.ser_id()); // ser_id
        Serialisable::serialise(&m, buf)?; // data

        if let Some(mut chunk_lease) = buf.get_chunk_lease() {
            // The length which a FrameHead tells does not include the size of the FrameHead itself
            let len = chunk_lease.remaining() - FRAME_HEAD_LEN as usize;
            chunk_lease.insert_head(FrameHead::new(FrameType::Data, len));
            let env = DispatchEnvelope::Msg {
                src: from.path_resolvable(),
                dst: self.clone(),
                msg: DispatchData::Serialised((chunk_lease, m.ser_id())),
            };
            from.dispatcher_ref().enqueue(MsgEnvelope::Typed(env));
            Ok(())
        } else {
            Err(SerError::BufferError("Could not get ChunkLease from Buffer".to_string()))
        }
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
    pub fn set_transport(&mut self, proto: Transport) {
        self.system_mut().protocol = proto;
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

const PATH_SEP: &'static str = "/";
const UNIQUE_PATH_SEP: &'static str = "#";

impl fmt::Display for ActorPath {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            &ActorPath::Named(ref np) => {
                let path = np
                    .path
                    .iter()
                    .fold(String::new(), |acc, arg| acc + PATH_SEP + arg);
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub fn uuid_ref(&self) -> &Uuid {
        &self.id
    }

    /// Returns a copy of this path's unique id
    pub fn clone_id(&self) -> Uuid {
        self.id.clone()
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
        NamedPath { system, path }
    }

    /// Returns a reference to the path vector
    pub fn path_ref(&self) -> &Vec<String> {
        &self.path
    }

    /// Returns a copy to the path vector
    pub fn clone_path(&self) -> Vec<String> {
        self.path.clone()
    }

    /// Returns a mutable reference to this path's system path part
    pub fn system_mut(&mut self) -> &mut SystemPath {
        &mut self.system
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
        let p = NamedPath::from_str(&s)?;
        Ok(p)
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
        if s2.len() < 1 {
            return Err(PathParseError::Form(s.to_string()));
        }
        let socket = SocketAddr::from_str(s2[0])?;
        let path: Vec<String> = if s2.len() > 1 {
            s2.split_off(1).into_iter().map(|s| s.to_string()).collect()
        } else {
            Vec::default()
        };
        Ok(NamedPath::with_socket(proto, socket, path.clone()))
    }
}

// this doesn't seem to be used anywhere...
//
/// An actor path paired with a reference to a dispatcher
///
/// This struct can be used in places where an [ActorSource](ActorSource) is expected.
// pub struct RegisteredPath {
//     path: ActorPath,
//     dispatcher: DispatcherRef,
// }

// impl Dispatching for RegisteredPath {
//     fn dispatcher_ref(&self) -> DispatcherRef {
//         self.dispatcher.clone()
//     }
// }

// impl ActorSource for RegisteredPath {
//     fn path_resolvable(&self) -> PathResolvable {
//         PathResolvable::Path(self.path.clone())
//     }
// }
#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &'static str = "local://127.0.0.1:0/test_actor";

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
            Transport::LOCAL,
            "127.0.0.1".parse().expect("hardcoded IP"),
            8080,
            Uuid::new_v4(),
        ));
        let ref1_string = ref1.to_string();
        //println!("ActorPath::Unique: {}", ref1_string);
        let ref1_deser = ActorPath::from_str(&ref1_string).expect("a proper path");
        let ref1_deser2: ActorPath = ref1_string.parse().expect("a proper path");
        assert_eq!(ref1, ref1_deser);
        assert_eq!(ref1, ref1_deser2);
    }

    #[test]
    fn actor_path_named_strings() {
        let ref1 = ActorPath::Named(NamedPath::new(
            Transport::LOCAL,
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
