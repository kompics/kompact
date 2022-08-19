use bytes::BytesMut;
use crate::net::buffers::BufferPool;
use super::*;
use quinn_proto::{
    EcnCodepoint, 
    ConnectionHandle, 
    Connection, 
    DatagramEvent,
    EndpointEvent,
    ConnectionEvent, 
    Transmit, 
    Endpoint, 
    ServerConfig,
    ClientConfig, Streams,
};
use mio::net::UdpSocket;
//use log::{info, warn};
use std::{
    collections::VecDeque,
    collections::HashMap,
    sync::{Arc},
    time::Instant, 
    net::{SocketAddr, Ipv4Addr},
    convert::TryInto,
    io::Error,
};
use hex_literal::hex;
use rustls::{Certificate, KeyLogFile, PrivateKey};
use lazy_static::lazy_static;
use assert_matches::assert_matches;
use portpicker::pick_unused_port;
use crate::{
    messaging::{SerialisedFrame},
    net::buffers::{DecodeBuffer, BufferChunk},
};

const MAX_DATAGRAMS: usize = 10;

pub struct QuicEndpoint {
    pub endpoint: Endpoint,
    pub addr: SocketAddr,
    pub socket: UdpSocket,
    timeout: Option<Instant>,
    pub outbound: VecDeque<Transmit>,
    //delayed: VecDeque<Transmit>,
    pub inbound: VecDeque<(Instant, Option<EcnCodepoint>, Vec<u8>)>,
    pub accepted: Option<ConnectionHandle>,
    pub connections: HashMap<ConnectionHandle, Connection>,
    pub conn_events: HashMap<ConnectionHandle, VecDeque<ConnectionEvent>>,
}

impl QuicEndpoint {
    pub fn new(endpoint: Endpoint, 
                addr: SocketAddr, 
                socket: UdpSocket) -> Self {          
        Self {
            endpoint,
            socket: socket,
            addr: addr,
            timeout: None,
            outbound: VecDeque::new(),
            //delayed: VecDeque::new(),
            inbound: VecDeque::new(),
            accepted: None,
            connections: HashMap::default(),
            conn_events: HashMap::default(),
        }
    }
    pub fn client_conn_mut(&mut self, ch: ConnectionHandle) -> &mut Connection {
        return self.connections.get_mut(&ch).unwrap();
    }
    pub fn server_conn_mut(&mut self, ch: ConnectionHandle) -> &mut Connection {
        return self.connections.get_mut(&ch).unwrap();
    }
    pub fn client_streams(&mut self, ch: ConnectionHandle) -> Streams<'_> {
        self.client_conn_mut(ch).streams()
    }
    pub fn server_streams(&mut self, ch: ConnectionHandle) -> Streams<'_> {
        self.server_conn_mut(ch).streams()
    }
    /// start connecting the client
    pub fn connect(&mut self, remote: SocketAddr) -> ConnectionHandle {
        println!("Initiating Connection to ... {}", remote);  
        let (client_ch, client_conn) = self
            .endpoint
            .connect(client_config(), remote, "localhost")
            .unwrap();
        self.connections.insert(client_ch, client_conn);
        client_ch
    }

    pub fn try_read_quic(&mut self, now: Instant) -> io::Result<()> { 
        //consume icoming packets and connection-generated events via handle and handle_event
        let mut buf = [0; 65000];
        match self.socket.recv_from(&mut buf) {
            Ok((n, remote)) => {
                println!("Received {n:?} from {remote:?}");
                if let Some((ch, event)) =
                    self.endpoint.handle(now, remote, None, None, BytesMut::from(buf.as_slice()))
                {
                    match event {
                        DatagramEvent::NewConnection(conn) => {
                            println!("NEW CONNECTION {:?} {:?}", ch, conn);
                            self.connections.insert(ch, conn);
                            self.accepted = Some(ch);
                        }
                        DatagramEvent::ConnectionEvent(event) => {
                            println!("REDIRECT TO EXISTING CONNECTION {:?}", ch);
                            self.conn_events
                            .entry(ch)
                            .or_insert_with(VecDeque::new)
                            .push_back(event);
                        }
                    }
                }
                //Get the next packet to transmit
                while let Some(x) = self.endpoint.poll_transmit() {
                    println!("endpoint poll transmit in write {}", self.addr);
                    self.outbound.push_back(x);
                }
                
                let mut endpoint_events: Vec<(ConnectionHandle, EndpointEvent)> = vec![];
                for (ch, conn) in self.connections.iter_mut() {
                    println!("connectionhandle read {:?}", ch);
                    if self.timeout.map_or(false, |x| x <= now) {
                        self.timeout = None;
                        conn.handle_timeout(now);
                    }
                    //Return endpoint-facing events
                    while let Some(event) = conn.poll_endpoint_events() {
                        println!("push to endpoint events {:?}", event);
                        endpoint_events.push((*ch, event))
                    }
                    // //Returns packets to transmit
                    while let Some(x) = conn.poll_transmit(now, 10) {
                    println!("conn poll transmit {} {:?} {:?}", remote, ch, conn);
                        self.outbound.push_back(x);
                    }
                    self.timeout = conn.poll_timeout();
                }
                for (ch, event) in endpoint_events {
                    //Process ConnectionEvents generated by the associated Endpoint
                    if let Some(event) = self.endpoint.handle_event(ch, event) {
                        if let Some(conn) = self.connections.get_mut(&ch) {
                            println!("handle event {:?} {:?}", ch, conn);
                            conn.handle_event(event);
                           // println!("conn poll  {:?}", conn.poll());
                        }
                    }
                }
                return Ok(());
            }
            Err(err) => {
                return Err(err);
            }
        }
    }

    pub fn try_write_quic(&mut self, now: Instant) -> io::Result<usize> {
        let mut sent_bytes: usize = 0;
        let mut interrupts = 0;
        //Get the next packet to transmit
        while let Some(x) = self.endpoint.poll_transmit() {
            println!("endpoint poll transmit in write {}", self.addr);
            self.outbound.push_back(x);
        }

        for (ch, conn) in self.connections.iter_mut() {
            if self.timeout.map_or(false, |x| x <= now) {
                self.timeout = None;
                conn.handle_timeout(now);
            }
            //Returns packets to transmit, Connections should be polled for transmit after:
            //the application performed some I/O on the connection
            //a call was made to handle_event
            //a call was made to handle_timeout
            while let Some(x) = conn.poll_transmit(now, MAX_DATAGRAMS) {
                println!("poll transmit in write {:?} {:?}", ch, conn);
                self.outbound.push_back(x);
            }
            self.timeout = conn.poll_timeout();
        }

        while let Some(x) = self.outbound.pop_front(){
            //let mut interrupts = 0;
            // x.make_contiguous();
            match self.socket.send_to(&x.contents, x.destination) {
                Ok(n) => {
                    sent_bytes += n;
                }
                Err(ref err) if would_block(err) => {
                    // re-insert the data at the front of the buffer and return
                    self.outbound.push_front(x);
                    return Ok(sent_bytes);
                }
                Err(err) if interrupted(&err) => {
                    // re-insert the data at the front of the buffer
                    self.outbound.push_front(x);
                    interrupts += 1;
                    if interrupts >= network_thread::MAX_INTERRUPTS {
                        return Err(err);
                    }
                }
                // Other errors we'll consider fatal.
                Err(err) => {
                    self.outbound.push_front(x);
                    return Err(err);
                }
            }
        }
        Ok(sent_bytes)
    }
}

 pub fn server_config() -> ServerConfig {
    ServerConfig::with_crypto(Arc::new(server_crypto()))
}

pub fn server_crypto() -> rustls::ServerConfig {
    let cert = Certificate(CERTIFICATE.serialize_der().unwrap());
    let key = PrivateKey(CERTIFICATE.serialize_private_key_der());
    server_crypto_with_cert(cert, key)
}

pub fn server_crypto_with_cert(cert: Certificate, key: PrivateKey) -> rustls::ServerConfig {
    server_conf(vec![cert], key).unwrap()
}

pub fn server_conf(
    cert_chain: Vec<rustls::Certificate>,
    key: rustls::PrivateKey,
) -> Result<rustls::ServerConfig, rustls::Error> {
    let mut cfg = rustls::ServerConfig::builder()
        .with_safe_default_cipher_suites()
        .with_safe_default_kx_groups()
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)?;
    cfg.max_early_data_size = u32::MAX;
    Ok(cfg)
}

pub fn client_config() -> ClientConfig {
    ClientConfig::new(Arc::new(client_crypto()))
}

pub fn client_crypto() -> rustls::ClientConfig {
    let cert = rustls::Certificate(CERTIFICATE.serialize_der().unwrap());
    client_crypto_with_certs(vec![cert])
}

pub fn client_crypto_with_certs(certs: Vec<rustls::Certificate>) -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    for cert in certs {
        roots.add(&cert).unwrap();
    }
    let mut config = rustls::ClientConfig::builder()
        .with_safe_default_cipher_suites()
        .with_safe_default_kx_groups()
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.enable_early_data = true;
    config.key_log = Arc::new(KeyLogFile::new());
    config
}

lazy_static! {
    static ref CERTIFICATE: rcgen::Certificate =
        rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
}


#[cfg(test)]
#[allow(unused_must_use)]
fn setup_endpoints() -> (QuicEndpoint, QuicEndpoint) {
    let local = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        pick_unused_port().expect("No ports free"),
    );
    let remote = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        pick_unused_port().expect("No ports free"),
    );
    let client_socket = UdpSocket::bind(local).expect("could not bind UDP for client");
    let server_socket = UdpSocket::bind(remote).expect("could not bind UDP for server");

    //Client endpoint should be using an endpoint with no server_config set
    //When called connect the client_config will be applied
    let endpoint_conf = Endpoint::new(Default::default(), None);
    let endpoint_conf2 = Endpoint::new(Default::default(), Some(Arc::new(quic_endpoint::server_config())));

    //Bind endpoints to client and server sockets
    let mut endpoint = QuicEndpoint::new(endpoint_conf, local, client_socket);
    let mut server_endpoint = QuicEndpoint::new(endpoint_conf2, remote, server_socket);

    //Connect to remote host
    endpoint.connect(remote);

    (endpoint, server_endpoint)
}

#[test]
fn establish_connection() {
    let (mut endpoint, mut server_endpoint) = setup_endpoints();

    match endpoint.try_write_quic(Instant::now()) {
        Ok(_) => {}
        Err(e) => {
           println!("Error writing quic from endpoint {}", e);
        }
    }
    thread::sleep(Duration::from_secs(1));

    //Server should be handshake data ready 
    match server_endpoint.try_read_quic(Instant::now()) {
        Ok(_) => {
            let server_ch = server_endpoint.accepted.take().expect("server didn't connect");
            assert_matches!(
                server_endpoint.connections.get_mut(&server_ch).unwrap().poll(),
                Some(quinn_proto::Event::HandshakeDataReady)    
            );        
        }
        Err(e) => {
           println!("Error reading quic from server endpoint {}", e);
        }
    }
    thread::sleep(Duration::from_secs(1));
 
    match endpoint.try_read_quic(Instant::now()) {
        Ok(_) => {
            let client_ch = endpoint.accepted.take().expect("client didn't connect");
            let server_ch = server_endpoint.accepted.take().expect("server didn't connect");

            //The client completes the connection
            assert_matches!(
                endpoint.connections.get_mut(&client_ch).unwrap().poll(),
                Some(quinn_proto::Event::HandshakeDataReady)    
            );   
            assert_matches!(
                endpoint.connections.get_mut(&client_ch).unwrap().poll(),
                Some(quinn_proto::Event::Connected { .. })    
            ); 
            //The server completes the connection
            assert_matches!(
                server_endpoint.connections.get_mut(&server_ch).unwrap().poll(),
                Some(quinn_proto::Event::HandshakeDataReady)    
            );
            assert_matches!(
                server_endpoint.connections.get_mut(&server_ch).unwrap().poll(),
                Some(quinn_proto::Event::Connected { .. })    
            );       
        }
        Err(e) => {
           println!("Error reading quic from endpoint {}", e);
        }
    }
}

#[test]
fn finish_stream() {

}

// #[test]
// fn version_negotiate_server() {
//     let client_addr = "127.0.0.1:7890".parse().expect("Address should work");
//     let mut server = Endpoint::new(Default::default(), Some(Arc::new(server_config())));
//     let now = Instant::now();
//     let event = server.handle(
//         now,
//         client_addr,
//         None,
//         None,
//         // Long-header packet with reserved version number
//         hex!("80 0a1a2a3a 04 00000000 04 00000000 00")[..].into(),
//     );
//     assert!(event.is_none());

//     let io = server.poll_transmit();
//     assert!(io.is_some());
//     if let Some(Transmit { contents, .. }) = io {
//         assert_ne!(contents[0] & 0x80, 0);
//         assert_eq!(&contents[1..15], hex!("00000000 04 00000000 04 00000000"));
//        assert!(contents[15..].chunks(4).any(|x| {
//            DEFAULT_SUPPORTED_VERSIONS.contains(&u32::from_be_bytes(x.try_into().unwrap()))
//        }));
//     }
//     assert_matches!(server.poll_transmit(), None);
//     /// The QUIC protocol version implemented.
//     pub const DEFAULT_SUPPORTED_VERSIONS: &[u32] = &[
//         0x00000001,
//         0xff00_001d,
//         0xff00_001e,
//         0xff00_001f,
//         0xff00_0020,
//         0xff00_0021,
//         0xff00_0022,
//     ];
// }