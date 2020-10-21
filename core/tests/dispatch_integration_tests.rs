use kompact::{prelude::*, prelude_test::net_test_helpers::*};
use std::{net::SocketAddr, thread, time::Duration};

#[test]
fn named_registration() {
    const ACTOR_NAME: &str = "ponger";

    let mut cfg = KompactConfig::new();
    cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
    let system = cfg.build().expect("KompactSystem");
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
    let (system, remote) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system())
    };
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
        println!("pinger uniq count: {}", c.count);
        assert_eq!(c.count, PING_COUNT);
    });
    pinger_named.on_definition(|c| {
        println!("pinger named count: {}", c.count);
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
    let (system, remote) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, net_cfg.clone().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system())
    };
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
    let (system, remote) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, net_cfg.clone().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system())
    };
    let (ponger_named, ponf) =
        remote.create_and_register(|| BigPongerAct::new_eager(buf_cfg.clone()));
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let (pinger_named, pinf) = system.create_and_register(move || {
        BigPingerAct::new_eager(ponger_named_path, 120, buf_cfg.clone())
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
fn remote_delivery_bigger_than_buffer_messages_preserialised_tcp() {
    let mut buf_cfg = BufferConfig::default();
    buf_cfg.chunk_size(128);
    let mut net_cfg = NetworkConfig::default();
    net_cfg.set_buffer_config(buf_cfg.clone());
    let (system, remote) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, net_cfg.clone().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system())
    };
    let (ponger_named, ponf) =
        remote.create_and_register(|| BigPongerAct::new_eager(buf_cfg.clone()));
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let (pinger_named, pinf) = system.create_and_register(move || {
        BigPingerAct::new_preserialised(ponger_named_path, 120, buf_cfg.clone())
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
    net_cfg.set_buffer_config(buf_cfg.clone());
    let (system, remote) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, net_cfg.clone().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system())
    };
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
    let (system, remote) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, net_cfg.clone().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system())
    };
    let (ponger_named, ponf) =
        remote.create_and_register(|| BigPongerAct::new_eager(buf_cfg.clone()));
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let mut ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_named_path.via_udp();
    let (pinger_named, pinf) = system.create_and_register(move || {
        BigPingerAct::new_eager(ponger_named_path, 1500 * 3 / 4, buf_cfg.clone())
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
    let (system, remote) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, net_cfg.clone().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system())
    };
    let (ponger_named, ponf) =
        remote.create_and_register(|| BigPongerAct::new_eager(buf_cfg.clone()));
    let poaf = remote.register_by_alias(&ponger_named, "custom_name");
    let _ = ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let mut ponger_named_path =
        poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_named_path.via_udp();
    let (pinger_named, pinf) = system.create_and_register(move || {
        BigPingerAct::new_preserialised(ponger_named_path, 1500 * 3 / 4, buf_cfg.clone())
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
    let (system, remote) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system())
    };
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
        println!("pinger uniq count: {}", c.count);
        assert_eq!(c.count, PING_COUNT);
    });
    pinger_named.on_definition(|c| {
        println!("pinger named count: {}", c.count);
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
    let (system, remote) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system())
    };
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
        println!("pinger uniq count: {}", c.count);
        assert_eq!(c.count, PING_COUNT);
    });
    pinger_named.on_definition(|c| {
        println!("pinger named count: {}", c.count);
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
    let (system, remote) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system())
    };
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
        println!("pinger uniq count: {}", c.count);
        assert_eq!(c.count, PING_COUNT);
    });
    pinger_named.on_definition(|c| {
        println!("pinger named count: {}", c.count);
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
    let system1 = || {
        let mut cfg = KompactConfig::new();
        cfg.system_components(
            DeadletterBox::new,
            NetworkConfig::new(SocketAddr::new("127.0.0.1".parse().unwrap(), 9111)).build(),
        );
        cfg.build().expect("KompactSystem")
    };
    let system2 = || {
        let mut cfg = KompactConfig::new();
        cfg.system_components(
            DeadletterBox::new,
            NetworkConfig::new(SocketAddr::new("127.0.0.1".parse().unwrap(), 9112)).build(),
        );
        cfg.build().expect("KompactSystem")
    };

    // Set-up system2a
    let system2a = system2();
    //let (ponger_unique, pouf) = remote.create_and_register(PongerAct::new);
    let (ponger_named, ponf) = system2a.create_and_register(PongerAct::new_lazy);
    let poaf = system2a.register_by_alias(&ponger_named, "custom_name");
    ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let named_path = ActorPath::Named(NamedPath::with_system(
        system2a.system_path(),
        vec!["custom_name".into()],
    ));
    let named_path_clone = named_path.clone();
    // Set-up system1
    let system1 = system1();
    let (pinger_named, pinf) =
        system1.create_and_register(move || PingerAct::new_lazy(named_path_clone));
    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    system2a.start(&ponger_named);
    system1.start(&pinger_named);

    // Wait for the pingpong
    thread::sleep(Duration::from_millis(2000));

    // Assert that things are going as we expect
    let pongfn = system2a.kill_notify(ponger_named);
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_named.on_definition(|c| {
        println!("pinger named count: {}", c.count);
        assert_eq!(c.count, PING_COUNT);
    });
    // We now kill system2
    system2a.shutdown().ok();
    // Start a new pinger on system1
    let (pinger_named2, pinf2) =
        system1.create_and_register(move || PingerAct::new_lazy(named_path));
    pinf2.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    system1.start(&pinger_named2);
    // Wait for it to send its pings, system1 should recognize the remote address
    thread::sleep(Duration::from_millis(1000));
    // Assert that things are going as they should be, ping count has not increased
    pinger_named2.on_definition(|c| {
        println!("pinger named count: {}", c.count);
        assert_eq!(c.count, 0);
    });
    // Start up system2b
    println!("Setting up system2b");
    let system2b = system2();
    let (ponger_named, ponf) = system2b.create_and_register(PongerAct::new_lazy);
    let poaf = system2b.register_by_alias(&ponger_named, "custom_name");
    ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    println!("Starting actor on system2b");
    system2b.start(&ponger_named);

    // We give the connection plenty of time to re-establish and transfer it's old queue
    println!("Final sleep");
    thread::sleep(Duration::from_millis(5000));
    // Final assertion, did our systems re-connect without lost messages?
    println!("Asserting");
    pinger_named2.on_definition(|c| {
        println!("pinger named count: {}", c.count);
        assert_eq!(c.count, PING_COUNT);
    });
    println!("Shutting down");
    system1
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system2b
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
// Identical with `remote_lost_and_continued_connection` up to the final sleep time and assertion
// system1 times out in its reconnection attempts and drops the enqueued buffers.
// After indirectly asserting that the queue was dropped we start up a new pinger, and assert that it succeeds.
#[ignore]
fn remote_lost_and_dropped_connection() {
    let mut network_cfg_1 = NetworkConfig::new(SocketAddr::new("127.0.0.1".parse().unwrap(), 9211));
    let max_reattempts = 3;
    network_cfg_1.set_max_connection_retry_attempts(max_reattempts);
    let reattempt_interval = network_cfg_1.get_connection_retry_interval();
    let system1 = || {
        let mut cfg = KompactConfig::new();
        cfg.system_components(DeadletterBox::new, network_cfg_1.build());
        cfg.build().expect("KompactSystem")
    };
    let system2 = || {
        let mut cfg = KompactConfig::new();
        cfg.system_components(
            DeadletterBox::new,
            NetworkConfig::new(SocketAddr::new("127.0.0.1".parse().unwrap(), 9212)).build(),
        );
        cfg.build().expect("KompactSystem")
    };

    // Set-up system2a
    let system2a = system2();
    //let (ponger_unique, pouf) = remote.create_and_register(PongerAct::new);
    let (ponger_named, ponf) = system2a.create_and_register(PongerAct::new_lazy);
    let poaf = system2a.register_by_alias(&ponger_named, "custom_name");
    ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    let named_path = ActorPath::Named(NamedPath::with_system(
        system2a.system_path(),
        vec!["custom_name".into()],
    ));
    let named_path_clone = named_path.clone();
    let named_path_clone2 = named_path.clone();
    // Set-up system1
    let system1 = system1();
    let (pinger_named, pinf) =
        system1.create_and_register(move || PingerAct::new_lazy(named_path_clone));
    pinf.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");

    system2a.start(&ponger_named);
    system1.start(&pinger_named);

    // Wait for the pingpong
    thread::sleep(Duration::from_millis(2000));

    // Assert that things are going as we expect
    let pongfn = system2a.kill_notify(ponger_named);
    pongfn
        .wait_timeout(Duration::from_millis(1000))
        .expect("Ponger never died!");
    pinger_named.on_definition(|c| {
        println!("pinger named count: {}", c.count);
        assert_eq!(c.count, PING_COUNT);
    });
    // We now kill system2
    system2a.shutdown().ok();
    // Start a new pinger on system1
    let (pinger_named2, pinf2) =
        system1.create_and_register(move || PingerAct::new_lazy(named_path));
    pinf2.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    system1.start(&pinger_named2);
    // Wait for it to send its pings, system1 should recognize the remote address
    thread::sleep(Duration::from_millis(1000));
    // Assert that things are going as they should be, ping count has not increased
    pinger_named2.on_definition(|c| {
        println!("pinger named count: {}", c.count);
        assert_eq!(c.count, 0);
    });
    // Sleep long-enough that the remote connection will be dropped with its queue
    thread::sleep(Duration::from_millis(
        (max_reattempts + 2) as u64 * reattempt_interval,
    ));
    // Start up system2b
    println!("Setting up system2b");
    let system2b = system2();
    let (ponger_named, ponf) = system2b.create_and_register(PongerAct::new_lazy);
    let poaf = system2b.register_by_alias(&ponger_named, "custom_name");
    ponf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    poaf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    println!("Starting actor on system2b");
    system2b.start(&ponger_named);

    // We give the connection plenty of time to re-establish and transfer it's old queue
    thread::sleep(Duration::from_millis(5000));
    // Final assertion, did our systems re-connect without lost messages?
    pinger_named2.on_definition(|c| {
        println!("pinger named count: {}", c.count);
        assert_eq!(c.count, 0);
    });

    // This one should now succeed
    let (pinger_named2, pinf2) =
        system1.create_and_register(move || PingerAct::new_lazy(named_path_clone2));
    pinf2.wait_expect(Duration::from_millis(1000), "Pinger failed to register!");
    system1.start(&pinger_named2);
    // Wait for it to send its pings, system1 should recognize the remote address
    thread::sleep(Duration::from_millis(1000));
    // Assert that things are going as they should be
    pinger_named2.on_definition(|c| {
        println!("pinger named count: {}", c.count);
        assert_eq!(c.count, PING_COUNT);
    });

    system1
        .shutdown()
        .expect("Kompact didn't shut down properly");
    system2b
        .shutdown()
        .expect("Kompact didn't shut down properly");
}

#[test]
fn local_delivery() {
    let mut cfg = KompactConfig::new();
    cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
    let system = cfg.build().expect("KompactSystem");

    let (ponger, pof) = system.create_and_register(PongerAct::new_lazy);
    // Construct ActorPath with system's `proto` field explicitly set to LOCAL
    let mut ponger_path = system.actor_path_for(&ponger);
    ponger_path.set_protocol(Transport::LOCAL);
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
    let mut cfg = KompactConfig::new();
    cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
    let system = cfg.build().expect("KompactSystem");

    let (ponger, pof) = system.create_and_register(PongerAct::new_lazy);
    // Construct ActorPath with system's `proto` field explicitly set to LOCAL
    let mut ponger_path =
        pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_path.set_protocol(Transport::LOCAL);

    let (forwarder, fof) = system.create_and_register(move || ForwarderAct::new(ponger_path));
    let mut forwarder_path =
        fof.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");
    forwarder_path.set_protocol(Transport::LOCAL);

    let (pinger, pif) = system.create_and_register(move || PingerAct::new_lazy(forwarder_path));
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
    let mut cfg = KompactConfig::new();
    cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
    let system = cfg.build().expect("KompactSystem");

    let (ponger, pof) = system.create_and_register(PongerAct::new_lazy);
    // Construct ActorPath with system's `proto` field explicitly set to LOCAL
    let mut ponger_path =
        pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");
    ponger_path.set_protocol(Transport::LOCAL);

    let (forwarder, fof) = system.create_and_register(move || ForwarderAct::new(ponger_path));
    let mut forwarder_path =
        fof.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");
    forwarder_path.set_protocol(Transport::LOCAL);

    let (pinger, pif) = system.create_and_register(move || PingerAct::new_eager(forwarder_path));
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
    let (system1, system2, system3) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system(), system())
    };

    let (ponger, pof) = system1.create_and_register(PongerAct::new_lazy);
    let ponger_path = pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let (forwarder, fof) = system2.create_and_register(move || ForwarderAct::new(ponger_path));
    let forwarder_path =
        fof.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");

    let (pinger, pif) = system3.create_and_register(move || PingerAct::new_lazy(forwarder_path));
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
    let (system1, system2) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system())
    };

    let (ponger, pof) = system1.create_and_register(PongerAct::new_lazy);
    let ponger_path = pof.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let (forwarder, fof) = system2.create_and_register(move || ForwarderAct::new(ponger_path));
    let forwarder_path =
        fof.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");

    let (pinger, pif) = system1.create_and_register(move || PingerAct::new_lazy(forwarder_path));
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
    let (system1, system2, system3) = {
        let system = || {
            let mut cfg = KompactConfig::new();
            cfg.system_components(DeadletterBox::new, NetworkConfig::default().build());
            cfg.build().expect("KompactSystem")
        };
        (system(), system(), system())
    };

    let (ponger, _pof) = system1.create_and_register(PongerAct::new_lazy);
    let pnf = system1.register_by_alias(&ponger, "ponger");
    let ponger_path = pnf.wait_expect(Duration::from_millis(1000), "Ponger failed to register!");

    let (forwarder, _fof) = system2.create_and_register(move || ForwarderAct::new(ponger_path));
    let fnf = system2.register_by_alias(&forwarder, "forwarder");
    let forwarder_path =
        fnf.wait_expect(Duration::from_millis(1000), "Forwarder failed to register!");

    let (pinger, pif) = system3.create_and_register(move || PingerAct::new_lazy(forwarder_path));
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
