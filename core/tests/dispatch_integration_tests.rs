use crossbeam_channel::Receiver as Rcv;
use kompact::{prelude::*, prelude_test::net_test_helpers::*};
use std::{net::SocketAddr, sync::Arc, thread, time::Duration};

const REGISTRATION_TIMEOUT: Duration = Duration::from_millis(1000);
const STOP_COMPONENT_TIMEOUT: Duration = Duration::from_millis(1000);
const PINGPONG_TIMEOUT: Duration = Duration::from_millis(10_000);
const PING_INTERVAL: Duration = Duration::from_millis(500);
const CONNECTION_STATUS_TIMEOUT: Duration = Duration::from_millis(5000);

const CONNECTION_RETRY_INTERVAL: u64 = 500;
const CONNECTION_RETRY_ATTEMPTS: u8 = 10;
const DROP_CONNECTION_TIMEOUT: Duration =
    Duration::from_millis(CONNECTION_RETRY_INTERVAL * (CONNECTION_RETRY_ATTEMPTS as u64 + 3));
const SMALL_CHUNK_SIZE: usize = 128;
const MINIMUM_UDP_CHUNK_SIZE: usize = 66000;
// BigPings with >0 data size provide stricter checks on data-correctness than regular pings...
const ARBITRARY_DATA_SIZE: usize = 500;

fn system_from_network_config(network_config: NetworkConfig) -> KompactSystem {
    let mut cfg = KompactConfig::default();
    cfg.system_components(DeadletterBox::new, network_config.build());
    cfg.build().expect("KompactSystem")
}

fn start_pinger(
    system: &KompactSystem,
    mut pinger_actor: PingerAct,
) -> (Arc<Component<PingerAct>>, KFuture<()>) {
    let all_pongs_received_future = pinger_actor.completion_future();
    let (pinger, reg_future) = system.create_and_register(move || pinger_actor);
    reg_future.wait_expect(REGISTRATION_TIMEOUT, "Pinger failed to register!");
    system.start(&pinger);
    (pinger, all_pongs_received_future)
}

fn start_big_pinger(
    system: &KompactSystem,
    mut big_pinger: BigPingerAct,
) -> (Arc<Component<BigPingerAct>>, KFuture<()>) {
    let all_pongs_received_future = big_pinger.completion_future();
    let (pinger, reg_future) = system.create_and_register(move || big_pinger);
    reg_future.wait_expect(REGISTRATION_TIMEOUT, "Pinger failed to register!");
    system.start(&pinger);
    (pinger, all_pongs_received_future)
}

fn start_ponger(
    system: &KompactSystem,
    ponger_actor: PongerAct,
) -> (Arc<Component<PongerAct>>, ActorPath) {
    let (ponger, pof) = system.create_and_register(move || ponger_actor);
    let ponger_path = system.actor_path_for(&ponger);
    pof.wait_expect(REGISTRATION_TIMEOUT, "Ponger failed to register!");
    system.start(&ponger);
    (ponger, ponger_path)
}

fn start_big_ponger(
    system: &KompactSystem,
    big_ponger: BigPongerAct,
) -> (Arc<Component<BigPongerAct>>, ActorPath) {
    let (ponger, pof) = system.create_and_register(move || big_ponger);
    let path = system.actor_path_for(&ponger);
    pof.wait_expect(REGISTRATION_TIMEOUT, "Ponger failed to register!");
    system.start(&ponger);
    (ponger, path)
}

fn start_ping_stream(system: &KompactSystem, target: &ActorPath) -> Arc<Component<PingStream>> {
    let (pinger, pif) =
        system.create_and_register(move || PingStream::new(target.clone(), PING_INTERVAL));
    pif.wait_expect(REGISTRATION_TIMEOUT, "Pinger failed to register!");
    system.start(&pinger);
    pinger
}

fn start_status_counter(
    system: &KompactSystem,
) -> (Arc<Component<NetworkStatusCounter>>, NetworkStatusReceiver) {
    let (sender, receiver) = crossbeam_channel::bounded(100);
    let (status_counter, reg_future) = system.create_and_register(NetworkStatusCounter::new);
    let started_future = status_counter.on_definition(|c| {
        c.set_status_sender(sender);
        system.connect_network_status_port(&mut c.network_status_port);
        c.started_future()
    });
    reg_future.wait_expect(REGISTRATION_TIMEOUT, "StatusCounter failed to register!");
    system.start(&status_counter);
    started_future
        .wait_timeout(REGISTRATION_TIMEOUT)
        .expect("StatusCounter failed to start");
    (status_counter, NetworkStatusReceiver { receiver })
}

struct NetworkStatusReceiver {
    receiver: Rcv<NetworkStatus>,
}

impl NetworkStatusReceiver {
    fn expect_connection_established(&self, timeout: Duration) {
        match self.receiver.recv_timeout(timeout) {
            Ok(NetworkStatus::ConnectionEstablished(_, _)) => {}
            Ok(other_status) => {
                panic!(
                    "unexpected network status {:?} waiting for ConnectionEstablished",
                    other_status
                )
            }
            Err(_) => {
                panic!("ConnectionStatus timeout waiting for ConnectionEstablished")
            }
        }
    }

    fn expect_connection_lost(&self, timeout: Duration) {
        match self.receiver.recv_timeout(timeout) {
            Ok(NetworkStatus::ConnectionLost(_, _)) => {}
            Ok(other_status) => {
                panic!(
                    "unexpected network status {:?} waiting for ConnectionLost",
                    other_status
                )
            }
            Err(_) => {
                panic!("ConnectionStatus timeout waiting for ConnectionLost")
            }
        }
    }

    fn expect_connection_dropped(&self, timeout: Duration) {
        match self.receiver.recv_timeout(timeout) {
            Ok(NetworkStatus::ConnectionDropped(_)) => {}
            Ok(other_status) => {
                panic!(
                    "unexpected network status {:?} waiting for ConnectionDropped",
                    other_status
                )
            }
            Err(_) => {
                panic!("ConnectionStatus timeout waiting for ConnectionDropped")
            }
        }
    }

    fn expect_connection_closed(&self, timeout: Duration) {
        match self.receiver.recv_timeout(timeout) {
            Ok(NetworkStatus::ConnectionClosed(_, _)) => {}
            Ok(other_status) => {
                panic!(
                    "unexpected network status {:?} waiting for ConnectionClosed",
                    other_status
                )
            }
            Err(_) => {
                panic!("ConnectionStatus timeout waiting for ConnectionClosed")
            }
        }
    }

    fn expect_blocked_system(&self, timeout: Duration) {
        match self.receiver.recv_timeout(timeout) {
            Ok(NetworkStatus::BlockedSystem(_)) => {}
            Ok(other_status) => {
                panic!(
                    "unexpected network status {:?} waiting for BlockedSystem",
                    other_status
                )
            }
            Err(_) => {
                panic!("ConnectionStatus timeout waiting for BlockedSystem")
            }
        }
    }

    fn expect_unblocked_system(&self, timeout: Duration) {
        match self.receiver.recv_timeout(timeout) {
            Ok(NetworkStatus::UnblockedSystem(_)) => {}
            Ok(other_status) => {
                panic!(
                    "unexpected network status {:?} waiting for UnblockedSystem",
                    other_status
                )
            }
            Err(_) => {
                panic!("ConnectionStatus timeout waiting for UnblockedSystem")
            }
        }
    }

    fn expect_blocked_ip(&self, timeout: Duration) {
        match self.receiver.recv_timeout(timeout) {
            Ok(NetworkStatus::BlockedIp(_)) => {}
            Ok(other_status) => {
                panic!(
                    "unexpected network status {:?} waiting for BlockedIp",
                    other_status
                )
            }
            Err(_) => {
                panic!("ConnectionStatus timeout waiting for BlockedIp")
            }
        }
    }

    fn expect_unblocked_ip(&self, timeout: Duration) {
        match self.receiver.recv_timeout(timeout) {
            Ok(NetworkStatus::UnblockedIp(_)) => {}
            Ok(other_status) => {
                panic!(
                    "unexpected network status {:?} waiting for UnblockedIp",
                    other_status
                )
            }
            Err(_) => {
                panic!("ConnectionStatus timeout waiting for UnblockedIp")
            }
        }
    }

    fn expect_connection_soft_limit_exceeded(&self, timeout: Duration) {
        match self.receiver.recv_timeout(timeout) {
            Ok(NetworkStatus::SoftConnectionLimitExceeded) => {}
            Ok(other_status) => {
                panic!(
                    "unexpected network status {:?} waiting for SoftConnectionLimitExceeded",
                    other_status
                )
            }
            Err(_) => {
                panic!("ConnectionStatus timeout waiting for SoftConnectionLimitExceeded")
            }
        }
    }

    fn expect_connection_hard_limit_reached(&self, timeout: Duration) {
        match self.receiver.recv_timeout(timeout) {
            Ok(NetworkStatus::HardConnectionLimitReached) => {}
            Ok(other_status) => {
                panic!(
                    "unexpected network status {:?} waiting for HardConnectionLimitReached",
                    other_status
                )
            }
            Err(_) => {
                panic!("ConnectionStatus timeout waiting for HardConnectionLimitReached")
            }
        }
    }
}

#[test]
fn named_registration() {
    const ACTOR_NAME: &str = "ponger";
    let system = system_from_network_config(NetworkConfig::default());

    let ponger = system.create(PongerAct::new_lazy);
    system.start(&ponger);

    let _res = system.register_by_alias(&ponger, ACTOR_NAME).wait_expect(
        REGISTRATION_TIMEOUT,
        "Single registration with unique alias should succeed.",
    );

    let res = system
        .register_by_alias(&ponger, ACTOR_NAME)
        .wait_timeout(REGISTRATION_TIMEOUT)
        .expect("Registration never completed.");

    assert_eq!(
        res,
        Err(RegistrationError::DuplicateEntry),
        "Duplicate alias registration should fail."
    );

    system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger did not die");

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

// Sets up two KompactSystems with 2x Pingers and Pongers. One Ponger is registered by UUID,
// the other by a custom name. One Pinger communicates with the UUID-registered Ponger,
// the other with the named Ponger. Both sets are expected to exchange PING_COUNT ping-pong
// messages.
#[test]
fn remote_delivery_to_registered_actors_eager() {
    let pinger_system = system_from_network_config(NetworkConfig::default());
    let ponger_system = system_from_network_config(NetworkConfig::default());

    let (ponger_unique, ponger_unique_path) = start_ponger(&ponger_system, PongerAct::new_eager());
    let (ponger_named, _) = start_ponger(&ponger_system, PongerAct::new_eager());
    let ponger_named_path = ponger_system
        .register_by_alias(&ponger_named, "custom_name")
        .wait_expect(REGISTRATION_TIMEOUT, "Ponger failed to register!");

    let (pinger_unique, all_unique_pongs_received_future) =
        start_pinger(&pinger_system, PingerAct::new_eager(ponger_unique_path));
    let (pinger_named, all_named_pongs_received_future) =
        start_pinger(&pinger_system, PingerAct::new_eager(ponger_named_path));

    all_unique_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");
    all_named_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    pinger_system
        .stop_notify(&pinger_unique)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    pinger_system
        .stop_notify(&pinger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");
    ponger_system
        .kill_notify(ponger_unique)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system
        .kill_notify(ponger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    pinger_unique.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems, one with a BigPinger and one with a BigPonger.
// BigPonger will validate the BigPing messages on reception, BigPinger counts replies
fn remote_delivery_bigger_than_buffer_messages_lazy_tcp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(SMALL_CHUNK_SIZE);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg);
    let ponger_system = system_from_network_config(net_cfg.clone());
    let pinger_system = system_from_network_config(net_cfg);

    let (ponger, ponger_path) = start_big_ponger(&ponger_system, BigPongerAct::new_lazy());

    let (pinger, all_pongs_received_future) = start_big_pinger(
        &pinger_system,
        BigPingerAct::new_lazy(ponger_path, SMALL_CHUNK_SIZE),
    );

    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Should complete");

    pinger_system
        .stop_notify(&pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems, one with a BigPinger and one with a BigPonger.
// BigPonger will validate the BigPing messages on reception, BigPinger counts replies
fn remote_delivery_bigger_than_buffer_messages_eager_tcp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(SMALL_CHUNK_SIZE);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg.clone());
    let ponger_system = system_from_network_config(net_cfg.clone());
    let pinger_system = system_from_network_config(net_cfg);

    let (ponger, ponger_path) =
        start_big_ponger(&ponger_system, BigPongerAct::new_eager(buf_cfg.clone()));

    let (pinger, pinger_complete_future) = start_big_pinger(
        &pinger_system,
        BigPingerAct::new_eager(ponger_path, SMALL_CHUNK_SIZE, buf_cfg),
    );

    pinger_complete_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Should complete");

    pinger_system
        .stop_notify(&pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems, one with a BigPinger and one with a BigPonger.
// BigPonger will validate the BigPing messages on reception, BigPinger counts replies
fn remote_delivery_bigger_than_buffer_messages_preserialised_tcp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(SMALL_CHUNK_SIZE);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg.clone());
    let ponger_system = system_from_network_config(net_cfg.clone());
    let pinger_system = system_from_network_config(net_cfg);

    let (ponger, ponger_path) =
        start_big_ponger(&ponger_system, BigPongerAct::new_eager(buf_cfg.clone()));

    let (pinger, all_pongs_received_future) = start_big_pinger(
        &pinger_system,
        BigPingerAct::new_preserialised(ponger_path, SMALL_CHUNK_SIZE, buf_cfg),
    );

    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Should complete");

    pinger_system
        .stop_notify(&pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Checks that BufferSwaps will occur for UDP sending/receiving during lazy sending
fn remote_delivery_bigger_than_buffer_messages_lazy_udp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(MINIMUM_UDP_CHUNK_SIZE);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg);
    let ponger_system = system_from_network_config(net_cfg.clone());
    let pinger_system = system_from_network_config(net_cfg);

    let (ponger, mut ponger_path) = start_big_ponger(&ponger_system, BigPongerAct::new_lazy());
    ponger_path.via_udp();
    let (pinger, all_pongs_received_future) = start_big_pinger(
        &pinger_system,
        BigPingerAct::new_lazy(ponger_path, MINIMUM_UDP_CHUNK_SIZE / PING_COUNT as usize),
    );

    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Should complete");

    pinger_system
        .stop_notify(&pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Checks that BufferSwaps will occur for UDP sending/receiving during eager sending
fn remote_delivery_bigger_than_buffer_messages_eager_udp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(MINIMUM_UDP_CHUNK_SIZE);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg.clone());
    let ponger_system = system_from_network_config(net_cfg.clone());
    let pinger_system = system_from_network_config(net_cfg);

    let (ponger, mut ponger_path) =
        start_big_ponger(&ponger_system, BigPongerAct::new_eager(buf_cfg.clone()));
    ponger_path.via_udp();
    let (pinger, all_pongs_received_future) = start_big_pinger(
        &pinger_system,
        BigPingerAct::new_eager(
            ponger_path,
            MINIMUM_UDP_CHUNK_SIZE / PING_COUNT as usize,
            buf_cfg,
        ),
    );

    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Should complete");

    pinger_system
        .stop_notify(&pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Checks that BufferSwaps will occur for UDP sending/receiving during preserialized sends
fn remote_delivery_bigger_than_buffer_messages_preserialised_udp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(MINIMUM_UDP_CHUNK_SIZE);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg.clone());
    let ponger_system = system_from_network_config(net_cfg.clone());
    let pinger_system = system_from_network_config(net_cfg);

    let (ponger, mut ponger_path) =
        start_big_ponger(&ponger_system, BigPongerAct::new_eager(buf_cfg.clone()));
    ponger_path.via_udp();
    let (pinger, all_pongs_received_future) = start_big_pinger(
        &pinger_system,
        BigPingerAct::new_preserialised(
            ponger_path,
            MINIMUM_UDP_CHUNK_SIZE / PING_COUNT as usize,
            buf_cfg,
        ),
    );

    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Should complete");

    pinger_system
        .stop_notify(&pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems with 2x Pingers and Pongers. One Ponger is registered by UUID,
// the other by a custom name. One Pinger communicates with the UUID-registered Ponger,
// the other with the named Ponger. Both sets are expected to exchange PING_COUNT ping-pong
// messages.
fn remote_delivery_to_registered_actors_eager_mixed_udp() {
    let pinger_system = system_from_network_config(NetworkConfig::default());
    let ponger_system = system_from_network_config(NetworkConfig::default());

    let (ponger_unique, mut ponger_unique_path) =
        start_ponger(&ponger_system, PongerAct::new_eager());
    let (ponger_named, _) = start_ponger(&ponger_system, PongerAct::new_eager());
    let mut ponger_named_path = ponger_system
        .register_by_alias(&ponger_named, "custom_name")
        .wait_expect(REGISTRATION_TIMEOUT, "Ponger failed to register!");
    ponger_named_path.via_udp();
    ponger_unique_path.via_udp();
    let (pinger_unique, all_unique_pongs_received_future) =
        start_pinger(&pinger_system, PingerAct::new_eager(ponger_unique_path));
    let (pinger_named, all_named_pongs_received_future) =
        start_pinger(&pinger_system, PingerAct::new_eager(ponger_named_path));

    all_unique_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");
    all_named_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    pinger_system
        .stop_notify(&pinger_unique)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    pinger_system
        .stop_notify(&pinger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");
    ponger_system
        .kill_notify(ponger_unique)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system
        .kill_notify(ponger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    pinger_unique.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems with 2x Pingers and Pongers. One Ponger is registered by UUID,
// the other by a custom name. One Pinger communicates with the UUID-registered Ponger,
// the other with the named Ponger. Both sets are expected to exchange PING_COUNT ping-pong
// messages.
fn remote_delivery_to_registered_actors_lazy() {
    let pinger_system = system_from_network_config(NetworkConfig::default());
    let ponger_system = system_from_network_config(NetworkConfig::default());

    let (ponger_unique, ponger_unique_path) = start_ponger(&ponger_system, PongerAct::new_lazy());
    let (ponger_named, _) = start_ponger(&ponger_system, PongerAct::new_lazy());
    let ponger_named_path = ponger_system
        .register_by_alias(&ponger_named, "custom_name")
        .wait_expect(REGISTRATION_TIMEOUT, "Ponger failed to register!");

    let (pinger_unique, all_unique_pongs_received_future) =
        start_pinger(&pinger_system, PingerAct::new_lazy(ponger_unique_path));
    let (pinger_named, all_named_pongs_received_future) =
        start_pinger(&pinger_system, PingerAct::new_lazy(ponger_named_path));

    all_unique_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");
    all_named_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    pinger_system
        .stop_notify(&pinger_unique)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    pinger_system
        .stop_notify(&pinger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");
    ponger_system
        .kill_notify(ponger_unique)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system
        .kill_notify(ponger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    pinger_unique.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems with 2x Pingers and Pongers. One Ponger is registered by UUID,
// the other by a custom name. One Pinger communicates with the UUID-registered Ponger,
// the other with the named Ponger. Both sets are expected to exchange PING_COUNT ping-pong
// messages.
fn remote_delivery_to_registered_actors_lazy_mixed_udp() {
    let pinger_system = system_from_network_config(NetworkConfig::default());
    let ponger_system = system_from_network_config(NetworkConfig::default());

    let (ponger_unique, mut ponger_unique_path) =
        start_ponger(&ponger_system, PongerAct::new_lazy());
    let (ponger_named, _) = start_ponger(&ponger_system, PongerAct::new_lazy());
    let mut ponger_named_path = ponger_system
        .register_by_alias(&ponger_named, "custom_name")
        .wait_expect(REGISTRATION_TIMEOUT, "Ponger failed to register!");
    ponger_unique_path.via_udp();
    ponger_named_path.via_udp();

    let (pinger_unique, all_unique_pongs_received_future) =
        start_pinger(&pinger_system, PingerAct::new_lazy(ponger_unique_path));
    let (pinger_named, all_named_pongs_received_future) =
        start_pinger(&pinger_system, PingerAct::new_lazy(ponger_named_path));

    all_unique_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");
    all_named_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    pinger_system
        .stop_notify(&pinger_unique)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    pinger_system
        .stop_notify(&pinger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");
    ponger_system
        .kill_notify(ponger_unique)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system
        .kill_notify(ponger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    pinger_unique.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems 1 and 2a, with Named paths. It first spawns a pinger-ponger couple
// The ping pong process completes, it shuts down system2a, spawns a new pinger on system1
// and finally boots a new system 2b with an identical networkconfig and named actor registration
// The final ping pong round should then complete as system1 automatically reconnects to system2 and
// transfers the enqueued messages.
// Also checks that `session` of received messages are incremented and matches up with NetworkStatus
// messages.
#[ignore]
fn remote_lost_and_continued_connection() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_max_connection_retry_attempts(CONNECTION_RETRY_ATTEMPTS);
    net_cfg.set_connection_retry_interval(CONNECTION_RETRY_INTERVAL);
    let pinger_system = system_from_network_config(net_cfg);
    let ponger_system_1 = system_from_network_config(NetworkConfig::default());
    let ponger_system_port = ponger_system_1.system_path().port();
    let (status_counter, status_receiver) = start_status_counter(&pinger_system);

    let (ponger_named, _) = start_ponger(&ponger_system_1, PongerAct::new_lazy());
    ponger_system_1
        .register_by_alias(&ponger_named, "custom_name")
        .wait_expect(REGISTRATION_TIMEOUT, "Ponger failed to register!");
    let named_path = ActorPath::Named(NamedPath::with_system(
        ponger_system_1.system_path(),
        vec!["custom_name".into()],
    ));

    // PingStream ensures that the network layer detects failures
    let ping_stream = start_ping_stream(&pinger_system, &named_path);
    ponger_system_1.start(&ponger_named);

    status_receiver.expect_connection_established(CONNECTION_STATUS_TIMEOUT);

    let (pinger_1, all_pongs_received_future_1) =
        start_pinger(&pinger_system, PingerAct::new_lazy(named_path.clone()));

    all_pongs_received_future_1
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    ponger_system_1
        .kill_notify(ponger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");
    // We now kill remote_a
    ponger_system_1.kill_system().ok();

    status_receiver.expect_connection_lost(CONNECTION_STATUS_TIMEOUT);

    // Start up remote_b
    let mut addr: SocketAddr = "127.0.0.1:0".parse().expect("Address should work");
    addr.set_port(ponger_system_port);
    let ponger_system_2 = system_from_network_config(NetworkConfig::new(addr));
    let (ponger_named, _) = start_ponger(&ponger_system_2, PongerAct::new_lazy());
    ponger_system_2
        .register_by_alias(&ponger_named, "custom_name")
        .wait_expect(REGISTRATION_TIMEOUT, "Ponger failed to register!");

    status_receiver.expect_connection_established(DROP_CONNECTION_TIMEOUT);

    let (pinger_2, all_pongs_received_future_2) =
        start_pinger(&pinger_system, PingerAct::new_lazy(named_path));

    all_pongs_received_future_2
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    let (first_session, second_session) = {
        status_counter.on_definition(|c| {
            assert_eq!(c.connected_systems[0].1, c.disconnected_systems[0].1);
            assert_ne!(c.connected_systems[0].1, c.connected_systems[1].1);
            (c.connected_systems[0].1, c.connected_systems[1].1)
        })
    };
    ping_stream.on_definition(|c| {
        assert_eq!(first_session, c.pong_system_paths[0].1);
        assert_eq!(second_session, c.pong_system_paths[1].1);
    });

    pinger_system
        .stop_notify(&ping_stream)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    pinger_system
        .stop_notify(&pinger_1)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    pinger_system
        .stop_notify(&pinger_2)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system_2
        .kill_notify(ponger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    ponger_system_2
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
#[ignore]
fn remote_lost_and_dropped_connection() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_max_connection_retry_attempts(CONNECTION_RETRY_ATTEMPTS);
    net_cfg.set_connection_retry_interval(CONNECTION_RETRY_INTERVAL);
    let pinger_system = system_from_network_config(net_cfg);
    let ponger_system_1 = system_from_network_config(NetworkConfig::default());
    let ponger_system_port = ponger_system_1.system_path().port();

    let (ponger_named, _) = start_ponger(&ponger_system_1, PongerAct::new_lazy());
    ponger_system_1
        .register_by_alias(&ponger_named, "custom_name")
        .wait_expect(REGISTRATION_TIMEOUT, "Ponger failed to register!");
    let named_path = ActorPath::Named(NamedPath::with_system(
        ponger_system_1.system_path(),
        vec!["custom_name".into()],
    ));

    let (_, status_receiver) = start_status_counter(&pinger_system);

    // PingStream ensures that the network layer detects failures
    let _ = start_ping_stream(&pinger_system, &named_path);

    let (pinger_named, all_pongs_received_future) =
        start_pinger(&pinger_system, PingerAct::new_lazy(named_path.clone()));
    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");
    status_receiver.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    ponger_system_1
        .kill_notify(ponger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    // We now kill system2
    ponger_system_1.kill_system().ok();
    status_receiver.expect_connection_lost(CONNECTION_STATUS_TIMEOUT);

    // Start a new pinger on system
    let (pinger_named_2, _) = start_pinger(&pinger_system, PingerAct::new_lazy(named_path.clone()));
    // Wait for it to send its pings but no progress should be made
    // Assert that things are going as they should be, pong count has not increased
    pinger_named_2.on_definition(|c| {
        assert_eq!(c.count, 0);
    });
    status_receiver.expect_connection_dropped(DROP_CONNECTION_TIMEOUT);

    // Start up remote_b
    let mut addr: SocketAddr = "127.0.0.1:0".parse().expect("Address should work");
    addr.set_port(ponger_system_port);
    let ponger_system_2 = system_from_network_config(NetworkConfig::new(addr));

    let (ponger_named, _) = start_ponger(&ponger_system_2, PongerAct::new_lazy());
    ponger_system_2
        .register_by_alias(&ponger_named, "custom_name")
        .wait_expect(REGISTRATION_TIMEOUT, "Ponger failed to register!");

    // This one should now succeed
    let (pinger_named_3, all_pongs_received_future) =
        start_pinger(&pinger_system, PingerAct::new_lazy(named_path));

    // Wait for it to send its pings, system should recognize the remote address
    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    // Ensure that the new connection worked
    pinger_named_3.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    // Assert that the old messages were dropped
    pinger_named_2.on_definition(|c| {
        assert_eq!(c.count, 0);
    });

    pinger_system
        .stop_notify(&pinger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    pinger_system
        .stop_notify(&pinger_named_2)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    pinger_system
        .stop_notify(&pinger_named_3)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system_2
        .kill_notify(ponger_named)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    ponger_system_2
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn local_delivery() {
    let system = system_from_network_config(NetworkConfig::default());

    let (ponger, mut ponger_path) = start_ponger(&system, PongerAct::new_lazy());
    ponger_path.set_protocol(Transport::Local);
    let (pinger, pinger_done_future) = start_pinger(&system, PingerAct::new_lazy(ponger_path));
    pinger_done_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    system
        .stop_notify(&pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn local_forwarding() {
    let system = system_from_network_config(NetworkConfig::default());

    let (ponger, mut ponger_path) = start_big_ponger(&system, BigPongerAct::new_lazy());
    ponger_path.set_protocol(Transport::Local);

    let (forwarder, fof) = system.create_and_register(move || ForwarderAct::new(ponger_path));
    let mut forwarder_path = fof.wait_expect(REGISTRATION_TIMEOUT, "Forwarder failed to register!");
    forwarder_path.set_protocol(Transport::Local);
    system.start(&forwarder);

    let (pinger, all_pongs_received_future) = start_big_pinger(
        &system,
        BigPingerAct::new_lazy(forwarder_path, ARBITRARY_DATA_SIZE),
    );

    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");
    system
        .kill_notify(forwarder)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Forwarder never died!");
    system
        .stop_notify(&pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn local_forwarding_eager() {
    let system = system_from_network_config(NetworkConfig::default());

    let (ponger, mut ponger_path) = start_big_ponger(&system, BigPongerAct::new_lazy());
    ponger_path.set_protocol(Transport::Local);

    let (forwarder, fof) = system.create_and_register(move || ForwarderAct::new(ponger_path));
    let mut forwarder_path = fof.wait_expect(REGISTRATION_TIMEOUT, "Forwarder failed to register!");
    forwarder_path.set_protocol(Transport::Local);
    system.start(&forwarder);

    let (pinger, all_pongs_received_future) = start_big_pinger(
        &system,
        BigPingerAct::new_eager(forwarder_path, ARBITRARY_DATA_SIZE, BufferConfig::default()),
    );

    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");
    system
        .kill_notify(forwarder)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Forwarder never died!");
    system
        .stop_notify(&pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn remote_forwarding_unique() {
    let ponger_system = system_from_network_config(NetworkConfig::default());
    let forwarder_system = system_from_network_config(NetworkConfig::default());
    let pinger_system = system_from_network_config(NetworkConfig::default());

    let (ponger, ponger_path) = start_big_ponger(&ponger_system, BigPongerAct::new_lazy());

    let (forwarder, fof) =
        forwarder_system.create_and_register(move || ForwarderAct::new(ponger_path));
    let forwarder_path = fof.wait_expect(REGISTRATION_TIMEOUT, "Forwarder failed to register!");
    forwarder_system.start(&forwarder);

    let (pinger, all_pongs_received_future) = start_big_pinger(
        &pinger_system,
        BigPingerAct::new_lazy(forwarder_path, ARBITRARY_DATA_SIZE),
    );

    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    ponger_system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");
    forwarder_system
        .kill_notify(forwarder)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Forwarder never died!");
    pinger_system
        .stop_notify(&pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    forwarder_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn remote_forwarding_unique_two_systems() {
    let ponger_system = system_from_network_config(NetworkConfig::default());
    let pinger_system = system_from_network_config(NetworkConfig::default());

    let (ponger, ponger_path) = start_big_ponger(&ponger_system, BigPongerAct::new_lazy());

    let (forwarder, fof) =
        pinger_system.create_and_register(move || ForwarderAct::new(ponger_path));
    let forwarder_path = fof.wait_expect(REGISTRATION_TIMEOUT, "Forwarder failed to register!");
    pinger_system.start(&forwarder);

    let (pinger, all_pongs_received_future) = start_big_pinger(
        &pinger_system,
        BigPingerAct::new_lazy(forwarder_path, ARBITRARY_DATA_SIZE),
    );

    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    ponger_system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");
    pinger_system
        .kill_notify(forwarder)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Forwarder never died!");
    pinger_system
        .stop_notify(&pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn remote_forwarding_named() {
    let ponger_system = system_from_network_config(NetworkConfig::default());
    let forwarder_system = system_from_network_config(NetworkConfig::default());
    let pinger_system = system_from_network_config(NetworkConfig::default());

    let (ponger, _) = start_big_ponger(&ponger_system, BigPongerAct::new_lazy());

    let ponger_path = ponger_system
        .register_by_alias(&ponger, "ponger")
        .wait_expect(REGISTRATION_TIMEOUT, "Ponger failed to register!");

    let (forwarder, _fof) =
        forwarder_system.create_and_register(move || ForwarderAct::new(ponger_path));
    let forwarder_path = forwarder_system
        .register_by_alias(&forwarder, "forwarder")
        .wait_expect(REGISTRATION_TIMEOUT, "Forwarder failed to register!");
    forwarder_system.start(&forwarder);

    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(SMALL_CHUNK_SIZE);
    // Use bigger than buffer chunk messages to make it a more difficult test scenario
    let (pinger, all_pongs_received_future) = start_big_pinger(
        &pinger_system,
        BigPingerAct::new_lazy(forwarder_path, SMALL_CHUNK_SIZE * 2),
    );

    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    ponger_system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");
    forwarder_system
        .kill_notify(forwarder)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Forwarder never died!");
    pinger_system
        .stop_notify(&pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    forwarder_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn network_status_port_established_lost_dropped_connection() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_max_connection_retry_attempts(CONNECTION_RETRY_ATTEMPTS);
    net_cfg.set_connection_retry_interval(CONNECTION_RETRY_INTERVAL);
    let pinger_system = system_from_network_config(net_cfg.clone());
    let ponger_system = system_from_network_config(net_cfg);

    // Create a status_counter which will listen to the status port and count messages received
    let (status_counter, status_receiver) = start_status_counter(&pinger_system);

    // Create a pinger ponger pair such that the Network will be used.
    let (_, ponger_path) = start_ponger(&ponger_system, PongerAct::new_lazy());
    let (_, pinger_done_future) =
        start_pinger(&pinger_system, PingerAct::new_lazy(ponger_path.clone()));

    let _ = start_ping_stream(&pinger_system, &ponger_path);

    pinger_done_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Pinger should complete");

    // Shutdown the remote system and wait for the connection to be lost and dropped by local_system
    let _ = ponger_system.kill_system();

    status_receiver.expect_connection_established(CONNECTION_STATUS_TIMEOUT);

    status_receiver.expect_connection_lost(CONNECTION_STATUS_TIMEOUT);

    status_receiver.expect_connection_dropped(DROP_CONNECTION_TIMEOUT);

    status_counter.on_definition(|sc| {
        assert_eq!(sc.connection_established, 1, "Connection established count");
        assert_eq!(sc.connection_lost, 1, "Connection lost count");
        assert_eq!(sc.connection_dropped, 1, "Connection dropped count");
    });

    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn network_status_port_close_connection_closed_connection() {
    let ponger_system = system_from_network_config(NetworkConfig::default());
    let pinger_system = system_from_network_config(NetworkConfig::default());

    // Create a status_counter which will listen to the status port and count messages received
    let (ponger_system_status_counter, ponger_status_receiver) =
        start_status_counter(&ponger_system);
    let (pinger_system_status_counter, pinger_status_receiver) =
        start_status_counter(&pinger_system);

    // Create a pinger ponger pair such that the Network will be used.
    let (_, ponger_path) = start_ponger(&ponger_system, PongerAct::new_lazy());
    let (_, all_pongs_received_future) =
        start_pinger(&pinger_system, PingerAct::new_lazy(ponger_path));
    let ponger_system_path = ponger_system.system_path();

    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    pinger_system_status_counter.on_definition(|sc| {
        sc.send_status_request(NetworkStatusRequest::DisconnectSystem(ponger_system_path));
    });

    // Wait for the channel to be closed
    ponger_status_receiver.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    pinger_status_receiver.expect_connection_established(CONNECTION_STATUS_TIMEOUT);

    ponger_status_receiver.expect_connection_closed(CONNECTION_STATUS_TIMEOUT);
    pinger_status_receiver.expect_connection_closed(CONNECTION_STATUS_TIMEOUT);

    ponger_system_status_counter.on_definition(|sc| {
        assert_eq!(sc.connection_closed, 1);
    });
    pinger_system_status_counter.on_definition(|sc| {
        assert_eq!(sc.connection_closed, 1);
    });

    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn network_status_port_open_close_open() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_max_connection_retry_attempts(2);
    net_cfg.set_connection_retry_interval(1000);
    let initiator_system = system_from_network_config(net_cfg.clone());
    let receiver_system = system_from_network_config(net_cfg);

    let system_path = receiver_system.system_path();
    // Create a status_counter which will listen to the status port and count messages received
    let (status_counter, status_receiver) = start_status_counter(&initiator_system);

    status_counter.on_definition(|sc| {
        sc.send_status_request(NetworkStatusRequest::ConnectSystem(system_path.clone()));
    });
    status_receiver.expect_connection_established(CONNECTION_STATUS_TIMEOUT);

    status_counter.on_definition(|sc| {
        assert_eq!(sc.connection_established, 1);
        assert_eq!(sc.connection_closed, 0);
        sc.send_status_request(NetworkStatusRequest::DisconnectSystem(system_path.clone()));
    });
    status_receiver.expect_connection_closed(CONNECTION_STATUS_TIMEOUT);
    status_counter.on_definition(|sc| {
        assert_eq!(sc.connection_established, 1);
        assert_eq!(sc.connection_closed, 1);
        sc.send_status_request(NetworkStatusRequest::ConnectSystem(system_path.clone()));
    });
    status_receiver.expect_connection_established(DROP_CONNECTION_TIMEOUT);
    status_counter.on_definition(|sc| {
        assert_eq!(sc.connection_established, 2);
        assert_eq!(sc.connection_closed, 1);
        assert_eq!(sc.connected_systems[0], sc.disconnected_systems[0]);
        assert_ne!(sc.connected_systems[0], sc.connected_systems[1]);
    });

    initiator_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    receiver_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn network_status_port_block_unblock_system() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_max_connection_retry_attempts(CONNECTION_RETRY_ATTEMPTS);
    net_cfg.set_connection_retry_interval(CONNECTION_RETRY_INTERVAL);

    let ponger_system = system_from_network_config(net_cfg.clone());
    let pinger_system = system_from_network_config(net_cfg);

    // Create a status_counter which will listen to the status port and count messages received
    let (status_counter, ponger_status_receiver) = start_status_counter(&ponger_system);
    let (_, pinger_status_receiver) = start_status_counter(&pinger_system);

    let (ponger, ponger_path) = start_ponger(&ponger_system, PongerAct::new_lazy());

    let pinger = start_ping_stream(&pinger_system, &ponger_path);

    ponger_status_receiver.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    pinger_status_receiver.expect_connection_established(CONNECTION_STATUS_TIMEOUT);

    let pinger_sys_path = pinger_system.system_path();
    status_counter.on_definition(|sc| {
        sc.send_status_request(NetworkStatusRequest::BlockSystem(pinger_sys_path.clone()));
    });

    ponger_status_receiver.expect_blocked_system(CONNECTION_STATUS_TIMEOUT);
    pinger_status_receiver.expect_connection_lost(CONNECTION_STATUS_TIMEOUT);

    let before_block_count = ponger.on_definition(|ponger| ponger.count);

    // Give pings time to fail
    thread::sleep(PING_INTERVAL * 3);

    ponger.on_definition(|ponger| assert_eq!(before_block_count, ponger.count));

    status_counter.on_definition(|sc| {
        assert!(sc.blocked_systems.contains(&pinger_sys_path));
        sc.send_status_request(NetworkStatusRequest::UnblockSystem(pinger_sys_path.clone()));
    });
    ponger_status_receiver.expect_unblocked_system(CONNECTION_STATUS_TIMEOUT);

    ponger_status_receiver.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    pinger_status_receiver.expect_connection_established(CONNECTION_STATUS_TIMEOUT);

    let (_, all_pongs_received_future) =
        start_pinger(&pinger_system, PingerAct::new_eager(ponger_path));

    all_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    ponger.on_definition(|ponger| {
        assert!(
            before_block_count < ponger.count,
            "Should have received more pings after unblocking: before; {}, after: {}",
            before_block_count,
            ponger.count
        );
    });
    status_counter.on_definition(|sc| assert!(!sc.blocked_systems.contains(&pinger_sys_path)));

    pinger_system
        .kill_notify(pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");

    ponger_system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    let _ = ponger_system.shutdown();
    let _ = pinger_system.shutdown();
}

#[test]
fn network_status_port_block_unblock_ip() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_connection_retry_interval(CONNECTION_RETRY_INTERVAL);
    net_cfg.set_max_connection_retry_attempts(CONNECTION_RETRY_ATTEMPTS);
    let ponger_system = system_from_network_config(net_cfg.clone());
    let pinger_system = system_from_network_config(net_cfg);

    // Create a status_counter which will listen to the status port and count messages received
    let (status_counter, status_receiver) = start_status_counter(&ponger_system);

    let (ponger, ponger_path) = start_ponger(&ponger_system, PongerAct::new_eager());
    let _ = start_ping_stream(&pinger_system, &ponger_path);
    let pinger_ip = *pinger_system.system_path().address();

    let (_, all_pongs_received_future_1) =
        start_pinger(&pinger_system, PingerAct::new_lazy(ponger_path.clone()));
    all_pongs_received_future_1
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    status_counter.on_definition(|sc| {
        sc.send_status_request(NetworkStatusRequest::BlockIp(pinger_ip));
    });

    status_receiver.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    status_receiver.expect_blocked_ip(CONNECTION_STATUS_TIMEOUT);

    let before_block_count = ponger.on_definition(|ponger| ponger.count);
    let (_, all_pongs_received_future_2) =
        start_pinger(&pinger_system, PingerAct::new_lazy(ponger_path.clone()));
    all_pongs_received_future_2
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect_err("Expecting time out waiting for ping pong to complete");

    ponger.on_definition(|ponger| assert_eq!(before_block_count, ponger.count));
    status_counter.on_definition(|sc| {
        assert!(sc.blocked_ip.contains(&pinger_ip));
        sc.send_status_request(NetworkStatusRequest::UnblockIp(pinger_ip));
    });

    status_receiver.expect_unblocked_ip(CONNECTION_STATUS_TIMEOUT);
    status_receiver.expect_connection_established(CONNECTION_STATUS_TIMEOUT);

    let (_, all_pongs_received_future_3) =
        start_pinger(&pinger_system, PingerAct::new_lazy(ponger_path));
    all_pongs_received_future_3
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    status_counter.on_definition(|sc| assert!(!sc.blocked_ip.contains(&pinger_ip)));

    let _ = ponger_system.shutdown();
    let _ = pinger_system.shutdown();
}

#[test]
// Sets up three KompactSystems: One with a BigPonger, one with a BigPinger with big pings
// and one with a BigPinger with small pings. The big Pings are sent first and occupies
// all buffers of the BigPonger system. The small pings are then sent but can not be received
// until the Ponger system closes the Big Ping-channel due to too many retries.
// A new batch up small-pings are then sent and replied to.
fn remote_delivery_overflow_network_thread_buffers() {
    let mut buf_cfg = BufferConfig::default();
    let chunk_size = 1000;
    let chunk_count = 10;
    buf_cfg.chunk_size(chunk_size);
    buf_cfg.max_chunk_count(chunk_count);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg.clone());
    // We will attempt to establish a connection for 5 seconds before giving up.
    // This config is also used when giving up on running out of buffers.
    // The big_pinger_system will occupy all buffers on the ponger_system for 5 seconds
    // And then it will be freed.
    net_cfg.set_connection_retry_interval(CONNECTION_RETRY_INTERVAL);
    net_cfg.set_max_connection_retry_attempts(CONNECTION_RETRY_ATTEMPTS);

    let big_pinger_system = system_from_network_config(net_cfg.clone());
    let ponger_system = system_from_network_config(net_cfg.clone());
    net_cfg.set_max_connection_retry_attempts(CONNECTION_RETRY_ATTEMPTS * 3);
    let small_pinger_system = system_from_network_config(net_cfg);
    // Create the BigPonger on the Ponger system
    let (ponger, ponger_path) = start_big_ponger(&ponger_system, BigPongerAct::new_eager(buf_cfg));

    // Create the BIG pinger and expect it to fail
    let (big_pinger, all_big_pongs_received_future) = start_big_pinger(
        &big_pinger_system,
        BigPingerAct::new_preserialised(
            ponger_path.clone(),
            chunk_size * chunk_count,
            BufferConfig::default(),
        ),
    );
    all_big_pongs_received_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect_err("Expecting time out waiting for ping pong to complete");
    let (small_pinger, all_small_pongs_received_future) = start_big_pinger(
        &small_pinger_system,
        BigPingerAct::new_preserialised(ponger_path, chunk_size / 100, BufferConfig::default()),
    );
    big_pinger_system
        .stop_notify(&big_pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    big_pinger.on_definition(|c| {
        assert_eq!(c.count, 0);
    });
    big_pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");

    // ponger system should become unblocked and the ping pong should complete
    all_small_pongs_received_future
        .wait_timeout((DROP_CONNECTION_TIMEOUT * 3) + PINGPONG_TIMEOUT)
        .expect("Should Complete");

    small_pinger_system
        .stop_notify(&small_pinger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Pinger never stopped!");
    ponger_system
        .kill_notify(ponger)
        .wait_timeout(STOP_COMPONENT_TIMEOUT)
        .expect("Ponger never died!");

    small_pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    small_pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up four KompactSystems with a Soft Channel Limit of 2.
// Starts a Ponger on system1, then Pingers on system2, then on system3, then on system4.
// Asserts that system2 connection is dropped by system1
fn soft_connection_limit_exceeded() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_soft_connection_limit(2);

    let system1 = system_from_network_config(net_cfg.clone());
    let system2 = system_from_network_config(net_cfg.clone());
    let system3 = system_from_network_config(net_cfg.clone());
    let system4 = system_from_network_config(net_cfg);

    let (_, status_receiver1) = start_status_counter(&system1);
    let (_, status_receiver2) = start_status_counter(&system2);
    let (_, status_receiver3) = start_status_counter(&system3);

    let (_, ponger1_path) = start_ponger(&system1, PongerAct::new_lazy());
    let (_, ponger2_path) = start_ponger(&system2, PongerAct::new_lazy());

    let (_, pinger2_future) = start_pinger(&system2, PingerAct::new_lazy(ponger1_path.clone()));
    status_receiver1.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    status_receiver2.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    pinger2_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    let (_, pinger3_future) = start_pinger(&system3, PingerAct::new_lazy(ponger1_path.clone()));
    status_receiver1.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    status_receiver3.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    pinger3_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    let (_, pinger4_future) = start_pinger(&system4, PingerAct::new_lazy(ponger1_path));

    status_receiver1.expect_connection_soft_limit_exceeded(CONNECTION_STATUS_TIMEOUT);
    status_receiver2.expect_connection_closed(CONNECTION_STATUS_TIMEOUT);
    // Two equally valid orderings of status messages at this point:
    match status_receiver1
        .receiver
        .recv_timeout(CONNECTION_STATUS_TIMEOUT)
    {
        Ok(NetworkStatus::ConnectionClosed(systempath, _)) => {
            assert_eq!(systempath, system2.system_path());
            status_receiver1.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
        }
        Ok(NetworkStatus::ConnectionEstablished(systempath, _)) => {
            assert_eq!(systempath, system4.system_path());
            status_receiver1.expect_connection_closed(CONNECTION_STATUS_TIMEOUT);
        }
        _ => {
            panic!("Unexpected status messages")
        }
    }
    pinger4_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    // Create a pinger on system1 to ponger on system2
    let (_, pinger1_future) = start_pinger(&system1, PingerAct::new_lazy(ponger2_path));

    status_receiver1.expect_connection_soft_limit_exceeded(CONNECTION_STATUS_TIMEOUT);
    match status_receiver1
        .receiver
        .recv_timeout(CONNECTION_STATUS_TIMEOUT)
    {
        Ok(NetworkStatus::ConnectionClosed(systempath, _)) => {
            assert_eq!(systempath, system3.system_path());
            status_receiver1.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
        }
        Ok(NetworkStatus::ConnectionEstablished(systempath, _)) => {
            assert_eq!(systempath, system2.system_path());
            status_receiver1.expect_connection_closed(CONNECTION_STATUS_TIMEOUT);
        }
        _ => {
            panic!("Unexpected status messages")
        }
    }
    status_receiver3.expect_connection_closed(CONNECTION_STATUS_TIMEOUT);
    status_receiver2.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    pinger1_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    system1
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system2
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system3
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system4
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up four KompactSystems with a Hard Channel Limit of 2.
// Starts a Ponger on system1, then Pingers on system2, then on system3, then on system4.
// Asserts that system4 fails to achieve a connection
fn hard_connection_limit_exceeded() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_hard_connection_limit(2);

    let system1 = system_from_network_config(net_cfg.clone());
    let system2 = system_from_network_config(net_cfg.clone());
    let system3 = system_from_network_config(net_cfg.clone());
    let system4 = system_from_network_config(net_cfg);

    let (_, status_receiver1) = start_status_counter(&system1);
    let (_, status_receiver2) = start_status_counter(&system2);
    let (_, status_receiver3) = start_status_counter(&system3);

    let (_, ponger1_path) = start_ponger(&system1, PongerAct::new_lazy());
    let (_, ponger4_path) = start_ponger(&system4, PongerAct::new_lazy());

    let (_, pinger2_future) = start_pinger(&system2, PingerAct::new_lazy(ponger1_path.clone()));
    status_receiver1.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    status_receiver2.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    pinger2_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    let (_, pinger3_future) = start_pinger(&system3, PingerAct::new_lazy(ponger1_path.clone()));
    status_receiver1.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    status_receiver3.expect_connection_established(CONNECTION_STATUS_TIMEOUT);
    pinger3_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect("Time out waiting for ping pong to complete");

    let (_, pinger4_future) = start_pinger(&system4, PingerAct::new_lazy(ponger1_path));

    status_receiver1.expect_connection_hard_limit_reached(CONNECTION_STATUS_TIMEOUT);

    pinger4_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect_err("Connection attempt should fail");

    // Create a pinger on system1 to ponger on system4
    let (_, pinger1_future) = start_pinger(&system1, PingerAct::new_lazy(ponger4_path));

    status_receiver1.expect_connection_hard_limit_reached(CONNECTION_STATUS_TIMEOUT);
    pinger1_future
        .wait_timeout(PINGPONG_TIMEOUT)
        .expect_err("Connection attempt should fail");

    system1
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system2
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system3
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system4
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[allow(dead_code)]
#[cfg_attr(not(feature = "low_latency"), test)]
// Sets up one KompactSystem with one Ponger, with a hard_limit of 6 and soft_limit of 4.
// Then creates 30 KompactSystems with Pingers, each pinging the ponger.
// Eventually all Pings and Pongs should be received. The Connections are always gracefully closed,
// and thus messages are retained while the pinger-systems take turns achieving active connections.
fn hard_and_soft_connection_limit() {
    let mut net_cfg = NetworkConfig::default();
    let hard_limit = 6;
    let soft_limit = 4;
    let connection_retry_interval = 500;
    let max_connection_retry_attempts = 50; // 500ms*50 = 25 Sec
    let number_of_pingers = hard_limit * 5; // Kind of an arbitrary amount
    let timeout =
        Duration::from_millis(connection_retry_interval * max_connection_retry_attempts as u64)
            + PINGPONG_TIMEOUT;

    net_cfg.set_hard_connection_limit(hard_limit);
    net_cfg.set_soft_connection_limit(soft_limit);
    net_cfg.set_connection_retry_interval(connection_retry_interval);
    net_cfg.set_max_connection_retry_attempts(max_connection_retry_attempts);

    let ponger_system = system_from_network_config(net_cfg.clone());
    let (_, ponger_path) = start_ponger(&ponger_system, PongerAct::new_lazy());

    let mut pinger_systems = Vec::new();
    for _ in 0..number_of_pingers {
        pinger_systems.push(system_from_network_config(net_cfg.clone()))
    }

    let mut pingers = Vec::new();
    let mut pinger_futures = Vec::new();

    for system in &pinger_systems {
        let (pinger, future) = start_pinger(system, PingerAct::new_lazy(ponger_path.clone()));
        pingers.push(pinger);
        pinger_futures.push(future);
    }

    pinger_futures.expect_completion(timeout, {
        for pinger in pingers {
            pinger.on_definition(|p| {
                if p.count < 10 {
                    info!(
                        pinger.logger(),
                        "Connection Limit test failed, pong count: {}", p.count
                    );
                }
            });
        }
        "One or more pingers failed"
    });
}
