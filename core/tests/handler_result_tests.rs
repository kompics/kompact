mod common;

use common::{build_test_system, eventually};
use kompact::prelude::*;
use std::{
    error::Error,
    fmt,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::Duration,
};

const TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FaultKind {
    Benign,
    Recoverable,
    Unrecoverable,
}

impl FaultKind {
    const ALL: [FaultKind; 3] = [
        FaultKind::Benign,
        FaultKind::Recoverable,
        FaultKind::Unrecoverable,
    ];

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
    Lifecycle(LifecycleHook),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LifecycleHook {
    Start,
    Stop,
    Kill,
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
enum ProbeNotice {
    HandlerExecuted,
    RecoveryHandlerExecuted,
    PingHandled,
}

#[derive(Debug)]
enum ProbeMessage {
    TriggerSelectedHandler,
    Ping,
}

#[derive(ComponentDefinition)]
struct HandlerResultProbeComponent {
    ctx: ComponentContext<Self>,
    source: HandlerSource,
    kind: FaultKind,
    notices: Sender<ProbeNotice>,
}

impl HandlerResultProbeComponent {
    fn new(source: HandlerSource, kind: FaultKind, notices: Sender<ProbeNotice>) -> Self {
        HandlerResultProbeComponent {
            ctx: ComponentContext::uninitialised(),
            source,
            kind,
            notices,
        }
    }

    fn report_ran(&self) {
        self.notices
            .send(ProbeNotice::HandlerExecuted)
            .expect("test receiver");
    }

    fn fail(&self, source: HandlerSource) -> HandlerResult {
        self.kind.into_result(source)
    }

    fn check_fail_lifecycle(&self, hook: LifecycleHook) -> HandlerResult {
        let source = HandlerSource::Lifecycle(hook);
        if self.source == source {
            self.report_ran();
            self.fail(source)
        } else {
            Handled::OK
        }
    }
}

impl ComponentLifecycle for HandlerResultProbeComponent {
    fn on_start(&mut self) -> HandlerResult {
        self.check_fail_lifecycle(LifecycleHook::Start)
    }

    fn on_stop(&mut self) -> HandlerResult {
        self.check_fail_lifecycle(LifecycleHook::Stop)
    }

    fn on_kill(&mut self) -> HandlerResult {
        self.check_fail_lifecycle(LifecycleHook::Kill)
    }
}

impl Actor for HandlerResultProbeComponent {
    type Message = ProbeMessage;

    fn receive_local(&mut self, msg: Self::Message) -> HandlerResult {
        match msg {
            ProbeMessage::TriggerSelectedHandler => match self.source {
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
                HandlerSource::Lifecycle(_) => Handled::OK,
            },
            ProbeMessage::Ping => {
                self.notices
                    .send(ProbeNotice::PingHandled)
                    .expect("test receiver");
                Handled::OK
            }
        }
    }

    #[cfg(feature = "distributed")]
    fn receive_network(&mut self, _msg: NetMessage) -> HandlerResult {
        unimplemented!("No networking in handler result tests");
    }
}

fn create_probe(
    system: &KompactSystem,
    source: HandlerSource,
    kind: FaultKind,
) -> (
    Arc<Component<HandlerResultProbeComponent>>,
    Receiver<ProbeNotice>,
) {
    let (tx, rx) = mpsc::channel();
    let component = system.create(move || HandlerResultProbeComponent::new(source, kind, tx));
    (component, rx)
}

fn install_recovery(component: &Arc<Component<HandlerResultProbeComponent>>) {
    let tx = component.on_definition(|probe| probe.notices.clone());
    component.set_recovery_function(move |ctx| {
        ctx.recover_with(move |ctx, _, _| {
            assert!(
                matches!(ctx.fault, Fault::Handler(_)),
                "recoverable handler errors must be reported as handler faults"
            );
            tx.send(ProbeNotice::RecoveryHandlerExecuted)
                .expect("test receiver");
        })
    });
}

fn expect_notice(rx: &Receiver<ProbeNotice>, notice: ProbeNotice) {
    assert_eq!(
        rx.recv_timeout(TIMEOUT)
            .expect("timed out waiting for notice"),
        notice
    );
}

fn start(component: &Arc<Component<HandlerResultProbeComponent>>, system: &KompactSystem) {
    system
        .start_notify(component)
        .wait_timeout(TIMEOUT)
        .expect("component did not start");
}

fn trigger_source(
    system: &KompactSystem,
    component: &Arc<Component<HandlerResultProbeComponent>>,
    source: HandlerSource,
    kind: FaultKind,
) {
    match source {
        HandlerSource::Normal | HandlerSource::Async | HandlerSource::Blocking => {
            start(component, system);
            component
                .actor_ref()
                .tell(ProbeMessage::TriggerSelectedHandler);
        }
        HandlerSource::Lifecycle(LifecycleHook::Start) if kind == FaultKind::Benign => {
            start(component, system);
        }
        HandlerSource::Lifecycle(LifecycleHook::Start) => system.start(component),
        HandlerSource::Lifecycle(LifecycleHook::Stop) => {
            start(component, system);
            system.stop(component);
        }
        HandlerSource::Lifecycle(LifecycleHook::Kill) => {
            start(component, system);
            system.kill(component.clone());
        }
    }
}

fn expect_healthy(
    component: &Arc<Component<HandlerResultProbeComponent>>,
    rx: &Receiver<ProbeNotice>,
) {
    component.actor_ref().tell(ProbeMessage::Ping);
    expect_notice(rx, ProbeNotice::PingHandled);
    assert!(component.is_active());
}

fn assert_outcome(
    system: &KompactSystem,
    component: &Arc<Component<HandlerResultProbeComponent>>,
    rx: &Receiver<ProbeNotice>,
    source: HandlerSource,
    kind: FaultKind,
) {
    expect_notice(rx, ProbeNotice::HandlerExecuted);

    match kind {
        FaultKind::Benign => match source {
            HandlerSource::Lifecycle(LifecycleHook::Stop) => {
                eventually("component stop", TIMEOUT, || {
                    !component.is_active() && !component.is_faulty() && !component.is_destroyed()
                });
                start(component, system);
                expect_healthy(component, rx);
            }
            HandlerSource::Lifecycle(LifecycleHook::Kill) => {
                eventually("component destruction", TIMEOUT, || {
                    component.is_destroyed()
                });
            }
            _ => expect_healthy(component, rx),
        },
        FaultKind::Recoverable => {
            expect_notice(rx, ProbeNotice::RecoveryHandlerExecuted);
            eventually("component fault", TIMEOUT, || component.is_faulty());
        }
        FaultKind::Unrecoverable => {
            eventually("component destruction", TIMEOUT, || {
                component.is_destroyed()
            });
        }
    }
}

fn run_case(source: HandlerSource, kind: FaultKind) {
    let system = build_test_system();
    let (component, rx) = create_probe(&system, source, kind);

    if kind == FaultKind::Recoverable {
        install_recovery(&component);
    }

    trigger_source(&system, &component, source, kind);
    assert_outcome(&system, &component, &rx, source, kind);

    system.shutdown().wait().expect("shutdown");
}

#[test]
fn normal_handlers_apply_all_error_classifications() {
    for kind in FaultKind::ALL {
        run_case(HandlerSource::Normal, kind);
    }
}

#[test]
fn async_futures_apply_all_error_classifications() {
    for kind in FaultKind::ALL {
        run_case(HandlerSource::Async, kind);
    }
}

#[test]
fn blocking_futures_apply_all_error_classifications() {
    for kind in FaultKind::ALL {
        run_case(HandlerSource::Blocking, kind);
    }
}

#[test]
fn lifecycle_handlers_apply_all_error_classifications() {
    for hook in [
        LifecycleHook::Start,
        LifecycleHook::Stop,
        LifecycleHook::Kill,
    ] {
        for kind in FaultKind::ALL {
            run_case(HandlerSource::Lifecycle(hook), kind);
        }
    }
}

fn fail_with(kind: FaultKind) -> Result<(), TestFault> {
    Err(TestFault {
        kind,
        source: HandlerSource::Normal,
    })
}

#[test]
fn handler_result_extension_preserves_ok_values() {
    assert_eq!(Ok::<_, TestFault>(7).benign_err().expect("ok"), 7);
    assert_eq!(Ok::<_, TestFault>(8).recoverable_err().expect("ok"), 8);
    assert_eq!(Ok::<_, TestFault>(9).unrecoverable_err().expect("ok"), 9);
}

#[test]
fn handler_result_extension_classifies_errors() {
    assert!(matches!(
        fail_with(FaultKind::Benign).benign_err(),
        Err(HandlerError::Benign(_))
    ));
    assert!(matches!(
        fail_with(FaultKind::Recoverable).recoverable_err(),
        Err(HandlerError::Recoverable(_))
    ));
    assert!(matches!(
        fail_with(FaultKind::Unrecoverable).unrecoverable_err(),
        Err(HandlerError::Unrecoverable(_))
    ));
}
