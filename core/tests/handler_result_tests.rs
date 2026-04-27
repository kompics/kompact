use kompact::prelude::*;
use std::{
    error::Error,
    fmt,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FaultKind {
    Benign,
    Recoverable,
    Unrecoverable,
}

impl FaultKind {
    fn all() -> [FaultKind; 3] {
        [
            FaultKind::Benign,
            FaultKind::Recoverable,
            FaultKind::Unrecoverable,
        ]
    }

    fn into_result(self, source: HandlerSource) -> HandlerResult {
        let fault = TestFault { kind: self, source };
        match self {
            FaultKind::Benign => Err(HandlerError::benign(fault)),
            FaultKind::Recoverable => Err(HandlerError::recoverable(fault)),
            FaultKind::Unrecoverable => Err(HandlerError::unrecoverable(fault)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandlerSource {
    Normal,
    Async,
    Blocking,
    Lifecycle,
}

#[derive(Debug)]
struct TestFault {
    kind: FaultKind,
    source: HandlerSource,
}

impl fmt::Display for TestFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} fault from {:?} handler", self.kind, self.source)
    }
}

impl Error for TestFault {}

#[derive(Debug, PartialEq, Eq)]
enum Notice {
    Ran,
    Recovered,
    Ping,
}

#[derive(Debug)]
enum TestMessage {
    Trigger,
    Ping,
}

#[derive(ComponentDefinition)]
struct HandlerResultProbe {
    ctx: ComponentContext<Self>,
    source: HandlerSource,
    kind: FaultKind,
    notices: Sender<Notice>,
}

impl HandlerResultProbe {
    fn new(source: HandlerSource, kind: FaultKind, notices: Sender<Notice>) -> Self {
        HandlerResultProbe {
            ctx: ComponentContext::uninitialised(),
            source,
            kind,
            notices,
        }
    }

    fn report_ran(&self) {
        self.notices.send(Notice::Ran).expect("test receiver");
    }

    fn fail(&self, source: HandlerSource) -> HandlerResult {
        self.kind.into_result(source)
    }
}

impl ComponentLifecycle for HandlerResultProbe {
    fn on_start(&mut self) -> HandlerResult {
        if self.source == HandlerSource::Lifecycle {
            self.report_ran();
            self.fail(HandlerSource::Lifecycle)
        } else {
            Handled::OK
        }
    }
}

impl Actor for HandlerResultProbe {
    type Message = TestMessage;

    fn receive_local(&mut self, msg: Self::Message) -> HandlerResult {
        match msg {
            TestMessage::Trigger => match self.source {
                HandlerSource::Normal => {
                    self.report_ran();
                    self.fail(HandlerSource::Normal)
                }
                HandlerSource::Async => {
                    let kind = self.kind;
                    self.spawn_local(move |async_self| async move {
                        async_self.report_ran();
                        kind.into_result(HandlerSource::Async)
                    });
                    Handled::OK
                }
                HandlerSource::Blocking => {
                    let kind = self.kind;
                    Handled::block_on(self, move |async_self| async move {
                        async_self.report_ran();
                        kind.into_result(HandlerSource::Blocking)
                    })
                }
                HandlerSource::Lifecycle => Handled::OK,
            },
            TestMessage::Ping => {
                self.notices.send(Notice::Ping).expect("test receiver");
                Handled::OK
            }
        }
    }

    #[cfg(feature = "distributed")]
    fn receive_network(&mut self, _msg: NetMessage) -> HandlerResult {
        unimplemented!("No networking in handler result tests");
    }
}

fn make_system() -> KompactSystem {
    KompactConfig::default().build().expect("system")
}

fn make_probe(
    system: &KompactSystem,
    source: HandlerSource,
    kind: FaultKind,
) -> (Arc<Component<HandlerResultProbe>>, Receiver<Notice>) {
    let (tx, rx) = mpsc::channel();
    let component = system.create(move || HandlerResultProbe::new(source, kind, tx));
    (component, rx)
}

fn install_recovery(component: &Arc<Component<HandlerResultProbe>>) {
    let tx = component.on_definition(|probe| probe.notices.clone());
    component.set_recovery_function(move |ctx| {
        ctx.recover_with(move |ctx, _, _| {
            assert!(
                matches!(ctx.fault, Fault::Handler(_)),
                "recoverable handler errors must be reported as handler faults"
            );
            tx.send(Notice::Recovered).expect("test receiver");
        })
    });
}

fn expect_notice(rx: &Receiver<Notice>, notice: Notice) {
    assert_eq!(
        rx.recv_timeout(TIMEOUT)
            .expect("timed out waiting for notice"),
        notice
    );
}

fn wait_until(name: &str, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {name}");
}

fn run_case(source: HandlerSource, kind: FaultKind) {
    let system = make_system();
    let (component, rx) = make_probe(&system, source, kind);

    if kind == FaultKind::Recoverable {
        install_recovery(&component);
    }

    if source == HandlerSource::Lifecycle && kind != FaultKind::Benign {
        system.start(&component);
    } else {
        system
            .start_notify(&component)
            .wait_timeout(TIMEOUT)
            .expect("component did not start");
    }

    if source != HandlerSource::Lifecycle {
        component.actor_ref().tell(TestMessage::Trigger);
    }

    match kind {
        FaultKind::Benign => {
            expect_notice(&rx, Notice::Ran);
            component.actor_ref().tell(TestMessage::Ping);
            expect_notice(&rx, Notice::Ping);
            assert!(component.is_active());
        }
        FaultKind::Recoverable => {
            expect_notice(&rx, Notice::Ran);
            expect_notice(&rx, Notice::Recovered);
            assert!(component.is_faulty());
        }
        FaultKind::Unrecoverable => {
            wait_until("component destruction", || component.is_destroyed());
        }
    }

    system.shutdown().expect("shutdown");
}

#[test]
fn normal_handlers_apply_all_error_classifications() {
    for kind in FaultKind::all() {
        run_case(HandlerSource::Normal, kind);
    }
}

#[test]
fn async_futures_apply_all_error_classifications() {
    for kind in FaultKind::all() {
        run_case(HandlerSource::Async, kind);
    }
}

#[test]
fn blocking_futures_apply_all_error_classifications() {
    for kind in FaultKind::all() {
        run_case(HandlerSource::Blocking, kind);
    }
}

#[test]
fn lifecycle_handlers_apply_all_error_classifications() {
    for kind in FaultKind::all() {
        run_case(HandlerSource::Lifecycle, kind);
    }
}
