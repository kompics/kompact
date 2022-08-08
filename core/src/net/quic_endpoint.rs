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
    ClientConfig,
};
use mio::net::UdpSocket;
//use log::{info, warn};
use std::{
    collections::VecDeque,
    collections::HashMap,
    sync::{Arc},
    time::Instant, 
    net::{SocketAddr},
    convert::TryInto,
};
use hex_literal::hex;
use rustls::{Certificate, KeyLogFile, PrivateKey};
use lazy_static::lazy_static;
use assert_matches::assert_matches;
use portpicker::pick_unused_port;


pub struct QuicEndpoint {
    pub endpoint: Endpoint,
    pub addr: SocketAddr,
    pub socket: UdpSocket,
    timeout: Option<Instant>,
    pub outbound: VecDeque<Transmit>,
    delayed: VecDeque<Transmit>,
    pub inbound: VecDeque<(Instant, Option<EcnCodepoint>, Vec<u8>)>,
    pub accepted: Option<ConnectionHandle>,
    pub connections: HashMap<ConnectionHandle, Connection>,
    pub conn_events: HashMap<ConnectionHandle, VecDeque<ConnectionEvent>>,
}
//The most important types are Endpoint, which conceptually represents the protocol state for a single socket 
//and mostly manages configuration and dispatches incoming datagrams to the related Connection. 
//Connection types contain the bulk of the protocol logic related to managing a single connection and all the related 
//state (such as streams).
///// This object performs no I/O whatsoever. Instead, it generates a stream of packets to send via
/// `poll_transmit`, and consumes incoming packets and connection-generated events via `handle` and
/// `handle_event`.
//A Quinn endpoint corresponds to a single UDP socket, no matter how many connections are in use
impl QuicEndpoint {
    pub fn new(endpoint: Endpoint, addr: SocketAddr, socket: UdpSocket) -> Self {          
        Self {
            endpoint,
            socket: socket,
            addr: addr,
            timeout: None,
            outbound: VecDeque::new(),
            delayed: VecDeque::new(),
            inbound: VecDeque::new(),
            accepted: None,
            connections: HashMap::default(),
            conn_events: HashMap::default(),
        }
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


    pub fn read_quic(&mut self, now: Instant, remote: SocketAddr) {
        //consume icoming packets and connection-generated events via handle and handle_event
        while self.inbound.front().map_or(false, |x| x.0 <= now) {
            let (recv_time, ecn, packet) = self.inbound.pop_front().unwrap();
            if let Some((ch, event)) =
                self.endpoint
                    .handle(recv_time, remote, None, ecn, packet.as_slice().into())
            {
                match event {
                    DatagramEvent::NewConnection(conn) => {
                        println!("NEW CONNECTION {}", remote);
                        self.connections.insert(ch, conn);
                        self.accepted = Some(ch);
                    }
                    DatagramEvent::ConnectionEvent(event) => {
                        println!("REDIRECT TO EXISTING CONNECTION {}", remote);
                        self.conn_events
                            .entry(ch)
                            .or_insert_with(VecDeque::new)
                            .push_back(event);
                    }
                }
            }
        }
        //generate a stream of packet to send via poll_transmit
        let mut endpoint_events: Vec<(ConnectionHandle, EndpointEvent)> = vec![];
        for (ch, conn) in self.connections.iter_mut() {
            while let Some(event) = conn.poll_endpoint_events() {
                println!("poll endpoint events {:?}", ch);
                endpoint_events.push((*ch, event));
            }

            while let Some(x) = conn.poll_transmit(now, 10) {
                println!("poll transmit {}", remote);
                self.outbound.extend(split_transmit(x));
                //println!("Self outbound {:?}", self.outbound);
            }
            self.timeout = conn.poll_timeout();
        }
        for (ch, event) in endpoint_events {
            if let Some(event) = self.endpoint.handle_event(ch, event) {
                if let Some(conn) = self.connections.get_mut(&ch) {
                    println!("handle event event {:?}", event);
                    conn.handle_event(event);
                }
            }
        }
    }

    pub fn write_quic(&mut self, now: Instant, remote: SocketAddr) {



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

fn split_transmit(transmit: Transmit) -> Vec<Transmit> {
    let segment_size = match transmit.segment_size {
        Some(segment_size) => segment_size,
        _ => return vec![transmit],
    };

    let mut offset = 0;
    let mut transmits = Vec::new();
    while offset < transmit.contents.len() {
        let end = (offset + segment_size).min(transmit.contents.len());

        let contents = transmit.contents[offset..end].to_vec();
        transmits.push(Transmit {
            destination: transmit.destination,
            ecn: transmit.ecn,
            contents,
            segment_size: None,
            src_ip: transmit.src_ip,
        });

        offset = end;
    }

    transmits
}

// #[test]
// fn establish_connection() {

//     let local = SocketAddr::new(
//         IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
//         pick_unused_port().expect("No ports free"),
//     );
//     let remote = SocketAddr::new(
//         IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
//         pick_unused_port().expect("No ports free"),
//     );

//     let client_socket = UdpSocket::bind(local).expect("could not bind UDP on TCP port for client");
//     let server_socket = UdpSocket::bind(remote).expect("could not bind UDP on TCP port for server");

//     let endpoint_conf = Endpoint::new(Default::default(), Some(Arc::new(quic_endpoint::server_config())));
//     let endpoint_conf_server = Endpoint::new(Default::default(), Some(Arc::new(quic_endpoint::server_config())));
//     let mut client_endpoint = QuicEndpoint::new(endpoint_conf, local, client_socket);
//     let mut server_endpoint = QuicEndpoint::new(endpoint_conf_server, remote, server_socket);

//     //info!("connecting");
//     let client_ch = client_endpoint.connect(remote);
//     let now = Instant::now();
//     client_endpoint.write_quic(now, remote);
//     server_endpoint.write_quic(now, local);

//     // The client completes the connection
//     assert_matches!(
//         //client_endpoint.client_conn_mut(client_ch).poll(),
//         Some(quinn_proto::Event::HandshakeDataReady)
//     );
//     assert_matches!(
//         //client_endpoint.client_conn_mut(client_ch).poll(),
//         Some(quinn_proto::Event::Connected)
//     );

//     // The server completes the connection
//     let server_ch = server_endpoint.accepted.take().expect("server didn't connect");
//     assert_matches!(
//         //server_endpoint.server_conn_mut(server_ch).poll(),
//         Some(quinn_proto::Event::HandshakeDataReady)
//     );
//     assert_matches!(
//         //server_endpoint.server_conn_mut(server_ch).poll(),
//         Some(quinn_proto::Event::Connected)
//     );
// }

#[test]
fn version_negotiate_server() {
    let client_addr = "127.0.0.1:7890".parse().expect("Address should work");
    let mut server = Endpoint::new(Default::default(), Some(Arc::new(server_config())));
    let now = Instant::now();
    let event = server.handle(
        now,
        client_addr,
        None,
        None,
        // Long-header packet with reserved version number
        hex!("80 0a1a2a3a 04 00000000 04 00000000 00")[..].into(),
    );
    assert!(event.is_none());

    let io = server.poll_transmit();
    assert!(io.is_some());
    if let Some(Transmit { contents, .. }) = io {
        assert_ne!(contents[0] & 0x80, 0);
        assert_eq!(&contents[1..15], hex!("00000000 04 00000000 04 00000000"));
       assert!(contents[15..].chunks(4).any(|x| {
           DEFAULT_SUPPORTED_VERSIONS.contains(&u32::from_be_bytes(x.try_into().unwrap()))
       }));
    }
    assert_matches!(server.poll_transmit(), None);
}

// #[test]
// fn version_negotiate_client() {
//     let server_addr = "127.0.0.1:7890".parse().expect("Address should work");
//     let mut client = Endpoint::new(Default::default(), None);
//     println!("Client {:?}", client);
//     let (_, mut client_ch) = client
//         .connect(client_config(), server_addr, "localhost")
//         .unwrap();
//     let now = Instant::now();
//     let opt_event = client.handle(
//         now,
//         server_addr,
//         None,
//         None,
//        // Version negotiation packet for reserved version
//         hex!("80 00000000 04 00000000 04 00000000 0a1a2a3a")[..].into(),
//     );

//     if let Some((_, quinn_proto::DatagramEvent::ConnectionEvent(event))) = opt_event {
//         client_ch.handle_event(event);
//     }
//     assert_matches!(
//         client_ch.poll(),
//         Some(quinn_proto::Event::ConnectionLost {
//             reason: quinn_proto::ConnectionError::VersionMismatch,
//         })
//     );
// }

/// The QUIC protocol version implemented.
pub const DEFAULT_SUPPORTED_VERSIONS: &[u32] = &[
    0x00000001,
    0xff00_001d,
    0xff00_001e,
    0xff00_001f,
    0xff00_0020,
    0xff00_0021,
    0xff00_0022,
];