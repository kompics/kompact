use super::*;
use crate::messaging::{DispatchData, DispatchEnvelope, MsgEnvelope, PathResolvable, Serialised};
use std::{
    convert::{TryFrom, TryInto},
    error::Error,
    fmt::{self, Debug},
    net::{AddrParseError, IpAddr, SocketAddr},
    str::FromStr,
};
use uuid::Uuid;
use crate::net::buffer::ChunkLease;
use bytes::{BufMut, BytesMut};
use crate::net::frames::FrameHead;
use crate::net::frames;
use std::borrow::Borrow;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Transport {
    LOCAL = 0b00,
    TCP = 0b01,
    UDP = 0b10,
}

// impl Transport
impl Transport {
    pub fn is_local(&self) -> bool {
        match *self {
            Transport::LOCAL => true,
            _ => false,
        }
    }

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

#[derive(Clone, Debug)]
pub struct TransportParseError(());
impl fmt::Display for TransportParseError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.write_str(self.description())
    }
}
impl Error for TransportParseError {
    fn description(&self) -> &str {
        "Transport must be one of [local,tcp,udp]"
    }
}

#[derive(Clone, Debug)]
pub enum PathParseError {
    Form(String),
    Transport(TransportParseError),
    Addr(AddrParseError),
}
impl fmt::Display for PathParseError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.write_str(self.description())
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemPath {
    protocol: Transport,
    // TODO address could be IPv4, IPv6, or a domain name (not supported yet)
    address: IpAddr,
    port: u16,
}

impl SystemPath {
    pub fn new(protocol: Transport, address: IpAddr, port: u16) -> SystemPath {
        SystemPath {
            protocol,
            address,
            port,
        }
    }

    pub fn with_socket(protocol: Transport, socket: SocketAddr) -> SystemPath {
        SystemPath {
            protocol,
            address: socket.ip(),
            port: socket.port(),
        }
    }

    pub fn address(&self) -> &IpAddr {
        &self.address
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
        self.system().protocol
    }
    fn address(&self) -> &IpAddr {
        &self.system().address
    }
    fn port(&self) -> u16 {
        self.system().port
    }
}

impl SystemField for SystemPath {
    fn system(&self) -> &SystemPath {
        self
    }
}

pub trait ActorSource: Dispatching {
    fn path_resolvable(&self) -> PathResolvable;
}

pub trait ActorPathFactory {
    fn actor_path(&self) -> ActorPath;
}

// impl<F: ActorPathFactory + Dispatching> ActorSource for F {
//     fn path_resolvable(&self) -> PathResolvable {
//         PathResolvable::Ac
//     }
// }

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

#[derive(Clone, Debug)]
#[repr(u8)]
#[derive(PartialEq, Eq)]
pub enum ActorPath {
    Unique(UniquePath),
    Named(NamedPath),
}

impl ActorPath {
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

    /// Same as [tell](tell), but serialises eagerly.
    pub fn tell_ser<S, B, E>(&self, m: B, from: &S) -> Result<(), E>
    where
        S: ActorSource,
        B: TryInto<Serialised, Error = E>,
    {
        let msg: Serialised = m.try_into()?;
        let src = from.path_resolvable();
        let dst = self.clone();
        let env = DispatchEnvelope::Msg {
            src,
            dst,
            msg: DispatchData::Eager(msg),
        };
        from.dispatcher_ref().enqueue(MsgEnvelope::Typed(env));
        Ok(())
    }

    /// Same as [tell](tell), but serialises eagerly.
    pub fn tell_pooled<B, CD>(&self, m: B, from: &mut CD) -> Result<(), SerError>
        where
            B: Serialisable + Sized,
            //S: ActorSource,
            CD: ComponentDefinition + Sized + 'static,
    {
        //println!("TELL POOLED");
        let src = from.path_resolvable();
        let ser_id = &m.ser_id();
        let src_path = match src {
            PathResolvable::Path(actor_path) => {
                actor_path
            }
            PathResolvable::ActorId(uuid) => {
                //println!("actor path unique");
                ActorPath::Unique(UniquePath::with_system(from.ctx().system().system_path(), uuid.clone()))
            }
            _ => {
                panic!("tell_pooled sent from non-actor");
            }
        };
        let dst = self.clone();
        //println!("checking size hint");
        if let Some(mut size) = m.size_hint() {

            size += FrameHead::encoded_len();
            size += src_path.size_hint().unwrap_or(0);
            size += dst.size_hint().unwrap_or(0);
            size += ser_id.size();
            //println!("getting buffer");
            if let Some(mut buf) = from.ctx_mut().get_buffer(size) {
                //println!("serializing into chunk");
                crate::serialisation::helpers::serialise_into_framed_chunk(&src_path, &dst, m, &mut buf);
                let env = DispatchEnvelope::Msg {
                    src: from.path_resolvable(),
                    dst,
                    msg: DispatchData::Pooled((buf, *ser_id)),
                };
                //println!("enqueing: {:?}", env);
                from.dispatcher_ref().enqueue(MsgEnvelope::Typed(env));
                Ok(())
            } else {
                panic!("failed to get buffer!");
                // TODO: No buffer we should do exponential back-off right here and recursively call the function again
            }
        } else {
            panic!("no size hint");
        }
    }

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniquePath {
    system: SystemPath,
    id: Uuid,
}

impl UniquePath {
    pub fn new(protocol: Transport, address: IpAddr, port: u16, id: Uuid) -> UniquePath {
        UniquePath {
            system: SystemPath::new(protocol, address, port),
            id,
        }
    }

    pub fn with_system(system: SystemPath, id: Uuid) -> UniquePath {
        UniquePath { system, id }
    }

    pub fn with_socket(protocol: Transport, socket: SocketAddr, id: Uuid) -> UniquePath {
        UniquePath {
            system: SystemPath::with_socket(protocol, socket),
            id,
        }
    }

    pub fn uuid_ref(&self) -> &Uuid {
        &self.id
    }

    pub fn clone_id(&self) -> Uuid {
        self.id.clone()
    }

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

/// Attempts to parse a `&str` as a [UniquePath].
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedPath {
    system: SystemPath,
    path: Vec<String>,
}

impl NamedPath {
    pub fn new(protocol: Transport, address: IpAddr, port: u16, path: Vec<String>) -> NamedPath {
        NamedPath {
            system: SystemPath::new(protocol, address, port),
            path,
        }
    }

    pub fn with_socket(protocol: Transport, socket: SocketAddr, path: Vec<String>) -> NamedPath {
        NamedPath {
            system: SystemPath::with_socket(protocol, socket),
            path,
        }
    }

    pub fn with_system(system: SystemPath, path: Vec<String>) -> NamedPath {
        NamedPath { system, path }
    }

    pub fn path_ref(&self) -> &Vec<String> {
        &self.path
    }

    pub fn clone_path(&self) -> Vec<String> {
        self.path.clone()
    }

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

pub struct RegisteredPath {
    path: ActorPath,
    dispatcher: DispatcherRef,
}

impl Dispatching for RegisteredPath {
    fn dispatcher_ref(&self) -> DispatcherRef {
        self.dispatcher.clone()
    }
}

impl ActorSource for RegisteredPath {
    fn path_resolvable(&self) -> PathResolvable {
        PathResolvable::Path(self.path.clone())
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    const PATH: &'static str = "local://127.0.0.1:0/test_actor";

    #[test]
    fn actor_path_strings() {
        let mut settings = KompactConfig::new();
        settings.label("my_system".to_string());
        //let system = KompicsSystem::new(settings);
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
