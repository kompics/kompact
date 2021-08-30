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
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};

/// The default policy associated with the broadcast marker
pub static DEFAULT_BROADCAST_POLICY: BroadcastRouting = BroadcastRouting;

/// The default hasher builder
///
/// This is equivalent to `HasherBuilderDefault<std::collections::hash_map::DefaultHasher>`
/// but it is statically assignable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DefaultHasherBuilder;
impl BuildHasher for DefaultHasherBuilder {
    type Hasher = std::collections::hash_map::DefaultHasher;

    fn build_hasher(&self) -> Self::Hasher {
        std::collections::hash_map::DefaultHasher::default()
    }
}

/// The default policy associated with the select marker
pub static DEFAULT_SELECT_POLICY: SenderHashBucketRouting<DefaultHasherBuilder> =
    FieldHashBucketRouting {
        hasher_builder: DefaultHasherBuilder,
        field_extractor: NetMessage::sender,
    };

/// Cloneable [RoutingPolicy](RoutingPolicy) wrapper used inside the `ActorStore`
#[derive(Debug)]
pub struct StorePolicy(Box<dyn RoutingPolicy<DynActorRef, NetMessage> + Send + Sync>);
impl StorePolicy {
    /// Wrap a boxed [RoutingPolicy](RoutingPolicy)
    pub fn new(policy: Box<dyn RoutingPolicy<DynActorRef, NetMessage> + Send + Sync>) -> Self {
        StorePolicy(policy)
    }
}
impl Clone for StorePolicy {
    fn clone(&self) -> Self {
        StorePolicy(self.0.boxed_clone())
    }
}
impl Deref for StorePolicy {
    type Target = dyn RoutingPolicy<DynActorRef, NetMessage>;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}
impl<R> From<R> for StorePolicy
where
    R: RoutingPolicy<DynActorRef, NetMessage> + Send + Sync + 'static,
{
    fn from(policy: R) -> Self {
        let boxed = Box::new(policy);
        Self::new(boxed)
    }
}

/// A routing group keeps group membership together with a routing policy
#[derive(Debug)]
pub struct RoutingGroup<'a> {
    members: Vec<&'a DynActorRef>,
    policy: &'a dyn RoutingPolicy<DynActorRef, NetMessage>,
}
impl<'a> RoutingGroup<'a> {
    /// Create a new routing group
    pub fn new(
        members: Vec<&'a DynActorRef>,
        policy: &'a dyn RoutingPolicy<DynActorRef, NetMessage>,
    ) -> Self {
        RoutingGroup { members, policy }
    }

    /// Route `msg` to our members as instructed by our policy
    pub fn route(&self, msg: NetMessage, logger: &KompactLogger) {
        let members: &[&DynActorRef] = &self.members;
        self.policy.route(members, msg, logger);
    }
}

/// A routing policy described how to route a given `msg` over
/// a given set of `members`
pub trait RoutingPolicy<Ref, M>: fmt::Debug {
    /// Route the `msg` to the appropriate `members`
    ///
    /// A logger should be provided to allow debugging and handle potential routing errors.
    fn route(&self, members: &[&Ref], msg: M, logger: &KompactLogger);

    /// Create a boxed copy of this policy
    ///
    /// Used to make trait objects of this policy cloneable
    fn boxed_clone(&self) -> Box<dyn RoutingPolicy<Ref, M> + Send + Sync>;

    /// Provide the broadcast part of this policy, if any
    fn broadcast(&self) -> Option<&(dyn RoutingPolicy<Ref, M> + Send + Sync)>;

    /// Provide the select part of this policy, if any
    fn select(&self) -> Option<&(dyn RoutingPolicy<Ref, M> + Send + Sync)>;
}

/// Round-robin dispatch policy
///
/// This policy can handle changing membership,
/// but won't start a new round immediately on change.
#[derive(Debug)]
pub struct RoundRobinRouting {
    offset: AtomicUsize,
}
impl RoundRobinRouting {
    /// Create a new instance starting at index 0
    pub fn new() -> Self {
        RoundRobinRouting {
            offset: AtomicUsize::new(0),
        }
    }

    /// Get and return the current index and increment it for the next invocation
    pub fn get_and_increment_index(&self, length: usize) -> usize {
        // fetch_add wraps around automatically
        self.offset.fetch_add(1, Ordering::Relaxed) % length
    }
}
impl Default for RoundRobinRouting {
    fn default() -> Self {
        Self::new()
    }
}
impl RoutingPolicy<DynActorRef, NetMessage> for RoundRobinRouting {
    fn route(&self, members: &[&DynActorRef], msg: NetMessage, logger: &KompactLogger) {
        let index = self.get_and_increment_index(members.len());
        trace!(logger, "Routing msg to member at index={}", index);
        members[index].tell(msg);
    }

    fn boxed_clone(&self) -> Box<dyn RoutingPolicy<DynActorRef, NetMessage> + Send + Sync> {
        // just use the last value for the new router...it doesn't matter if there is some overlap between the two versions
        let offset = self.offset.load(Ordering::Relaxed);
        let routing = RoundRobinRouting {
            offset: AtomicUsize::new(offset),
        };
        Box::new(routing)
    }

    fn broadcast(&self) -> Option<&(dyn RoutingPolicy<DynActorRef, NetMessage> + Send + Sync)> {
        None
    }

    fn select(&self) -> Option<&(dyn RoutingPolicy<DynActorRef, NetMessage> + Send + Sync)> {
        Some(self)
    }
}

/// A router using consistent hashing on some field of the message
///
/// Changing the membership is supported,
/// but will result in bucket reassignments.
pub struct FieldHashBucketRouting<M, T: Hash, H: BuildHasher + Clone> {
    hasher_builder: H,
    field_extractor: fn(&M) -> &T,
}
impl<M, T: Hash, H: BuildHasher + Clone> Clone for FieldHashBucketRouting<M, T, H> {
    fn clone(&self) -> Self {
        FieldHashBucketRouting {
            hasher_builder: self.hasher_builder.clone(),
            field_extractor: self.field_extractor,
        }
    }
}
impl<M, T: Hash, H: BuildHasher + Clone + Copy> Copy for FieldHashBucketRouting<M, T, H> {}

impl<M, T: Hash, H: BuildHasher + Clone> fmt::Debug for FieldHashBucketRouting<M, T, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "FieldHashBucketRouting {{ hasher_builder: {}, field_extractor: {} }}",
            std::any::type_name::<H>(),
            std::any::type_name::<fn(&M) -> &T>()
        )
    }
}
impl<M, T: Hash, H: BuildHasher + Clone> FieldHashBucketRouting<M, T, H> {
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
    /// It does so by calling [hash_message](Self::hash_message) internally
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

impl<H: BuildHasher + Default + Clone> Default for SenderHashBucketRouting<H> {
    fn default() -> Self {
        let hasher_builder = H::default();
        Self::new(hasher_builder, NetMessage::sender)
    }
}

impl<H: BuildHasher + Clone + Send + Sync + 'static> RoutingPolicy<DynActorRef, NetMessage>
    for SenderHashBucketRouting<H>
{
    fn route(&self, members: &[&DynActorRef], msg: NetMessage, logger: &KompactLogger) {
        let index = self.get_bucket(&msg, members.len());
        trace!(
            logger,
            "Routing msg with sender={} to member at index={}",
            msg.sender,
            index
        );
        members[index].tell(msg);
    }

    fn boxed_clone(&self) -> Box<dyn RoutingPolicy<DynActorRef, NetMessage> + Send + Sync> {
        let cloned: SenderHashBucketRouting<H> = self.clone();
        Box::new(cloned)
    }

    fn broadcast(&self) -> Option<&(dyn RoutingPolicy<DynActorRef, NetMessage> + Send + Sync)> {
        None
    }

    fn select(&self) -> Option<&(dyn RoutingPolicy<DynActorRef, NetMessage> + Send + Sync)> {
        Some(self)
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
    fn route(&self, members: &[&DynActorRef], msg: NetMessage, logger: &KompactLogger) {
        trace!(logger, "Trying to broadcast message: {:?}", msg);
        let res = members
            .iter()
            .for_each_try_with(msg, |member, my_msg| member.tell(my_msg));
        match res {
            Ok(_) => trace!(logger, "The message was broadcast."),
            Err(e) => error!(logger, "Could not broadcast a message! Error was: {}", e),
        }
    }

    fn boxed_clone(&self) -> Box<dyn RoutingPolicy<DynActorRef, NetMessage> + Send + Sync> {
        Box::new(*self)
    }

    fn broadcast(&self) -> Option<&(dyn RoutingPolicy<DynActorRef, NetMessage> + Send + Sync)> {
        Some(self)
    }

    fn select(&self) -> Option<&(dyn RoutingPolicy<DynActorRef, NetMessage> + Send + Sync)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{prelude::*, routing::test_helpers::*};

    const GROUP_SIZE: usize = 3;
    const NUM_MESSAGES: usize = 30;
    // TODO: Replace the sleeps
    const SLEEP_TIME: Duration = Duration::from_millis(1000);

    #[test]
    fn router_debug() {
        {
            let router = RoundRobinRouting::default();
            println!("Router: {:?}", router);
            let group = RoutingGroup::new(Vec::new(), &router);
            println!("Group: {:?}", group);
        }
        {
            let router = SenderDefaultHashBucketRouting::default();
            println!("Router: {:?}", router);
            let group = RoutingGroup::new(Vec::new(), &router);
            println!("Group: {:?}", group);
        }
        {
            let router = BroadcastRouting::default();
            println!("Router: {:?}", router);
            let group = RoutingGroup::new(Vec::new(), &router);
            println!("Group: {:?}", group);
        }
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

        let router = RoundRobinRouting::default();
        let group = RoutingGroup::new(receiver_refs.iter().collect(), &router);
        let group_ref: ActorPath = NamedPath::with_system(
            system.system_path(),
            vec!["routing_group".to_string(), "?".to_string()],
        )
        .into();
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

        let router = SenderDefaultHashBucketRouting::default();
        let group = RoutingGroup::new(receiver_refs.iter().collect(), &router);
        let group_ref: ActorPath = NamedPath::with_system(
            system.system_path(),
            vec!["routing_group".to_string(), "?".to_string()],
        )
        .into();
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

        let other_source_ref: ActorPath = system
            .system_path()
            .into_named_with_string("othersource")
            .expect("actor path")
            .into();

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

        let router = BroadcastRouting::default();
        let group = RoutingGroup::new(receiver_refs.iter().collect(), &router);
        let group_ref: ActorPath = NamedPath::with_system(
            system.system_path(),
            vec!["routing_group".to_string(), "*".to_string()],
        )
        .into();
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
}
