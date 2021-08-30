pub mod groups;

#[cfg(test)]
pub(crate) mod test_helpers {
    use crate::prelude::*;

    pub use std::{sync::Arc, time::Duration};

    pub fn individual_count(receivers: &[Arc<Component<ReceiverComponent>>]) -> Vec<usize> {
        receivers
            .iter()
            .map(|component| component.on_definition(|cd| cd.count))
            .collect()
    }

    pub fn total_count(receivers: &[Arc<Component<ReceiverComponent>>]) -> usize {
        individual_count(receivers).iter().sum()
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CountMe;
    impl Serialisable for CountMe {
        fn ser_id(&self) -> SerId {
            Self::SER_ID
        }

        fn size_hint(&self) -> Option<usize> {
            Some(0)
        }

        fn serialise(&self, _buf: &mut dyn BufMut) -> Result<(), SerError> {
            Ok(())
        }

        fn local(self: Box<Self>) -> Result<Box<dyn Any + Send>, Box<dyn Serialisable>> {
            Ok(self)
        }

        fn cloned(&self) -> Option<Box<dyn Serialisable>> {
            Some(Box::new(*self))
        }
    }
    impl Deserialiser<CountMe> for CountMe {
        const SER_ID: SerId = 42;

        fn deserialise(_buf: &mut dyn Buf) -> Result<CountMe, SerError> {
            Ok(CountMe)
        }
    }

    #[derive(ComponentDefinition)]
    pub struct ReceiverComponent {
        ctx: ComponentContext<Self>,
        count: usize,
    }
    impl Default for ReceiverComponent {
        fn default() -> Self {
            ReceiverComponent {
                ctx: ComponentContext::uninitialised(),
                count: 0,
            }
        }
    }
    ignore_lifecycle!(ReceiverComponent);
    impl Actor for ReceiverComponent {
        type Message = Never;

        fn receive_local(&mut self, _msg: Self::Message) -> Handled {
            unreachable!("Can't instantiate Never type!");
        }

        fn receive_network(&mut self, msg: NetMessage) -> Handled {
            match_deser! {
                msg {
                    msg(_count_me): CountMe => self.count += 1,
                    err(e) => error!(self.log(), "Received something else: {:?}", e),
                }
            };
            Handled::Ok
        }
    }

    pub fn new_kompact_system() -> KompactSystem {
        let mut cfg = KompactConfig::default();
        cfg.system_components(DeadletterBox::new, {
            let net_config =
                NetworkConfig::new("127.0.0.1:0".parse().expect("Address should work"));
            net_config.build()
        });
        cfg.build().expect("Kompact System")
    }
}

#[cfg(test)]
mod tests {
    use super::{groups::*, test_helpers::*};
    use crate::prelude::*;

    const GROUP_SIZE: usize = 3;
    const NUM_MESSAGES: usize = 30;
    const SLEEP_TIME: Duration = Duration::from_millis(3000);

    #[test]
    fn test_explicit_round_robin_select() {
        let system = new_kompact_system();

        let receivers: Vec<Arc<Component<ReceiverComponent>>> = (0..GROUP_SIZE)
            .map(|_i| system.create(ReceiverComponent::default))
            .collect();
        let group_ref = system
            .set_routing_policy(RoundRobinRouting::default(), "routing-group", false)
            .wait_expect(SLEEP_TIME, "Could not register policy");
        let receiver_refs: Vec<ActorPath> = receivers
            .iter()
            .enumerate()
            .map(|(i, c)| system.register_by_alias(c, &format!("routing-group/receiver{}", i)))
            .map(|f| f.wait_expect(SLEEP_TIME, "Could not register component"))
            .collect();
        receivers.iter().for_each(|c| system.start(c));

        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(1, total_count(&receivers));

        group_ref.tell(CountMe, &system);
        let alternative_group_ref: ActorPath = (group_ref.clone().unwrap_named() / '?').into();
        alternative_group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(3, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert!(counts.iter().all(|&v| v == 1));

        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(4, total_count(&receivers));
        let counts = individual_count(&receivers);
        // can't really guarantee that the indices match up here
        assert!(counts.iter().any(|&v| v == 2));
        assert_eq!(2, counts.iter().filter(|&&v| v == 1).count());

        for _i in 0..NUM_MESSAGES {
            group_ref.tell(CountMe, &system);
        }
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(NUM_MESSAGES + 4, total_count(&receivers));

        drop(receiver_refs);
        system.shutdown().expect("shutdown");
    }

    #[test]
    fn test_explicit_hash_select() {
        let system = new_kompact_system();

        let receivers: Vec<Arc<Component<ReceiverComponent>>> = (0..GROUP_SIZE)
            .map(|_i| system.create(ReceiverComponent::default))
            .collect();
        let group_ref = system
            .set_routing_policy(
                SenderDefaultHashBucketRouting::default(),
                "routing-group",
                false,
            )
            .wait_expect(SLEEP_TIME, "Could not register policy");
        let receiver_refs: Vec<ActorPath> = receivers
            .iter()
            .enumerate()
            .map(|(i, c)| system.register_by_alias(c, &format!("routing-group/receiver{}", i)))
            .map(|f| f.wait_expect(SLEEP_TIME, "Could not register component"))
            .collect();
        receivers.iter().for_each(|c| system.start(c));

        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(1, total_count(&receivers));

        group_ref.tell(CountMe, &system);
        let alternative_group_ref: ActorPath = (group_ref.clone().unwrap_named() / '?').into();
        alternative_group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(3, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert!(counts.iter().any(|&v| v == 3));

        let other_source_ref: ActorPath = system
            .system_path()
            .into_named_with_string("other_source")
            .expect("actor path")
            .into();

        group_ref.tell(CountMe, &other_source_ref.using_dispatcher(&system));
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(4, total_count(&receivers));
        let counts = individual_count(&receivers);
        // they could end up in the same bucket
        assert!(
            (counts.iter().any(|&v| v == 3) && counts.iter().any(|&v| v == 1))
                || counts.iter().any(|&v| v == 4)
        );

        for _i in 0..NUM_MESSAGES {
            group_ref.tell(CountMe, &other_source_ref.using_dispatcher(&system));
        }
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(NUM_MESSAGES + 4, total_count(&receivers));
        let counts = individual_count(&receivers);
        // they could technically end up in the same bucket
        assert!(
            (counts.iter().any(|&v| v == 3) && counts.iter().any(|&v| v == NUM_MESSAGES + 1))
                || counts.iter().any(|&v| v == NUM_MESSAGES + 4)
        );

        drop(receiver_refs);
        system.shutdown().expect("shutdown");
    }

    #[cfg(feature = "implicit_routes")]
    #[test]
    fn test_implicit_select() {
        let system = new_kompact_system();

        let receivers: Vec<Arc<Component<ReceiverComponent>>> = (0..GROUP_SIZE)
            .map(|_i| system.create(ReceiverComponent::default))
            .collect();
        let group_ref: ActorPath = system
            .system_path()
            .into_named_with_string("routing-group/?")
            .expect("actor path")
            .into();
        let receiver_refs: Vec<ActorPath> = receivers
            .iter()
            .enumerate()
            .map(|(i, c)| system.register_by_alias(c, &format!("routing-group/receiver{}", i)))
            .map(|f| f.wait_expect(SLEEP_TIME, "Could not register component"))
            .collect();
        receivers.iter().for_each(|c| system.start(c));

        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(1, total_count(&receivers));

        group_ref.tell(CountMe, &system);
        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(3, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert!(counts.iter().any(|&v| v == 3));

        let other_source_ref: ActorPath = system
            .system_path()
            .into_named_with_string("other_source")
            .expect("actor path")
            .into();

        group_ref.tell(CountMe, &other_source_ref.using_dispatcher(&system));
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(4, total_count(&receivers));
        let counts = individual_count(&receivers);
        // they could  end up in the same bucket
        assert!(
            (counts.iter().any(|&v| v == 3) && counts.iter().any(|&v| v == 1))
                || counts.iter().any(|&v| v == 4)
        );

        for _i in 0..NUM_MESSAGES {
            group_ref.tell(CountMe, &other_source_ref.using_dispatcher(&system));
        }
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(NUM_MESSAGES + 4, total_count(&receivers));
        let counts = individual_count(&receivers);
        // they could technically end up in the same bucket
        assert!(
            (counts.iter().any(|&v| v == 3) && counts.iter().any(|&v| v == NUM_MESSAGES + 1))
                || counts.iter().any(|&v| v == NUM_MESSAGES + 4)
        );

        drop(receiver_refs);
        system.shutdown().expect("shutdown");
    }

    #[test]
    fn test_explicit_broadcast() {
        let system = new_kompact_system();

        let receivers: Vec<Arc<Component<ReceiverComponent>>> = (0..GROUP_SIZE)
            .map(|_i| system.create(ReceiverComponent::default))
            .collect();
        let group_ref = system
            .set_routing_policy(BroadcastRouting::default(), "routing-group", false)
            .wait_expect(SLEEP_TIME, "Could not register policy");
        let receiver_refs: Vec<ActorPath> = receivers
            .iter()
            .enumerate()
            .map(|(i, c)| system.register_by_alias(c, &format!("routing-group/receiver{}", i)))
            .map(|f| f.wait_expect(SLEEP_TIME, "Could not register component"))
            .collect();
        receivers.iter().for_each(|c| system.start(c));

        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(3, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(1, counts[0]);
        assert_eq!(1, counts[1]);
        assert_eq!(1, counts[2]);

        group_ref.tell(CountMe, &system);
        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(9, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(3, counts[0]);
        assert_eq!(3, counts[1]);
        assert_eq!(3, counts[2]);

        let alternative_group_ref: ActorPath = (group_ref.clone().unwrap_named() / '*').into();
        alternative_group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(12, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(4, counts[0]);
        assert_eq!(4, counts[1]);
        assert_eq!(4, counts[2]);

        for _i in 0..NUM_MESSAGES {
            group_ref.tell(CountMe, &system);
        }
        std::thread::sleep(SLEEP_TIME);
        assert_eq!((NUM_MESSAGES + 4) * GROUP_SIZE, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(NUM_MESSAGES + 4, counts[0]);
        assert_eq!(NUM_MESSAGES + 4, counts[1]);
        assert_eq!(NUM_MESSAGES + 4, counts[2]);

        drop(receiver_refs);
        system.shutdown().expect("shutdown");
    }

    #[cfg(feature = "implicit_routes")]
    #[test]
    fn test_implicit_broadcast() {
        let system = new_kompact_system();

        let receivers: Vec<Arc<Component<ReceiverComponent>>> = (0..GROUP_SIZE)
            .map(|_i| system.create(ReceiverComponent::default))
            .collect();
        let group_ref: ActorPath = system
            .system_path()
            .into_named_with_string("routing-group/*")
            .expect("actor path")
            .into();
        let receiver_refs: Vec<ActorPath> = receivers
            .iter()
            .enumerate()
            .map(|(i, c)| system.register_by_alias(c, &format!("routing-group/receiver{}", i)))
            .map(|f| f.wait_expect(SLEEP_TIME, "Could not register component"))
            .collect();
        receivers.iter().for_each(|c| system.start(c));

        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(3, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(1, counts[0]);
        assert_eq!(1, counts[1]);
        assert_eq!(1, counts[2]);

        group_ref.tell(CountMe, &system);
        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(9, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(3, counts[0]);
        assert_eq!(3, counts[1]);
        assert_eq!(3, counts[2]);

        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(12, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(4, counts[0]);
        assert_eq!(4, counts[1]);
        assert_eq!(4, counts[2]);

        for _i in 0..NUM_MESSAGES {
            group_ref.tell(CountMe, &system);
        }
        std::thread::sleep(SLEEP_TIME);
        assert_eq!((NUM_MESSAGES + 4) * GROUP_SIZE, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(NUM_MESSAGES + 4, counts[0]);
        assert_eq!(NUM_MESSAGES + 4, counts[1]);
        assert_eq!(NUM_MESSAGES + 4, counts[2]);

        drop(receiver_refs);
        system.shutdown().expect("shutdown");
    }

    #[cfg(feature = "implicit_routes")]
    #[test]
    fn test_nested_select() {
        let system = new_kompact_system();

        let receivers: Vec<Arc<Component<ReceiverComponent>>> = (0..3)
            .map(|_i| system.create(ReceiverComponent::default))
            .collect();
        let group_ref: ActorPath = system
            .system_path()
            .into_named_with_string("routing-group/?")
            .expect("actor path")
            .into();
        let receiver_reg_fs: Vec<KFuture<RegistrationResult>> = vec![
            system.register_by_alias(&receivers[0], "routing-group/receiver1"),
            system.register_by_alias(&receivers[1], "routing-group/receiver2"),
            system.register_by_alias(&receivers[2], "routing-group/receivers/node1"),
        ];
        let receiver_refs: Vec<ActorPath> = receiver_reg_fs
            .into_iter()
            .map(|f| f.wait_expect(SLEEP_TIME, "Could not register component"))
            .collect();
        receivers.iter().for_each(|c| system.start(c));

        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(1, total_count(&receivers));

        group_ref.tell(CountMe, &system);
        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(3, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert!(counts.iter().any(|&v| v == 3));

        let other_source_ref: ActorPath = system
            .system_path()
            .into_named_with_string("other_source")
            .expect("actor path")
            .into();

        group_ref.tell(CountMe, &other_source_ref.using_dispatcher(&system));
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(4, total_count(&receivers));
        let counts = individual_count(&receivers);
        // they could  end up in the same bucket
        assert!(
            (counts.iter().any(|&v| v == 3) && counts.iter().any(|&v| v == 1))
                || counts.iter().any(|&v| v == 4)
        );

        for _i in 0..NUM_MESSAGES {
            group_ref.tell(CountMe, &other_source_ref.using_dispatcher(&system));
        }
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(NUM_MESSAGES + 4, total_count(&receivers));
        let counts = individual_count(&receivers);
        // they could technically end up in the same bucket
        assert!(
            (counts.iter().any(|&v| v == 3) && counts.iter().any(|&v| v == NUM_MESSAGES + 1))
                || counts.iter().any(|&v| v == NUM_MESSAGES + 4)
        );

        drop(receiver_refs);
        system.shutdown().expect("shutdown");
    }

    #[cfg(feature = "implicit_routes")]
    #[test]
    fn test_nested_broadcast() {
        let system = new_kompact_system();

        let receivers: Vec<Arc<Component<ReceiverComponent>>> = (0..3)
            .map(|_i| system.create(ReceiverComponent::default))
            .collect();
        let group_ref: ActorPath = system
            .system_path()
            .into_named_with_string("routing-group/*")
            .expect("actor path")
            .into();
        let receiver_reg_fs: Vec<KFuture<RegistrationResult>> = vec![
            system.register_by_alias(&receivers[0], "routing-group/receiver1"),
            system.register_by_alias(&receivers[1], "routing-group/receiver2"),
            system.register_by_alias(&receivers[2], "routing-group/receivers/node1"),
        ];
        let receiver_refs: Vec<ActorPath> = receiver_reg_fs
            .into_iter()
            .map(|f| f.wait_expect(SLEEP_TIME, "Could not register component"))
            .collect();
        receivers.iter().for_each(|c| system.start(c));

        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(3, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(1, counts[0]);
        assert_eq!(1, counts[1]);
        assert_eq!(1, counts[2]);

        group_ref.tell(CountMe, &system);
        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(9, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(3, counts[0]);
        assert_eq!(3, counts[1]);
        assert_eq!(3, counts[2]);

        group_ref.tell(CountMe, &system);
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(12, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(4, counts[0]);
        assert_eq!(4, counts[1]);
        assert_eq!(4, counts[2]);

        for _i in 0..NUM_MESSAGES {
            group_ref.tell(CountMe, &system);
        }
        std::thread::sleep(SLEEP_TIME);
        assert_eq!((NUM_MESSAGES + 4) * GROUP_SIZE, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(NUM_MESSAGES + 4, counts[0]);
        assert_eq!(NUM_MESSAGES + 4, counts[1]);
        assert_eq!(NUM_MESSAGES + 4, counts[2]);

        drop(receiver_refs);
        system.shutdown().expect("shutdown");
    }
}
