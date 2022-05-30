use mio::net::UdpSocket;
use super::*;
use quinn_proto::{
    EcnCodepoint, 
    ConnectionHandle, 
    Connection, 
    ConnectionEvent, 
    Transmit, 
    Endpoint, 
    DatagramEvent, 
    EndpointEvent, 
    ServerConfig
};
use std::{
    collections::VecDeque,
    collections::HashMap,
    net::{SocketAddr},
    sync::Arc,
    usize,
    time::Instant, 
    env,
};
use lazy_static::lazy_static;
use rustls::{Certificate, PrivateKey};
pub use rustls::Error;

pub(super) struct QuinnEndpoint {
    pub endpoint: Endpoint,
    pub addr: SocketAddr,
    socket: Option<UdpSocket>,
    timeout: Option<Instant>,
    pub outbound: VecDeque<Transmit>,
    delayed: VecDeque<Transmit>,
    pub inbound: VecDeque<(Instant, Option<EcnCodepoint>, Vec<u8>)>,
    accepted: Option<ConnectionHandle>,
    pub connections: HashMap<ConnectionHandle, Connection>,
    conn_events: HashMap<ConnectionHandle, VecDeque<ConnectionEvent>>,
}
//The most important types are Endpoint, which conceptually represents the protocol state for a single socket 
//and mostly manages configuration and dispatches incoming datagrams to the related Connection. 
//Connection types contain the bulk of the protocol logic related to managing a single connection and all the related 
//state (such as streams).
impl QuinnEndpoint {
    pub(super) fn new(endpoint: Endpoint, addr: SocketAddr) -> Self {
        //Generate TLS session keys
        //Fetches the environment variable key from the current process, returning None if the variable isn't set.
        let socket = if env::var_os("SSLKEYLOGFILE").is_some() {
           let socket = UdpSocket::bind(addr).expect("failed to bind UDP socket");
            //SSL shutdown
           // socket
               // .set_read_timeout(Some(Duration::new(0, 10_000_000)))
               // .unwrap();
            Some(socket)
        } else {
            None
        };
        Self {
            endpoint,
            addr,
            socket,
            timeout: None,
            outbound: VecDeque::new(),
            delayed: VecDeque::new(),
            inbound: VecDeque::new(),
            accepted: None,
            connections: HashMap::default(),
            conn_events: HashMap::default(),
        }
    }
    pub(super) fn read_incoming(&mut self,  recv_time: Instant, remote: SocketAddr, ecn: Option<EcnCodepoint>, data: Vec<u8>){
        //connection handle and endpoint event resulting from processing a single datagram
        if let Some((ch, event)) =
        self.endpoint
            //Process an incoming UDP datagram from remote addr
            .handle(recv_time, remote, None, ecn, data.as_slice().into())
        {
            match event {
                //The datagram has resulted in starting a new Connection
                DatagramEvent::NewConnection(conn) => {
                    self.connections.insert(ch, conn);
                    self.accepted = Some(ch);
                }
                //The datagram is redirected to its Connection
                DatagramEvent::ConnectionEvent(event) => {
                    self.conn_events
                        .entry(ch)
                        .or_insert_with(VecDeque::new)
                        .push_back(event);
                }
            }
        }
    }

    fn drive(&mut self, now: Instant) {
        //CALL writeUDP when using poll_transmit()
        //call poll_transmit() when doin writeUDP
        //call poll_transmit() when sending quinn message
        while let Some(x) = self.endpoint.poll_transmit() {
            self.outbound.extend(split_transmit(x));
        }

        let mut endpoint_events: Vec<(ConnectionHandle, EndpointEvent)> = vec![];
        for (ch, conn) in self.connections.iter_mut() {
            if self.timeout.map_or(false, |x| x <= now) {
                self.timeout = None;
                conn.handle_timeout(now);
            }

            for (_, mut events) in self.conn_events.drain() {
                for event in events.drain(..) {
                    //Process EndpointEvents emitted from related Connections
                    //In turn, processing this event may return a ConnectionEvent for the same Connection.
                    conn.handle_event(event);
                }
            }

            while let Some(event) = conn.poll_endpoint_events() {
                endpoint_events.push((*ch, event));
            }

            while let Some(x) = conn.poll_transmit(now, MAX_DATAGRAMS) {
                self.outbound.extend(split_transmit(x));
            }
            self.timeout = conn.poll_timeout();
        }

        for (ch, event) in endpoint_events {
            if let Some(event) = self.endpoint.handle_event(ch, event) {
                if let Some(conn) = self.connections.get_mut(&ch) {
                    conn.handle_event(event);
                }
            }
        }
    }
}
pub(crate) fn server_config() -> ServerConfig {
    ServerConfig::with_crypto(Arc::new(server_crypto()))
}

pub fn server_crypto() -> rustls::ServerConfig {
    let cert = Certificate(CERTIFICATE.serialize_der().unwrap());
    let key = PrivateKey(CERTIFICATE.serialize_private_key_der());
    server_crypto_with_cert(cert, key)
}

fn server_crypto_with_cert(cert: Certificate, key: PrivateKey) -> rustls::ServerConfig {
    server_config_with_cert_and_key(vec![cert], key).unwrap()
}

pub(crate) fn server_config_with_cert_and_key(
    cert_chain: Vec<rustls::Certificate>,
    key: rustls::PrivateKey,
) -> Result<rustls::ServerConfig, Error> {
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

lazy_static! {
    pub(crate) static ref CERTIFICATE: rcgen::Certificate =
        rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
}
/// The maximum of datagrams QuinnEndpoint will produce via `poll_transmit`
const MAX_DATAGRAMS: usize = 10;

pub fn split_transmit(transmit: Transmit) -> Vec<Transmit> {
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