use async_std::fs::write;
use bytes::{BytesMut, BufMut};
use tracing::log::logger;
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
    Streams, StreamId, SendStream, RecvStream, Datagrams, Dir, coding::BufExt,
};
use mio::net::{UdpSocket};
//use log::{info, warn};
use crate::{
    messaging::{DispatchEnvelope, EventEnvelope, NetMessage, SerialisedFrame},
    net::{
        buffers::{BufferChunk, BufferPool, EncodeBuffer},
        udp_state::UdpState,
        quic_config,
    },
    prelude::{NetworkStatus, SessionId},
    serialisation::ser_helpers::deserialise_chunk_lease,
};
use std::{
    collections::VecDeque,
    collections::HashMap,
    sync::{Arc},
    time::Instant, 
    net::{SocketAddr, Ipv4Addr},
    convert::TryInto,
    io::Error, cell::RefCell,
};
use hex_literal::hex;
use assert_matches::assert_matches;
use portpicker::pick_unused_port;

const MAX_DATAGRAMS: usize = 10;
#[derive(Debug)]
pub struct QuicEndpoint {
    pub endpoint: Endpoint,
    pub addr: SocketAddr,
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
               // socket: UdpSocket
            ) -> Self {          
        Self {
            endpoint,
           // socket: udp_state.socket,
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
    pub fn conn_mut(&mut self, ch: ConnectionHandle) -> &mut Connection {
        return self.connections.get_mut(&ch).unwrap();
    }
    pub fn streams(&mut self, ch: ConnectionHandle) -> Streams<'_> {
        self.conn_mut(ch).streams()
    }
    pub fn send(&mut self, ch: ConnectionHandle, s: StreamId) -> SendStream<'_> {
        self.conn_mut(ch).send_stream(s)
    }
    pub fn recv(&mut self, ch: ConnectionHandle, s: StreamId) -> RecvStream<'_> {
        self.conn_mut(ch).recv_stream(s)
    }
    pub fn datagrams(&mut self, ch: ConnectionHandle) -> Datagrams<'_> {
        self.conn_mut(ch).datagrams()
    }

    /// start connecting the client
    pub fn connect(&mut self, remote: SocketAddr) -> ConnectionHandle {
        println!("Initiating Connection to ... {}", remote);  
        let (client_ch, client_conn) = self
            .endpoint
            .connect(quic_config::client_config(), remote, "localhost")
            .unwrap();
        self.connections.insert(client_ch, client_conn);
        client_ch
    }

    // pub fn connect_with(&mut self, config: ClientConfig) -> (ConnectionHandle, ConnectionHandle) {
    //     info!("connecting");
    //     let client_ch = self.begin_connect(config);
    //     self.drive();
    //     let server_ch = self.server.assert_accept();
    //     self.finish_connect(client_ch, server_ch);
    //     (client_ch, server_ch)
    // }

    pub(super) fn try_read_quic(&mut self, now: Instant, remote: SocketAddr, udp_state: &mut UdpState, buffer_pool: &RefCell<BufferPool>) -> io::Result<()> { 
        //consume icoming packets and connection-generated events via handle and handle_event
       // match self.socket.recv_from(&mut buf) {
        match udp_state.try_read(buffer_pool) {
            Ok(n) => {
                //let data = udp_state.input_buffer.read_chunk_lease(buf.len());
               // println!("data from udp_state incoming {:?}", data);
                let byte_data = udp_state.input_buffer.read_chunk_lease(1200);
              //  println!("byte_data {:?}", byte_data);
                let buffer = byte_data.content.to_vec();
                //buf.put_slice(bytes::BufMut);
                if let Some((ch, event)) =
                    self.endpoint.handle(now, remote, None, None, buffer.as_slice().into())
                {
                    match event {
                        DatagramEvent::NewConnection(conn) => {
                            println!("New connection {:?}", ch);
                            self.connections.insert(ch, conn);
                            self.accepted = Some(ch);
                        }
                        DatagramEvent::ConnectionEvent(event) => {
                            println!("Redirect to existing connection {:?}", ch);
                            self.conn_events
                            .entry(ch)
                            .or_insert_with(VecDeque::new)
                            .push_back(event);
                        }
                    }
                }
                //Get the next packet to transmit
                // while let Some(x) = self.endpoint.poll_transmit() {
                // //    println!("poll transmit on endpoint  {:?}", x);
                //     udp_state.outbound_queue.push_back((x.destination, SerialisedFrame::Vec(x.contents)));
                // }
                
                let mut endpoint_events: Vec<(ConnectionHandle, EndpointEvent)> = vec![];
                for (ch, conn) in self.connections.iter_mut() {
                    if self.timeout.map_or(false, |x| x <= now) {
                        self.timeout = None;
                        conn.handle_timeout(now);
                    }
                    //Return endpoint-facing events
                    while let Some(event) = conn.poll_endpoint_events() {
                    println!("poll transmit on conn  {:?}", ch);
                        endpoint_events.push((*ch, event))
                    }
                    // //Returns packets to transmit
                    // while let Some(x) = conn.poll_transmit(now, MAX_DATAGRAMS) {
                    //     println!("push to outbound queue  {:?}", x.destination);
                    //     udp_state.outbound_queue.push_back((x.destination, SerialisedFrame::Vec(x.contents)));
                    // }
                    // self.timeout = conn.poll_timeout();
                }
                for (ch, event) in endpoint_events {
                    //Process ConnectionEvents generated by the associated Endpoint
                    if let Some(event) = self.endpoint.handle_event(ch, event) {
                        if let Some(conn) = self.connections.get_mut(&ch) {
                            conn.handle_event(event);
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


//    pub fn send_message(&mut self, msg: SerialisedFrame, destination: SocketAddr) {

//         self.try_write_quic(Instant::now());
        
//        //udp_state.enqueue_serialised(addr, )
//        //udp_state.enqueue_somethingelse(addr, somethingelse)
//    }

    pub(super) fn try_write_quic(&mut self, now: Instant, udp_state: &mut UdpState) -> io::Result<()>{
        //Get the next packet to transmit
        while let Some(x) = self.endpoint.poll_transmit() {
            udp_state.outbound_queue.push_back((x.destination, SerialisedFrame::Vec(x.contents)));
        }

        for (ch, conn) in self.connections.iter_mut() {
            if self.timeout.map_or(false, |x| x <= now) {
                self.timeout = None;
                conn.handle_timeout(now);
            }
            while let Some(x) = conn.poll_transmit(now, MAX_DATAGRAMS) {
               // println!("poll transmit in write {:?} ", x.contents);
                udp_state.outbound_queue.push_back((x.destination, SerialisedFrame::Vec(x.contents)));
            }
            self.timeout = conn.poll_timeout();
        }
        match udp_state.try_write() {
            Ok(_) => Ok({}),
            // Other errors we'll consider fatal.
            Err(err) => {
                return Err(err);
            }
        }
    }
    
}


#[cfg(test)]
#[allow(unused_must_use)]
fn setup_endpoints() -> (QuicEndpoint, QuicEndpoint, SocketAddr, UdpSocket) {
    let local = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        pick_unused_port().expect("No ports free"),
    );
    let remote = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        pick_unused_port().expect("No ports free"),
    );
    let udp_address = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        pick_unused_port().expect("No ports free"),
    );

    let udp_socket = UdpSocket::bind(udp_address).expect("could not bind UDP for udp_state");
    //Client endpoint should be using an endpoint with no server_config set
    //When called connect the client_config will be applied
    let endpoint_conf = Endpoint::new(Default::default(), None);
    let endpoint_conf2 = Endpoint::new(Default::default(), Some(Arc::new(quic_config::server_config())));

    //Bind endpoints to client and server sockets
    let endpoint = QuicEndpoint::new(endpoint_conf, local);
    let server_endpoint = QuicEndpoint::new(endpoint_conf2, remote);

    //Connect to remote host
   // endpoint.connect(remote);

    (endpoint, server_endpoint, remote, udp_socket)
}

#[test]
fn establish_connection() {
    //Ugly setup
    let (mut endpoint, mut server_endpoint, remote, udp_socket) = setup_endpoints();
    let netconf = NetworkConfig::new2(remote, Transport::Quic);
    let mut buffer_pool = BufferPool::with_config(
        netconf.get_buffer_config(),
        netconf.get_custom_allocator(),
    );
    let udp_buffer = buffer_pool
    .get_buffer()
    .expect("Could not get buffer for setting up UDP");
    let buffer_pool_refcell = RefCell::new(buffer_pool);
    let mut cfg = KompactConfig::default();
    cfg.system_components(DeadletterBox::new, netconf.clone().build());
    let system = cfg.build().expect("KompactSystem");
    let logger = system.logger().clone();
    let mut udp_state = UdpState::new(udp_socket, udp_buffer, logger, &netconf);


    //Start connecting the client
    let client_ch = endpoint.connect(remote);

    match endpoint.try_write_quic(Instant::now(), &mut udp_state) {
        Ok(_) => {}
        Err(e) => {
            println!("Error during QUIC reading: {}", e);
        }
    }

    thread::sleep(Duration::from_secs(1));

    //Server should be handshake data ready 
    match server_endpoint.try_read_quic(Instant::now(), remote,&mut udp_state, &buffer_pool_refcell) {
        Ok(_) => {
        }
        Err(e) => {
            println!("Error during QUIC reading: {}", e);
        }
    }   

    endpoint.try_read_quic(Instant::now(), remote, &mut udp_state, &buffer_pool_refcell);


    // let server_ch = server_endpoint.accepted.take().expect("server didn't connect");
    // assert_matches!(
    //     server_endpoint.connections.get_mut(&server_ch).unwrap().poll(),
    //     Some(quinn_proto::Event::HandshakeDataReady)    
    // ); 
    
    thread::sleep(Duration::from_secs(1));
 
    match endpoint.try_read_quic(Instant::now(), remote, &mut udp_state, &buffer_pool_refcell) {
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
  //  let s = endpoint.streams(client_ch).open(Dir::Uni).unwrap();
  //  const MSG: &[u8] = b"hello";
  //  endpoint.send(client_ch, s).write(MSG).unwrap();

    endpoint.try_write_quic(Instant::now(), &mut udp_state);

    thread::sleep(Duration::from_secs(1));

    //Server should be handshake data ready 
    server_endpoint.try_read_quic(Instant::now(), remote, &mut udp_state, &buffer_pool_refcell);

   endpoint.try_read_quic(Instant::now(), remote, &mut udp_state, &buffer_pool_refcell);
    
}

#[test]
fn finish_stream() {

}