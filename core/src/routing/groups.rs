//! Support for routing groups and policies

use crate::{
    actors::{ActorPath, DynActorRef},
    messaging::NetMessage,
    utils::IterExtras,
    KompactLogger,
};
#[allow(unused_imports)]
use slog::{crit, debug, error, info, trace, warn};
use std::{
    fmt,
    hash::{BuildHasher, BuildHasherDefault, Hash, Hasher},
};

/// A routing group keeps group membership together with a routing policy
#[derive(Debug)]
pub struct RoutingGroup {
    members: Vec<DynActorRef>,
    policy: Box<dyn RoutingPolicy<DynActorRef, NetMessage>>,
}
impl RoutingGroup {
    /// Create a new routing group
    pub fn new(
        initial_members: Vec<DynActorRef>,
        policy: Box<dyn RoutingPolicy<DynActorRef, NetMessage>>,
    ) -> Self {
        RoutingGroup {
            members: initial_members,
            policy,
        }
    }

    /// Create an empty group from a concrete routing `policy`
    pub fn empty<P>(policy: P) -> Self
    where
        P: RoutingPolicy<DynActorRef, NetMessage> + 'static,
    {
        let boxed = Box::new(policy);
        RoutingGroup {
            members: Vec::new(),
            policy: boxed,
        }
    }

    /// Route `msg` to our members as instructed by our policy
    pub fn route(&mut self, msg: NetMessage, logger: &KompactLogger) {
        let members: &[DynActorRef] = &self.members;
        self.policy.route(members, msg, logger);
    }
}

/// A routing policy described how to route a given `msg` over
/// a given set of `members`
pub trait RoutingPolicy<Ref, M>: fmt::Debug {
    /// Route the `msg` to the appropriate `members`
    ///
    /// A logger should be provided to allow debugging and handle potential routing errors.
    fn route(&mut self, members: &[Ref], msg: M, logger: &KompactLogger);
}

/// Round-robin dispatch policy
///
/// This policy can handle changing membership,
/// but won't start a new round immediately on change.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RoundRobinRouting {
    offset: usize,
}
impl RoundRobinRouting {
    /// Create a new instance starting at index 0
    pub fn new() -> Self {
        RoundRobinRouting { offset: 0 }
    }

    /// Get and return the current index and increment it for the next invocation
    pub fn get_and_increment_index(&mut self, length: usize) -> usize {
        let index = if self.offset >= length {
            0
        } else {
            self.offset
        };
        self.offset = index.wrapping_add(1);
        index
    }
}
impl Default for RoundRobinRouting {
    fn default() -> Self {
        Self::new()
    }
}
impl RoutingPolicy<DynActorRef, NetMessage> for RoundRobinRouting {
    fn route(&mut self, members: &[DynActorRef], msg: NetMessage, logger: &KompactLogger) {
        let index = self.get_and_increment_index(members.len());
        trace!(logger, "Routing msg to member at index={}", index);
        members[index].tell(msg);
    }
}

/// A router using consistent hashing on some field of the message
///
/// Changing the membership is supported,
/// but will result in bucket reassignments.
#[derive(Clone, Copy)]
pub struct FieldHashBucketRouting<M, T: Hash, H: BuildHasher> {
    hasher_builder: H,
    field_extractor: fn(&M) -> &T,
}
impl<M, T: Hash, H: BuildHasher> fmt::Debug for FieldHashBucketRouting<M, T, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "FieldHashBucketRouting {{ hasher_builder: {}, field_extractor: {} }}",
            std::any::type_name::<H>(),
            std::any::type_name::<fn(&M) -> &T>()
        )
    }
}
impl<M, T: Hash, H: BuildHasher> FieldHashBucketRouting<M, T, H> {
    /// Create a new instance of this router using the provided hasher and field extractor function
    pub fn new(hasher_builder: H, field_extractor: fn(&M) -> &T) -> Self {
        FieldHashBucketRouting {
            hasher_builder,
            field_extractor,
        }
    }

    /// Hashes the `field` using this router's `Hasher`
    pub fn hash_field(&self, field: &T) -> u64 {
        let mut hasher = self.hasher_builder.build_hasher();
        field.hash(&mut hasher);
        hasher.finish()
    }

    /// Extracts and hashes the `msg` field using this router's `Hasher`
    pub fn hash_message(&self, msg: &M) -> u64 {
        let field_ref = (self.field_extractor)(msg);
        self.hash_field(field_ref)
    }

    /// Provides the bucket index for the `msg` for `length` buckets
    ///
    /// It does so by calling [hash_message](hash_message) internally
    /// and then using modulo arithmentic to decide a bucket from the hash.
    pub fn get_bucket(&self, msg: &M, length: usize) -> usize {
        let hash = self.hash_message(msg);
        let index = hash % (length as u64);
        index as usize
    }
}

/// A hash bucket router using the `sender` field of a `NetMessage` to decide the bucket
pub type SenderHashBucketRouting<H> = FieldHashBucketRouting<NetMessage, ActorPath, H>;
/// A hash bucket router using the `sender` field of a `NetMessage` to decide the bucket
///
/// Uses Rust's default hasher.
pub type SenderDefaultHashBucketRouting =
    SenderHashBucketRouting<BuildHasherDefault<std::collections::hash_map::DefaultHasher>>;

impl<H: BuildHasher + Default> Default for SenderHashBucketRouting<H> {
    fn default() -> Self {
        let hasher_builder = H::default();
        Self::new(hasher_builder, NetMessage::sender)
    }
}

impl<H: BuildHasher> RoutingPolicy<DynActorRef, NetMessage> for SenderHashBucketRouting<H> {
    fn route(&mut self, members: &[DynActorRef], msg: NetMessage, logger: &KompactLogger) {
        let index = self.get_bucket(&msg, members.len());
        trace!(
            logger,
            "Routing msg with sender={} to member at index={}",
            msg.sender,
            index
        );
        members[index].tell(msg);
    }
}

/// A router that simply hands a copy of the message to every member
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BroadcastRouting;
impl Default for BroadcastRouting {
    fn default() -> Self {
        BroadcastRouting
    }
}
impl RoutingPolicy<DynActorRef, NetMessage> for BroadcastRouting {
    fn route(&mut self, members: &[DynActorRef], msg: NetMessage, logger: &KompactLogger) {
        trace!(logger, "Trying to broadcast message: {:?}", msg);
        let res = members
            .iter()
            .for_each_try_with(msg, |member, my_msg| member.tell(my_msg));
        match res {
            Ok(_) => trace!(logger, "The message was broadcast."),
            Err(e) => error!(logger, "Could not broadcast a message! Error was: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use std::{sync::Arc, time::Duration};

    const GROUP_SIZE: usize = 3;
    const NUM_MESSAGES: usize = 30;
    const SLEEP_TIME: Duration = Duration::from_millis(100);

    #[test]
    fn router_debug() {
        {
            let router = RoundRobinRouting::default();
            println!("Router: {:?}", router);
            let group = RoutingGroup::empty(router);
            println!("Group: {:?}", group);
        }
        {
            let router = SenderDefaultHashBucketRouting::default();
            println!("Router: {:?}", router);
            let group = RoutingGroup::empty(router);
            println!("Group: {:?}", group);
        }
        {
            let router = BroadcastRouting::default();
            println!("Router: {:?}", router);
            let group = RoutingGroup::empty(router);
            println!("Group: {:?}", group);
        }
    }

    fn individual_count(receivers: &[Arc<Component<ReceiverComponent>>]) -> Vec<usize> {
        receivers
            .iter()
            .map(|component| component.on_definition(|cd| cd.count))
            .collect()
    }

    fn total_count(receivers: &[Arc<Component<ReceiverComponent>>]) -> usize {
        individual_count(receivers).iter().sum()
    }

    #[test]
    fn round_robin_routing() {
        let system = KompactConfig::default().build().expect("system");

        let receivers: Vec<Arc<Component<ReceiverComponent>>> = (0..GROUP_SIZE)
            .map(|_i| system.create(ReceiverComponent::default))
            .collect();
        let receiver_refs: Vec<DynActorRef> =
            receivers.iter().map(|c| c.actor_ref().dyn_ref()).collect();
        receivers.iter().for_each(|c| system.start(c));

        let mut group = RoutingGroup::new(receiver_refs, Box::new(RoundRobinRouting::default()));
        let group_ref: ActorPath =
            NamedPath::with_system(system.system_path(), vec!["routing_group".to_string()]).into();
        let source_ref = system.deadletter_path();

        let msg = NetMessage::with_box(
            CountMe::SER_ID,
            source_ref.clone(),
            group_ref.clone(),
            Box::new(CountMe),
        );
        group.route(msg, system.logger());
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(1, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(1, counts[0]);
        assert_eq!(0, counts[1]);
        assert_eq!(0, counts[2]);

        let msg2 = NetMessage::with_box(
            CountMe::SER_ID,
            source_ref.clone(),
            group_ref.clone(),
            Box::new(CountMe),
        );
        group.route(msg2, system.logger());
        let msg3 = NetMessage::with_box(
            CountMe::SER_ID,
            source_ref.clone(),
            group_ref.clone(),
            Box::new(CountMe),
        );
        group.route(msg3, system.logger());
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(3, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(1, counts[0]);
        assert_eq!(1, counts[1]);
        assert_eq!(1, counts[2]);

        let msg4 = NetMessage::with_box(
            CountMe::SER_ID,
            source_ref.clone(),
            group_ref.clone(),
            Box::new(CountMe),
        );
        group.route(msg4, system.logger());
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(4, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(2, counts[0]);
        assert_eq!(1, counts[1]);
        assert_eq!(1, counts[2]);

        for _i in 0..NUM_MESSAGES {
            let msg = NetMessage::with_box(
                CountMe::SER_ID,
                source_ref.clone(),
                group_ref.clone(),
                Box::new(CountMe),
            );
            group.route(msg, system.logger());
        }
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(NUM_MESSAGES + 4, total_count(&receivers));

        system.shutdown().expect("shutdown");
    }

    #[test]
    fn sender_hash_routing() {
        let system = KompactConfig::default().build().expect("system");

        let receivers: Vec<Arc<Component<ReceiverComponent>>> = (0..GROUP_SIZE)
            .map(|_i| system.create(ReceiverComponent::default))
            .collect();
        let receiver_refs: Vec<DynActorRef> =
            receivers.iter().map(|c| c.actor_ref().dyn_ref()).collect();
        receivers.iter().for_each(|c| system.start(c));

        let mut group = RoutingGroup::new(
            receiver_refs,
            Box::new(SenderDefaultHashBucketRouting::default()),
        );
        let group_ref: ActorPath =
            NamedPath::with_system(system.system_path(), vec!["routing_group".to_string()]).into();
        let source_ref = system.deadletter_path();

        let msg = NetMessage::with_box(
            CountMe::SER_ID,
            source_ref.clone(),
            group_ref.clone(),
            Box::new(CountMe),
        );
        group.route(msg, system.logger());
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(1, total_count(&receivers));

        let msg2 = NetMessage::with_box(
            CountMe::SER_ID,
            source_ref.clone(),
            group_ref.clone(),
            Box::new(CountMe),
        );
        group.route(msg2, system.logger());
        let msg3 = NetMessage::with_box(
            CountMe::SER_ID,
            source_ref,
            group_ref.clone(),
            Box::new(CountMe),
        );
        group.route(msg3, system.logger());
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(3, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert!(counts.iter().any(|&v| v == 3));

        let other_source_ref: ActorPath =
            NamedPath::with_system(system.system_path(), vec!["other_source".to_string()]).into();

        let msg4 = NetMessage::with_box(
            CountMe::SER_ID,
            other_source_ref.clone(),
            group_ref.clone(),
            Box::new(CountMe),
        );
        group.route(msg4, system.logger());
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(4, total_count(&receivers));
        let counts = individual_count(&receivers);
        // they could technically end up in the same bucket,
        // just try to pick a name above where they don't
        assert!(counts.iter().any(|&v| v == 3));
        assert!(counts.iter().any(|&v| v == 1));

        for _i in 0..NUM_MESSAGES {
            let msg = NetMessage::with_box(
                CountMe::SER_ID,
                other_source_ref.clone(),
                group_ref.clone(),
                Box::new(CountMe),
            );
            group.route(msg, system.logger());
        }
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(NUM_MESSAGES + 4, total_count(&receivers));
        let counts = individual_count(&receivers);
        // they could technically end up in the same bucket,
        // just try to pick a name above where they don't
        assert!(counts.iter().any(|&v| v == 3));
        assert!(counts.iter().any(|&v| v == NUM_MESSAGES + 1));

        system.shutdown().expect("shutdown");
    }

    #[test]
    fn broadcast_routing() {
        let system = KompactConfig::default().build().expect("system");

        let receivers: Vec<Arc<Component<ReceiverComponent>>> = (0..GROUP_SIZE)
            .map(|_i| system.create(ReceiverComponent::default))
            .collect();
        let receiver_refs: Vec<DynActorRef> =
            receivers.iter().map(|c| c.actor_ref().dyn_ref()).collect();
        receivers.iter().for_each(|c| system.start(c));

        let mut group = RoutingGroup::new(receiver_refs, Box::new(BroadcastRouting::default()));
        let group_ref: ActorPath =
            NamedPath::with_system(system.system_path(), vec!["routing_group".to_string()]).into();
        let source_ref = system.deadletter_path();

        let msg = NetMessage::with_box(
            CountMe::SER_ID,
            source_ref.clone(),
            group_ref.clone(),
            Box::new(CountMe),
        );
        group.route(msg, system.logger());
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(3, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(1, counts[0]);
        assert_eq!(1, counts[1]);
        assert_eq!(1, counts[2]);

        let msg2 = NetMessage::with_box(
            CountMe::SER_ID,
            source_ref.clone(),
            group_ref.clone(),
            Box::new(CountMe),
        );
        group.route(msg2, system.logger());
        let msg3 = NetMessage::with_box(
            CountMe::SER_ID,
            source_ref.clone(),
            group_ref.clone(),
            Box::new(CountMe),
        );
        group.route(msg3, system.logger());
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(9, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(3, counts[0]);
        assert_eq!(3, counts[1]);
        assert_eq!(3, counts[2]);

        let msg4 = NetMessage::with_box(
            CountMe::SER_ID,
            source_ref.clone(),
            group_ref.clone(),
            Box::new(CountMe),
        );
        group.route(msg4, system.logger());
        std::thread::sleep(SLEEP_TIME);
        assert_eq!(12, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(4, counts[0]);
        assert_eq!(4, counts[1]);
        assert_eq!(4, counts[2]);

        for _i in 0..NUM_MESSAGES {
            let msg = NetMessage::with_box(
                CountMe::SER_ID,
                source_ref.clone(),
                group_ref.clone(),
                Box::new(CountMe),
            );
            group.route(msg, system.logger());
        }
        std::thread::sleep(SLEEP_TIME);
        assert_eq!((NUM_MESSAGES + 4) * GROUP_SIZE, total_count(&receivers));
        let counts = individual_count(&receivers);
        assert_eq!(NUM_MESSAGES + 4, counts[0]);
        assert_eq!(NUM_MESSAGES + 4, counts[1]);
        assert_eq!(NUM_MESSAGES + 4, counts[2]);

        system.shutdown().expect("shutdown");
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct CountMe;
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
    struct ReceiverComponent {
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
            match_deser!(msg; {
                _count_me: CountMe [CountMe] => self.count += 1,
                !Err(e) => error!(self.log(), "Received something else: {:?}", e),
            });
            Handled::Ok
        }
    }
}
