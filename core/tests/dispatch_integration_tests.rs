use kompact::{prelude::*, prelude_test::net_test_helpers::*};
use std::{net::SocketAddr, thread, time::Duration};

fn system_from_network_config(network_config: NetworkConfig) -> KompactSystem {
    let mut cfg = KompactConfig::new();
    cfg.system_components(DeadletterBox::new, network_config.build());
    cfg.build().expect("KompactSystem")
}

#[test]
fn named_registration() {
    const ACTOR_NAME: &str = "ponger";
    let system = system_from_network_config(NetworkConfig::default());

    let ponger = system.create(PongerAct::new_lazy);
    system.start(&ponger);

    let _res = system.register_by_alias(&ponger, ACTOR_NAME).wait_expect(
        Duration::from_millis(1000),
        "Single registration with unique alias should succeed.",
    );

    let res = system
        .register_by_alias(&ponger, ACTOR_NAME)
        .wait_timeout(Duration::from_millis(1000))
        .expect("Registration never completed.");

    assert_eq!(
        res,
        Err(RegistrationError::DuplicateEntry),
        "Duplicate alias registration should fail."
    );

    system
        .kill_notify(ponger)
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger did not die");
    thread::sleep(Duration::from_millis(1000));

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
    let system = system_from_network_config(NetworkConfig::default());
    let remote = system_from_network_config(NetworkConfig::default());
    let (ponger_unique, pouf) = remote.create_and_register(PongerAct::new_eager);
    let (ponger_named, ponf) = remote.create_and_register(PongerAct::new_eager);
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");

    let ponger_unique_path =
        pouf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let (pinger_unique, piuf) =
        system.create_and_register(move || PingerAct::new_eager(ponger_unique_path));
    let (pinger_named, pinf) =
        system.create_and_register(move || PingerAct::new_eager(ponger_named_path));

    piuf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    remote.start(&ponger_unique);
    remote.start(&ponger_named);
    system.start(&pinger_unique);
    system.start(&pinger_named);

    // TODO maybe we could do this a bit more reliable?
    thread::sleep(Duration::from_millis(7000));

    let pingfu = system.stop_notify(&pinger_unique);
    let pingfn = system.stop_notify(&pinger_named);
    let pongfu = remote.kill_notify(ponger_unique);
    let pongfn = remote.kill_notify(ponger_named);

    pingfu
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfu
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pingfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_unique.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems, one with a BigPinger and one with a BigPonger.
// BigPonger will validate the BigPing messages on reception, BigPinger counts replies
fn remote_delivery_bigger_than_buffer_messages_lazy_tcp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(128);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg);
    let system = system_from_network_config(net_cfg.clone());
    let remote = system_from_network_config(net_cfg);

    let (ponger_named, ponf) = remote.create_and_register(BigPongerAct::new_lazy);
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let (pinger_named, pinf) =
        system.create_and_register(move || BigPingerAct::new_lazy(ponger_named_path, 120));

    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    remote.start(&ponger_named);
    system.start(&pinger_named);

    // TODO maybe we could do this a bit more reliable?
    thread::sleep(Duration::from_millis(15000));

    let pingfn = system.stop_notify(&pinger_named);
    let pongfn = remote.kill_notify(ponger_named);

    pingfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems, one with a BigPinger and one with a BigPonger.
// BigPonger will validate the BigPing messages on reception, BigPinger counts replies
fn remote_delivery_bigger_than_buffer_messages_eager_tcp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(128);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg.clone());
    let system = system_from_network_config(net_cfg.clone());
    let remote = system_from_network_config(net_cfg);

    let (ponger_named, ponf) =
        remote.create_and_register(|| BigPongerAct::new_eager(buf_cfg.clone()));
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let (pinger_named, pinf) = system
        .create_and_register(move || BigPingerAct::new_eager(ponger_named_path, 120, buf_cfg));

    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    remote.start(&ponger_named);
    system.start(&pinger_named);

    // TODO maybe we could do this a bit more reliable?
    thread::sleep(Duration::from_millis(15000));

    let pingfn = system.stop_notify(&pinger_named);
    let pongfn = remote.kill_notify(ponger_named);

    pingfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems, one with a BigPinger and one with a BigPonger.
// BigPonger will validate the BigPing messages on reception, BigPinger counts replies
fn remote_delivery_bigger_than_buffer_messages_preserialised_tcp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(128);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg.clone());
    let system = system_from_network_config(net_cfg.clone());
    let remote = system_from_network_config(net_cfg);

    let (ponger_named, ponf) =
        remote.create_and_register(|| BigPongerAct::new_eager(buf_cfg.clone()));
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let (pinger_named, pinf) = system.create_and_register(move || {
        BigPingerAct::new_preserialised(ponger_named_path, 120, buf_cfg)
    });

    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    remote.start(&ponger_named);
    system.start(&pinger_named);

    // TODO maybe we could do this a bit more reliable?
    thread::sleep(Duration::from_millis(15000));

    let pingfn = system.stop_notify(&pinger_named);
    let pongfn = remote.kill_notify(ponger_named);

    pingfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems, one with a BigPinger and one with a BigPonger.
// BigPonger will validate the BigPing messages on reception, BigPinger counts replies
fn remote_delivery_bigger_than_buffer_messages_lazy_udp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(66000);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg);
    let system = system_from_network_config(net_cfg.clone());
    let remote = system_from_network_config(net_cfg);

    let (ponger_named, ponf) = remote.create_and_register(BigPongerAct::new_lazy);
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let mut ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_named_path.via_udp();
    let (pinger_named, pinf) =
        system.create_and_register(move || BigPingerAct::new_lazy(ponger_named_path, 1500 * 3 / 4));

    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    remote.start(&ponger_named);
    system.start(&pinger_named);

    // TODO maybe we could do this a bit more reliable?
    thread::sleep(Duration::from_millis(15000));

    let pingfn = system.stop_notify(&pinger_named);
    let pongfn = remote.kill_notify(ponger_named);

    pingfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems, one with a BigPinger and one with a BigPonger.
// BigPonger will validate the BigPing messages on reception, BigPinger counts replies
fn remote_delivery_bigger_than_buffer_messages_eager_udp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(66000);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg.clone());
    let system = system_from_network_config(net_cfg.clone());
    let remote = system_from_network_config(net_cfg);

    let (ponger_named, ponf) =
        remote.create_and_register(|| BigPongerAct::new_eager(buf_cfg.clone()));
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let mut ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_named_path.via_udp();
    let (pinger_named, pinf) = system.create_and_register(move || {
        BigPingerAct::new_eager(ponger_named_path, 1500 * 3 / 4, buf_cfg)
    });

    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    remote.start(&ponger_named);
    system.start(&pinger_named);

    // TODO maybe we could do this a bit more reliable?
    thread::sleep(Duration::from_millis(15000));

    let pingfn = system.stop_notify(&pinger_named);
    let pongfn = remote.kill_notify(ponger_named);

    pingfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems, one with a BigPinger and one with a BigPonger.
// BigPonger will validate the BigPing messages on reception, BigPinger counts replies
fn remote_delivery_bigger_than_buffer_messages_preserialised_udp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(66000);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg.clone());
    let system = system_from_network_config(net_cfg.clone());
    let remote = system_from_network_config(net_cfg);

    let (ponger_named, ponf) =
        remote.create_and_register(|| BigPongerAct::new_eager(buf_cfg.clone()));
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let mut ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_named_path.via_udp();
    let (pinger_named, pinf) = system.create_and_register(move || {
        BigPingerAct::new_preserialised(ponger_named_path, 1500 * 3 / 4, buf_cfg)
    });

    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    remote.start(&ponger_named);
    system.start(&pinger_named);

    // TODO maybe we could do this a bit more reliable?
    thread::sleep(Duration::from_millis(15000));

    let pingfn = system.stop_notify(&pinger_named);
    let pongfn = remote.kill_notify(ponger_named);

    pingfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems with 2x Pingers and Pongers. One Ponger is registered by UUID,
// the other by a custom name. One Pinger communicates with the UUID-registered Ponger,
// the other with the named Ponger. Both sets are expected to exchange PING_COUNT ping-pong
// messages.
fn remote_delivery_to_registered_actors_eager_mixed_udp() {
    let system = system_from_network_config(NetworkConfig::default());
    let remote = system_from_network_config(NetworkConfig::default());

    let (ponger_unique, pouf) = remote.create_and_register(PongerAct::new_eager);
    let (ponger_named, ponf) = remote.create_and_register(PongerAct::new_eager);
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");

    let mut ponger_unique_path =
        pouf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_unique_path.via_udp();
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let mut ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_named_path.via_udp();

    let (pinger_unique, piuf) =
        system.create_and_register(move || PingerAct::new_eager(ponger_unique_path));
    let (pinger_named, pinf) =
        system.create_and_register(move || PingerAct::new_eager(ponger_named_path));

    piuf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    remote.start(&ponger_unique);
    remote.start(&ponger_named);
    system.start(&pinger_unique);
    system.start(&pinger_named);

    // TODO maybe we could do this a bit more reliable?
    thread::sleep(Duration::from_millis(7000));

    let pingfu = system.stop_notify(&pinger_unique);
    let pingfn = system.stop_notify(&pinger_named);
    let pongfu = remote.kill_notify(ponger_unique);
    let pongfn = remote.kill_notify(ponger_named);

    pingfu
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfu
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pingfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_unique.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems with 2x Pingers and Pongers. One Ponger is registered by UUID,
// the other by a custom name. One Pinger communicates with the UUID-registered Ponger,
// the other with the named Ponger. Both sets are expected to exchange PING_COUNT ping-pong
// messages.
fn remote_delivery_to_registered_actors_lazy() {
    let system = system_from_network_config(NetworkConfig::default());
    let remote = system_from_network_config(NetworkConfig::default());

    let (ponger_unique, pouf) = remote.create_and_register(PongerAct::new_lazy);
    let (ponger_named, ponf) = remote.create_and_register(PongerAct::new_lazy);
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");

    pouf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let named_path = ActorPath::Named(NamedPath::with_system(
        remote.system_path(),
        vec!["custom_name".into()],
    ));

    let unique_path = ActorPath::Unique(UniquePath::with_system(
        remote.system_path(),
        ponger_unique.id(),
    ));

    let (pinger_unique, piuf) =
        system.create_and_register(move || PingerAct::new_lazy(unique_path));
    let (pinger_named, pinf) = system.create_and_register(move || PingerAct::new_lazy(named_path));

    piuf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    remote.start(&ponger_unique);
    remote.start(&ponger_named);
    system.start(&pinger_unique);
    system.start(&pinger_named);

    // TODO maybe we could do this a bit more reliable?
    thread::sleep(Duration::from_millis(7000));

    let pingfu = system.stop_notify(&pinger_unique);
    let pingfn = system.stop_notify(&pinger_named);
    let pongfu = remote.kill_notify(ponger_unique);
    let pongfn = remote.kill_notify(ponger_named);

    pingfu
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfu
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pingfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_unique.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems with 2x Pingers and Pongers. One Ponger is registered by UUID,
// the other by a custom name. One Pinger communicates with the UUID-registered Ponger,
// the other with the named Ponger. Both sets are expected to exchange PING_COUNT ping-pong
// messages.
fn remote_delivery_to_registered_actors_lazy_mixed_udp() {
    let system = system_from_network_config(NetworkConfig::default());
    let remote = system_from_network_config(NetworkConfig::default());

    let (ponger_unique, pouf) = remote.create_and_register(PongerAct::new_lazy);
    let (ponger_named, ponf) = remote.create_and_register(PongerAct::new_lazy);
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");

    let mut ponger_unique_path =
        pouf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_unique_path.via_udp();
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let mut ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_named_path.via_udp();

    let (pinger_unique, piuf) =
        system.create_and_register(move || PingerAct::new_lazy(ponger_unique_path));
    let (pinger_named, pinf) =
        system.create_and_register(move || PingerAct::new_lazy(ponger_named_path));

    piuf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    remote.start(&ponger_unique);
    remote.start(&ponger_named);
    system.start(&pinger_unique);
    system.start(&pinger_named);

    // TODO maybe we could do this a bit more reliable?
    thread::sleep(Duration::from_millis(7000));

    let pingfu = system.stop_notify(&pinger_unique);
    let pingfn = system.stop_notify(&pinger_named);
    let pongfu = remote.kill_notify(ponger_unique);
    let pongfn = remote.kill_notify(ponger_named);

    pingfu
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfu
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pingfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_unique.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Sets up two KompactSystems 1 and 2a, with Named paths. It first spawns a pinger-ponger couple
// The ping pong process completes, it shuts down system2a, spawns a new pinger on system1
// and finally boots a new system 2b with an identical networkconfig and named actor registration
// The final ping pong round should then complete as system1 automatically reconnects to system2 and
// transfers the enqueued messages.
#[ignore]
fn remote_lost_and_continued_connection() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_max_connection_retry_attempts(6);
    net_cfg.set_connection_retry_interval(500);
    let system = system_from_network_config(net_cfg);
    let remote_a = system_from_network_config(NetworkConfig::default());
    let remote_port = remote_a.system_path().port();

    //let (ponger_unique, pouf) = remote.create_and_register(PongerAct::new);
    let (ponger_named, ponf) = remote_a.create_and_register(PongerAct::new_lazy);
    let poaf = remote_a.register_by_alias(&ponger_named, "custom_name");
    ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let named_path = ActorPath::Named(NamedPath::with_system(
        remote_a.system_path(),
        vec!["custom_name".into()],
    ));
    let named_path_clone = named_path.clone();

    let (pinger_named, pinf) =
        system.create_and_register(move || PingerAct::new_lazy(named_path_clone));
    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    remote_a.start(&ponger_named);
    system.start(&pinger_named);

    // Wait for the pingpong
    thread::sleep(Duration::from_millis(2000));

    // Assert that things are going as we expect
    let pongfn = remote_a.kill_notify(ponger_named);
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    // We now kill remote_a
    remote_a.kill_system().ok();

    thread::sleep(Duration::from_millis(1000));

    // Start a new pinger on system
    let (pinger_named2, pinf2) =
        system.create_and_register(move || PingerAct::new_lazy(named_path));
    pinf2.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    // The first start will succesfully buffer its outgoing message then trigger lost connection, losing the message
    system.start(&pinger_named2);
    thread::sleep(Duration::from_millis(1000));
    // The second start will be rejected due to lost connection, but retained until the connection is alive again.
    system.start(&pinger_named2);

    // Wait for it to send its pings, system should recognize the remote address
    thread::sleep(Duration::from_millis(1000));
    // Assert that things are going as they should be, ping count has not increased
    pinger_named2.on_definition(|c| {
        assert_eq!(c.count, 0);
    });

    // Start up remote_b
    let mut addr: SocketAddr = "127.0.0.1:0".parse().expect("Address should work");
    addr.set_port(remote_port);
    let remote_b = system_from_network_config(NetworkConfig::new(addr));

    let (ponger_named, ponf) = remote_b.create_and_register(PongerAct::new_lazy);
    let poaf = remote_b.register_by_alias(&ponger_named, "custom_name");
    ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    remote_b.start(&ponger_named);

    // We give the connection plenty of time to re-establish and transfer it's old queue
    thread::sleep(Duration::from_millis(5000));
    // Final assertion, did our systems re-connect without lost messages?
    pinger_named2.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });
    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    remote_b
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Identical with `remote_lost_and_continued_connection` up to the final sleep time and assertion
// system1 times out in its reconnection attempts and drops the enqueued buffers.
// After indirectly asserting that the queue was dropped we start up a new pinger, and assert that it succeeds.
#[ignore]
fn remote_lost_and_dropped_connection() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_max_connection_retry_attempts(2);
    net_cfg.set_connection_retry_interval(500);
    let system = system_from_network_config(net_cfg);
    let remote_a = system_from_network_config(NetworkConfig::default());
    let remote_port = remote_a.system_path().port();

    let (ponger_named, ponf) = remote_a.create_and_register(PongerAct::new_lazy);
    let poaf = remote_a.register_by_alias(&ponger_named, "custom_name");
    ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let named_path = ActorPath::Named(NamedPath::with_system(
        remote_a.system_path(),
        vec!["custom_name".into()],
    ));
    let named_path_clone = named_path.clone();
    let named_path_clone2 = named_path.clone();

    let (pinger_named, pinf) =
        system.create_and_register(move || PingerAct::new_lazy(named_path_clone));
    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    remote_a.start(&ponger_named);
    system.start(&pinger_named);

    // Wait for the pingpong
    thread::sleep(Duration::from_millis(2000));

    // Assert that things are going as we expect
    let pongfn = remote_a.kill_notify(ponger_named);
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });
    // We now kill system2
    remote_a.kill_system().ok();

    // Start a new pinger on system
    let (pinger_named2, pinf2) =
        system.create_and_register(move || PingerAct::new_lazy(named_path));
    pinf2.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    system.start(&pinger_named2);
    // Wait for it to send its pings, system should recognize the remote address
    thread::sleep(Duration::from_millis(1000));
    // Assert that things are going as they should be, ping count has not increased
    pinger_named2.on_definition(|c| {
        assert_eq!(c.count, 0);
    });
    // Sleep long-enough that the remote connection will be dropped with its queue
    thread::sleep(Duration::from_millis(
        3 * 500, // retry config from above
    ));
    // Start up remote_b
    let mut addr: SocketAddr = "127.0.0.1:0".parse().expect("Address should work");
    addr.set_port(remote_port);
    let remote_b = system_from_network_config(NetworkConfig::new(addr));

    let (ponger_named, ponf) = remote_b.create_and_register(PongerAct::new_lazy);
    let poaf = remote_b.register_by_alias(&ponger_named, "custom_name");
    ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    remote_b.start(&ponger_named);

    // We give the connection plenty of time to re-establish and transfer it's old queue
    thread::sleep(Duration::from_millis(5000));
    // Final assertion, did our systems re-connect without lost messages?
    pinger_named2.on_definition(|c| {
        assert_eq!(c.count, 0);
    });

    // This one should now succeed
    let (pinger_named2, pinf2) =
        system.create_and_register(move || PingerAct::new_lazy(named_path_clone2));
    pinf2.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    system.start(&pinger_named2);
    // Wait for it to send its pings, system should recognize the remote address
    thread::sleep(Duration::from_millis(1000));
    // Assert that things are going as they should be
    pinger_named2.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    remote_b
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn local_delivery() {
    let system = system_from_network_config(NetworkConfig::default());

    let (ponger, pof) = system.create_and_register(PongerAct::new_lazy);
    // Construct ActorPath with system's `proto` field explicitly set to LOCAL
    let mut ponger_path = system.actor_path_for(&ponger);
    ponger_path.set_protocol(Transport::Local);
    let (pinger, pif) = system.create_and_register(move || PingerAct::new_lazy(ponger_path));

    pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    pif.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    system.start(&ponger);
    system.start(&pinger);

    // TODO no sleeps!
    thread::sleep(Duration::from_millis(1000));

    let pingf = system.stop_notify(&pinger);
    let pongf = system.kill_notify(ponger);
    pingf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    pongf
        .wait_timeout(Duration::from_millis(1000))
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

    let (ponger, pof) = system.create_and_register(BigPongerAct::new_lazy);
    // Construct ActorPath with system's `proto` field explicitly set to LOCAL
    let mut ponger_path =
        pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_path.set_protocol(Transport::Local);

    let (forwarder, fof) = system.create_and_register(move || ForwarderAct::new(ponger_path));
    let mut forwarder_path =
        fof.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");
    forwarder_path.set_protocol(Transport::Local);

    let (pinger, pif) =
        system.create_and_register(move || BigPingerAct::new_lazy(forwarder_path, 512));
    let _pinger_path = pif.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    system.start(&ponger);
    system.start(&forwarder);
    system.start(&pinger);

    // TODO no sleeps!
    thread::sleep(Duration::from_millis(1000));

    let pingf = system.kill_notify(pinger.clone()); // hold on to this ref so we can check count later
    let forwf = system.kill_notify(forwarder);
    let pongf = system.kill_notify(ponger);
    pingf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    forwf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Forwarder never stopped!");
    pongf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
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

    let (ponger, pof) = system.create_and_register(BigPongerAct::new_lazy);
    // Construct ActorPath with system's `proto` field explicitly set to LOCAL
    let mut ponger_path =
        pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_path.set_protocol(Transport::Local);

    let (forwarder, fof) = system.create_and_register(move || ForwarderAct::new(ponger_path));
    let mut forwarder_path =
        fof.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");
    forwarder_path.set_protocol(Transport::Local);

    let (pinger, pif) = system.create_and_register(move || {
        BigPingerAct::new_eager(forwarder_path, 512, BufferConfig::default())
    });
    let _pinger_path = pif.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    system.start(&ponger);
    system.start(&forwarder);
    system.start(&pinger);

    // TODO no sleeps!
    thread::sleep(Duration::from_millis(1000));

    let pingf = system.kill_notify(pinger.clone()); // hold on to this ref so we can check count later
    let forwf = system.kill_notify(forwarder);
    let pongf = system.kill_notify(ponger);
    pingf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    forwf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Forwarder never stopped!");
    pongf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn remote_forwarding_unique() {
    let system1 = system_from_network_config(NetworkConfig::default());
    let system2 = system_from_network_config(NetworkConfig::default());
    let system3 = system_from_network_config(NetworkConfig::default());

    let (ponger, pof) = system1.create_and_register(BigPongerAct::new_lazy);
    let ponger_path = pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let (forwarder, fof) = system2.create_and_register(move || ForwarderAct::new(ponger_path));
    let forwarder_path =
        fof.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");

    let (pinger, pif) =
        system3.create_and_register(move || BigPingerAct::new_lazy(forwarder_path, 512));
    let _pinger_path = pif.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    system1.start(&ponger);
    system2.start(&forwarder);
    system3.start(&pinger);

    // TODO no sleeps!
    thread::sleep(Duration::from_millis(1000));

    let pingf = system3.kill_notify(pinger.clone()); // hold on to this ref so we can check count later
    let forwf = system2.kill_notify(forwarder);
    let pongf = system1.kill_notify(ponger);
    pingf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never died!");
    forwf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Forwarder never died!");
    pongf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system1
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system2
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system3
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn remote_forwarding_unique_two_systems() {
    let system1 = system_from_network_config(NetworkConfig::default());
    let system2 = system_from_network_config(NetworkConfig::default());

    let (ponger, pof) = system1.create_and_register(BigPongerAct::new_lazy);
    let ponger_path = pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let (forwarder, fof) = system2.create_and_register(move || ForwarderAct::new(ponger_path));
    let forwarder_path =
        fof.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");

    let (pinger, pif) =
        system1.create_and_register(move || BigPingerAct::new_lazy(forwarder_path, 512));
    let _pinger_path = pif.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    system1.start(&ponger);
    system2.start(&forwarder);
    system1.start(&pinger);

    // TODO no sleeps!
    thread::sleep(Duration::from_millis(1000));

    let pingf = system1.kill_notify(pinger.clone()); // hold on to this ref so we can check count later
    let forwf = system2.kill_notify(forwarder);
    let pongf = system1.kill_notify(ponger);
    pingf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never died!");
    forwf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Forwarder never died!");
    pongf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system1
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system2
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn remote_forwarding_named() {
    let system1 = system_from_network_config(NetworkConfig::default());
    let system2 = system_from_network_config(NetworkConfig::default());
    let system3 = system_from_network_config(NetworkConfig::default());

    let (ponger, _pof) = system1.create_and_register(BigPongerAct::new_lazy);
    let pnf = system1.register_by_alias(&ponger, "ponger");
    let ponger_path = pnf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let (forwarder, _fof) = system2.create_and_register(move || ForwarderAct::new(ponger_path));
    let fnf = system2.register_by_alias(&forwarder, "forwarder");
    let forwarder_path =
        fnf.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");

    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(256);
    let (pinger, pif) =
        system3.create_and_register(move || BigPingerAct::new_eager(forwarder_path, 512, buf_cfg));
    let _pinger_path = pif.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    system1.start(&ponger);
    system2.start(&forwarder);
    system3.start(&pinger);

    // TODO no sleeps!
    thread::sleep(Duration::from_millis(1000));

    let pingf = system3.kill_notify(pinger.clone()); // hold on to this ref so we can check count later
    let forwf = system2.kill_notify(forwarder);
    let pongf = system1.kill_notify(ponger);
    pingf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never died!");
    forwf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Forwarder never died!");
    pongf
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");

    pinger.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });

    system1
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system2
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system3
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn network_status_port_established_lost_dropped_connection() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_max_connection_retry_attempts(6);
    net_cfg.set_connection_retry_interval(1000);
    let local_system = system_from_network_config(net_cfg.clone());
    let remote_system = system_from_network_config(net_cfg);

    // Create a status_counter which will listen to the status port and count messages received
    let (status_counter, _scf) = local_system.create_and_register(NetworkStatusCounter::new);
    local_system.connect_network_status_port(&status_counter);
    local_system.start(&status_counter);

    // Create a pinger ponger pair such that the Network will be used.
    let (ponger, _pof) = local_system.create_and_register(PongerAct::new_lazy);
    let ponger_path = local_system
        .register_by_alias(&ponger, "ponger")
        .wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    local_system.start(&ponger);
    let (pinger, _pif) =
        remote_system.create_and_register(move || PingerAct::new_lazy(ponger_path));
    let pinger_path = remote_system
        .register_by_alias(&ponger, "ponger")
        .wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    remote_system.start(&pinger);
    // The systems establish a connection and the pings/pongs are sent
    thread::sleep(Duration::from_millis(5000));

    // Inspect live sockets
    // thread::sleep(Duration::from_millis(2 * 60 * 1000)); // "2MSL" to drop the socket

    // Shutdown the remote system and wait for the connection to be lost and dropped by local_system
    let _ = remote_system.kill_system();
    thread::sleep(Duration::from_millis(2500));

    let pinger_path_clone = pinger_path.clone();
    // Make sure the local_system discovers the lost connection
    let (failing_pinger, _pif) =
        local_system.create_and_register(move || PingerAct::new_lazy(pinger_path_clone));
    local_system.start(&failing_pinger);

    // Make sure the local_system discovers the lost connection on Linux....
    let (failing_pinger2, _pif) =
        local_system.create_and_register(move || PingerAct::new_lazy(pinger_path));
    local_system.start(&failing_pinger2);

    thread::sleep(Duration::from_millis(12000)); // let failure and drop happen
                                                 // Assert connection lost and dropped
    status_counter.on_definition(|sc| {
        assert_eq!(sc.connection_established, 1);
        assert_eq!(sc.connection_lost, 1);
        assert_eq!(sc.connection_dropped, 1);
    });
}

#[test]
fn network_status_port_close_connection_closed_connection() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_max_connection_retry_attempts(2);
    net_cfg.set_connection_retry_interval(1000);
    let local_system = system_from_network_config(net_cfg.clone());
    //std::proess::Command::new();
    let remote_system = system_from_network_config(net_cfg);

    // Create a status_counter which will listen to the status port and count messages received
    let (local_status_counter, _lscf) = local_system.create_and_register(NetworkStatusCounter::new);
    local_system.connect_network_status_port(&local_status_counter);
    local_system.start(&local_status_counter);

    let (remote_status_counter, _rscf) =
        remote_system.create_and_register(NetworkStatusCounter::new);
    remote_system.connect_network_status_port(&remote_status_counter);
    remote_system.start(&remote_status_counter);

    // Create a pinger ponger pair such that the Network will be used.
    let (ponger, _pof) = local_system.create_and_register(PongerAct::new_lazy);
    let pnf = local_system.register_by_alias(&ponger, "ponger");
    local_system.start(&ponger);
    let ponger_path = pnf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let local_system_path = ponger_path.system().clone();
    let (pinger, _pif) =
        remote_system.create_and_register(move || PingerAct::new_lazy(ponger_path));
    remote_system.start(&pinger);
    // The systems establish a connection and the pings/pongs are sent
    thread::sleep(Duration::from_millis(3000));

    remote_status_counter.on_definition(|sc| {
        sc.send_status_request(NetworkStatusRequest::DisconnectSystem(local_system_path));
    });

    // Wait for the channel to be closed
    thread::sleep(Duration::from_millis(5000));
    local_status_counter.on_definition(|sc| {
        assert_eq!(sc.connection_closed, 1);
    });

    remote_status_counter.on_definition(|sc| {
        assert_eq!(sc.connection_closed, 1);
    });
}

/*
#[test]
fn network_status_port_connected_and_disconnected_requests() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_max_connection_retry_attempts(2);
    net_cfg.set_connection_retry_interval(1000);
    let ponger_system = system_from_network_config(net_cfg.clone());
    let connection_system = system_from_network_config(net_cfg.clone());
    let disconnection_system = system_from_network_config(net_cfg);

    // Create a status_counter which will listen to the status port and count messages received
    let (local_status_counter, _lscf) =
        ponger_system.create_and_register(NetworkStatusCounter::new);
    ponger_system.connect_network_status_port(&local_status_counter);
    ponger_system.start(&local_status_counter);

    let (connection_status_counter, _rscf) =
        connection_system.create_and_register(NetworkStatusCounter::new);
    connection_system.connect_network_status_port(&connection_status_counter);
    connection_system.start(&connection_status_counter);

    let (disconnection_status_counter, _rscf) =
        disconnection_system.create_and_register(NetworkStatusCounter::new);
    disconnection_system.connect_network_status_port(&disconnection_status_counter);
    disconnection_system.start(&disconnection_status_counter);

    // Create a ponger and two pingers pair such that the Network will be used.
    let (ponger, _pof) = ponger_system.create_and_register(PongerAct::new_lazy);
    let ponger_path = ponger_system
        .register_by_alias(&ponger, "ponger")
        .wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_system.start(&ponger);
    let ponger_path_clone = ponger_path.clone();
    let ponger_path_clone2 = ponger_path.clone();
    let ponger_system_path = ponger_path.system().clone();

    let (pinger, _pif) =
        connection_system.create_and_register(move || PingerAct::new_lazy(ponger_path_clone));
    let pinger_path = connection_system
        .register_by_alias(&pinger, "pinger")
        .wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    connection_system.start(&pinger);
    let connection_system_path = pinger_path.system().clone();

    let (pinger2, _pif) =
        disconnection_system.create_and_register(move || PingerAct::new_lazy(ponger_path_clone2));
    let pinger2_path = disconnection_system
        .register_by_alias(&pinger2, "pinger")
        .wait_expect(Duration::from_millis(1000), "Pinger2 failed to register!");
    disconnection_system.start(&pinger2);
    let disconnection_system_path = pinger2_path.system().clone();

    // The systems establish a connection and the pings/pongs are sent, then close one channel
    thread::sleep(Duration::from_millis(3000));
    local_status_counter.on_definition(|sc| {
        sc.send_status_request(NetworkStatusRequest::DisconnectSystem(
            disconnection_system_path.clone(),
        ));
    });
    thread::sleep(Duration::from_millis(3000));

    // Send the status requests
    connection_status_counter.on_definition(|sc| {
        sc.send_status_request(NetworkStatusRequest::DisconnectedSystems);
        sc.send_status_request(NetworkStatusRequest::ConnectedSystems);
    });
    disconnection_status_counter.on_definition(|sc| {
        sc.send_status_request(NetworkStatusRequest::ConnectedSystems);
        sc.send_status_request(NetworkStatusRequest::DisconnectedSystems);
    });
    local_status_counter.on_definition(|sc| {
        sc.send_status_request(NetworkStatusRequest::DisconnectedSystems);
        sc.send_status_request(NetworkStatusRequest::ConnectedSystems);
    });

    // Wait for the messages then assert
    thread::sleep(Duration::from_millis(3000));
    local_status_counter.on_definition(|sc| {
        assert_eq!(sc.connected_systems[0], connection_system_path);
        assert_eq!(sc.disconnected_systems[0], disconnection_system_path);
    });
    disconnection_status_counter.on_definition(|sc| {
        assert_eq!(sc.disconnected_systems[0], ponger_system_path);
        assert!(sc.connected_systems.is_empty());
    });
    connection_status_counter.on_definition(|sc| {
        assert_eq!(sc.connected_systems[0], ponger_system_path);
        assert!(sc.disconnected_systems.is_empty());
    });
}
 */

#[test]
fn network_status_port_open_close_open() {
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_max_connection_retry_attempts(2);
    net_cfg.set_connection_retry_interval(1000);
    let local_system = system_from_network_config(net_cfg.clone());
    let remote_system = system_from_network_config(net_cfg);

    let system_path = remote_system.system_path();
    // Create a status_counter which will listen to the status port and count messages received
    let (local_status_counter, _lscf) = local_system.create_and_register(NetworkStatusCounter::new);
    local_system.connect_network_status_port(&local_status_counter);
    local_system.start(&local_status_counter);

    local_status_counter.on_definition(|sc| {
        sc.send_status_request(NetworkStatusRequest::ConnectSystem(system_path.clone()));
    });
    thread::sleep(Duration::from_millis(3000));
    local_status_counter.on_definition(|sc| {
        assert_eq!(sc.connection_established, 1);
        assert_eq!(sc.connection_closed, 0);
        sc.send_status_request(NetworkStatusRequest::DisconnectSystem(system_path.clone()));
    });
    thread::sleep(Duration::from_millis(3000));
    local_status_counter.on_definition(|sc| {
        assert_eq!(sc.connection_established, 1);
        assert_eq!(sc.connection_closed, 1);
        sc.send_status_request(NetworkStatusRequest::ConnectSystem(system_path.clone()));
    });
    thread::sleep(Duration::from_millis(3000));
    local_status_counter.on_definition(|sc| {
        assert_eq!(sc.connection_established, 2);
        assert_eq!(sc.connection_closed, 1);
    });
    let _ = local_system.shutdown();
    let _ = remote_system.shutdown();
}

#[test]
// Sets up three KompactSystems: One with a BigPonger, one with a BigPinger with big pings
// and one with a BigPinger with small pings. The big Pings are sent first and occupies
// all buffers of the BigPonger system. The small pings are then sent but can not be received
// until the Ponger system closes the Big Ping-channel due to too many retries.
// A new batch up small-pings are then sent and replied to.
fn remote_delivery_overflow_network_thread_buffers() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(1280);
    buf_cfg.max_chunk_count(10);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg.clone());
    // We will attempt to establish a connection for 5 seconds before giving up.
    // This config is also used when giving up on running out of buffers.
    // The big_pinger_system will occupy all buffers on the ponger_system for 5 seconds
    // And then it will be freed.
    net_cfg.set_connection_retry_interval(1000);
    net_cfg.set_max_connection_retry_attempts(10);
    let big_pinger_system = system_from_network_config(net_cfg.clone());
    let ponger_system = system_from_network_config(net_cfg.clone());
    let small_pinger_system = system_from_network_config(net_cfg);

    // Create the BigPonger on the Ponger system
    let (ponger_named, ponf) =
        ponger_system.create_and_register(|| BigPongerAct::new_eager(buf_cfg.clone()));
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let ponger_named_path = ponger_system
        .register_by_alias(&ponger_named, "custom_name")
        .wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let ponger_named_path1 = ponger_named_path.clone();
    let ponger_named_path2 = ponger_named_path.clone();

    // Create the three pingers
    let (big_pinger_named, pinf) = big_pinger_system.create_and_register(move || {
        BigPingerAct::new_preserialised(ponger_named_path, 15000, BufferConfig::default())
    });
    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    let (small_pinger1_named, pinf) = small_pinger_system.create_and_register(move || {
        BigPingerAct::new_preserialised(ponger_named_path1, 10, BufferConfig::default())
    });
    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    let (small_pinger2_named, pinf) = small_pinger_system.create_and_register(move || {
        BigPingerAct::new_preserialised(ponger_named_path2, 10, BufferConfig::default())
    });
    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    // Ponger_system will be blocked blocked for about 10 Seconds from this point.
    ponger_system.start(&ponger_named);
    big_pinger_system.start(&big_pinger_named);

    // Wait for the buffers to run out
    thread::sleep(Duration::from_millis(4000));

    // remote system should be unable to receive any messages as the BigPing is occupying all buffers
    small_pinger_system.start(&small_pinger1_named);
    thread::sleep(Duration::from_millis(4000));
    // Assert that it failed to ping-pong.
    small_pinger1_named.on_definition(|c| {
        assert_eq!(c.count, 0);
    });

    // Shutdown big_pinger_system to make sure it won't continue blocking.
    // Assert that the big_pinger never got anything as a sanity check.
    big_pinger_system
        .stop_notify(&big_pinger_named)
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    big_pinger_named.on_definition(|c| {
        assert_eq!(c.count, 0);
    });
    big_pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");

    thread::sleep(Duration::from_millis(4000));

    // Start the second Pinger.
    small_pinger_system
        .start_notify(&small_pinger2_named)
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");
    // Wait a long time to make sure that all time-outs occur and the sends are succesfull.
    thread::sleep(Duration::from_millis(12000));
    small_pinger_system
        .stop_notify(&small_pinger1_named)
        .wait_timeout(Duration::from_millis(1000))
        .expect("Pinger never stopped!");

    // Shut down the ponger
    ponger_system
        .kill_notify(ponger_named)
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");

    small_pinger2_named.on_definition(|c| {
        assert_eq!(c.count, PING_COUNT);
    });
    ponger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
    small_pinger_system
        .shutdown()
        .expect("Kompact didn't shut down properly");
}
