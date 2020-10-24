//! Data structures for looking up the dispatch/routing table.
//!
//! Suggested approaches are:
//!     1. Arc<Mutex<HashMap>>>
//!     2. Some concurrent hashmap crate (check crates.io)
//!     3. Thread-local caching backed by a slower lookup method (like Arc<Mutex<...>>)
//!     4. Broadcast to _all_ listeners, ensuring that the route/lookup exists in at least one of them.

use kompact::{
    lookup::{ActorLookup, InsertResult, LookupResult},
    messaging::{NetMessage, PathResolvable},
    prelude::{ActorPath, DynActorRef, PathParseError},
    routing::groups::{
        RoutingGroup,
        RoutingPolicy,
        StorePolicy,
        DEFAULT_BROADCAST_POLICY,
        DEFAULT_SELECT_POLICY,
    },
};
use rustc_hash::FxHashMap;
use sequence_trie::SequenceTrie;
use std::ops::Deref;
use uuid::Uuid;

#[derive(Debug, Clone)]
enum ActorTreeEntry {
    Ref(DynActorRef),
    Policy(StorePolicy),
}

impl ActorTreeEntry {
    fn into_result(entry: Option<ActorTreeEntry>) -> InsertResult {
        match entry {
            Some(ActorTreeEntry::Ref(aref)) => InsertResult::Ref(aref),
            Some(ActorTreeEntry::Policy(policy)) => InsertResult::Policy(policy),
            None => InsertResult::None,
        }
    }
}

/// Lookup structure for storing and retrieving `DynActorRef`s.
///
/// UUID-based references are stored in a [HashMap], and path-based named references
/// are stored in a Trie structure.
///
/// # Notes
/// The sequence trie supports the use case of grouping many [DynActorRefs] under the same path,
/// similar to a directory structure. Thus, actors can broadcast to all actors under a certain path
/// without requiring explicit identifiers for them.

/// Ex: Broadcasting a message to actors stored on the system path "tcp://127.0.0.1:8080/pongers/*"
///
/// This use case is not currently being utilized, but it may be in the future.
#[derive(Clone)]
pub struct ActorStore {
    uuid_map: FxHashMap<Uuid, DynActorRef>,
    name_map: SequenceTrie<String, ActorTreeEntry>,
    deadletter: Option<DynActorRef>,
}

impl ActorStore {
    pub fn new() -> Self {
        ActorStore {
            uuid_map: FxHashMap::default(),
            name_map: SequenceTrie::new(),
            deadletter: None,
        }
    }
}

impl ActorLookup for ActorStore {
    fn insert(
        &mut self,
        path: PathResolvable,
        actor: DynActorRef,
    ) -> Result<InsertResult, PathParseError> {
        match path {
            PathResolvable::Path(actor_path) => match actor_path {
                ActorPath::Unique(up) => {
                    let key = up.id();
                    Ok(self.uuid_map.insert(key, actor).into())
                }
                ActorPath::Named(np) => {
                    let keys = np.path_ref();
                    let prev = self.name_map.insert(keys, ActorTreeEntry::Ref(actor));
                    Ok(ActorTreeEntry::into_result(prev))
                }
            },
            PathResolvable::Alias(ref alias) => {
                let path = kompact::prelude_bench::parse_path(alias);
                kompact::prelude_bench::validate_insert_path(&path)?;
                let prev = self.name_map.insert(&path, ActorTreeEntry::Ref(actor));
                Ok(ActorTreeEntry::into_result(prev))
            }
            PathResolvable::Segments(path) => {
                kompact::prelude_bench::validate_insert_path(&path)?;
                let prev = self.name_map.insert(&path, ActorTreeEntry::Ref(actor));
                Ok(ActorTreeEntry::into_result(prev))
            }
            PathResolvable::ActorId(uuid) => Ok(self.uuid_map.insert(uuid, actor).into()),
            PathResolvable::System => Ok(self.deadletter.replace(actor).into()),
        }
    }

    fn set_routing_policy(
        &mut self,
        path: &[String],
        policy: StorePolicy,
    ) -> Result<InsertResult, PathParseError> {
        kompact::prelude_bench::validate_insert_path(path)?;
        let prev = self.name_map.insert(path, ActorTreeEntry::Policy(policy));
        Ok(ActorTreeEntry::into_result(prev))
    }

    fn contains(&self, path: &PathResolvable) -> bool {
        match path {
            PathResolvable::Path(actor_path) => match actor_path {
                ActorPath::Unique(ref up) => self.uuid_map.contains_key(&up.id()),
                ActorPath::Named(ref np) => {
                    let keys = np.path_ref();
                    debug_assert!(
                        kompact::prelude_bench::validate_lookup_path(&keys).is_ok(),
                        "Path contains illegal characters: {:?}",
                        keys
                    );
                    self.name_map.get(keys).is_some()
                }
            },
            PathResolvable::Alias(ref alias) => {
                let path = kompact::prelude_bench::parse_path(alias);
                debug_assert!(
                    kompact::prelude_bench::validate_lookup_path(&path).is_ok(),
                    "Path contains illegal characters: {:?}",
                    path
                );
                self.name_map.get(&path).is_some()
            }
            PathResolvable::Segments(ref path) => {
                debug_assert!(
                    kompact::prelude_bench::validate_lookup_path(path).is_ok(),
                    "Path contains illegal characters: {:?}",
                    path
                );
                self.name_map.get(path).is_some()
            }
            PathResolvable::ActorId(ref uuid) => self.uuid_map.contains_key(uuid),
            PathResolvable::System => self.deadletter.is_some(),
        }
    }

    fn get_by_uuid(&self, id: &Uuid) -> Option<&DynActorRef> {
        self.uuid_map.get(id)
    }

    fn get_by_named_path<'a, 'b>(&'a self, path: &'b [String]) -> LookupResult<'a> {
        use kompact::constants::{BROADCAST_MARKER, SELECT_MARKER};
        if path.is_empty() {
            self.deadletter.as_ref().into()
        } else {
            let last_index = path.len() - 1;
            let (lookup_path, marker_opt): (&'b [String], Option<char>) = match path[last_index]
                .chars()
                .next()
                .expect("Should not be empty")
            {
                // this is validated above
                BROADCAST_MARKER => (&path[0..last_index], Some(BROADCAST_MARKER)),
                SELECT_MARKER => (&path[0..last_index], Some(SELECT_MARKER)),
                _ => (path, None),
            };
            match self.name_map.get_node(lookup_path) {
                Some(node) => {
                    if let Some(entry) = node.value() {
                        match entry {
                            ActorTreeEntry::Ref(aref) => {
                                if let Some(marker) = marker_opt {
                                    LookupResult::Err(format!("Expected a routing policy (marker={}), but found an actor reference at path={:?}", marker, path))
                                } else {
                                    LookupResult::Ref(aref)
                                }
                            }
                            ActorTreeEntry::Policy(policy) => {
                                let children: Vec<&'a DynActorRef> = node
                                    .values()
                                    .flat_map(|v| {
                                        if let ActorTreeEntry::Ref(aref) = v {
                                            Some(aref)
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();
                                if let Some(marker) = marker_opt {
                                    // requested routing
                                    match marker {
                                        BROADCAST_MARKER => {
                                            if let Some(broadcast_policy) = policy.broadcast() {
                                                let group =
                                                    RoutingGroup::new(children, broadcast_policy);
                                                LookupResult::Group(group)
                                            } else {
                                                LookupResult::Err(format!("Expected a broadcast policy (marker={}), but found a non-broadcast policy at path={:?}", marker, path))
                                            }
                                        }
                                        SELECT_MARKER => {
                                            if let Some(select_policy) = policy.select() {
                                                let group =
                                                    RoutingGroup::new(children, select_policy);
                                                LookupResult::Group(group)
                                            } else {
                                                LookupResult::Err(format!("Expected a select policy (marker={}), but found a non-select policy at path={:?}", marker, path))
                                            }
                                        }
                                        _ => unreachable!(
                                            "Only put marker characters in to marker field!"
                                        ),
                                    }
                                } else {
                                    // transparent routing
                                    let group = RoutingGroup::new(children, policy.deref());
                                    LookupResult::Group(group)
                                }
                            }
                        }
                    } else if let Some(marker) = marker_opt {
                        // implicit routing
                        let policy: &(dyn RoutingPolicy<DynActorRef, NetMessage> + Send + Sync) =
                            match marker {
                                BROADCAST_MARKER => &DEFAULT_BROADCAST_POLICY,
                                SELECT_MARKER => &DEFAULT_SELECT_POLICY,
                                _ => unreachable!("Only put marker characters in to marker field!"),
                            };
                        let children: Vec<&'a DynActorRef> = node
                            .values()
                            .flat_map(|v| {
                                if let ActorTreeEntry::Ref(aref) = v {
                                    Some(aref)
                                } else {
                                    None
                                }
                            })
                            .collect();
                        let group = RoutingGroup::new(children, policy);
                        LookupResult::Group(group)
                    } else {
                        LookupResult::None
                    }
                }
                None => LookupResult::None,
            }
        }
    }

    fn remove(&mut self, actor: DynActorRef) -> usize {
        let mut num_deleted = 0;
        num_deleted += self.remove_from_uuid_map(&actor);
        num_deleted += self.remove_from_name_map(&actor);
        num_deleted
    }

    fn remove_by_uuid(&mut self, id: &Uuid) -> bool {
        self.uuid_map.remove(id).is_some()
    }

    fn remove_by_named_path(&mut self, path: &[String]) -> bool {
        let existed = self.name_map.get(path).is_some();
        self.name_map.remove(path);
        existed
    }

    fn cleanup(&mut self) -> usize {
        self.remove_deallocated_entries()
    }
}

impl ActorStore {
    fn remove_from_uuid_map(&mut self, actor: &DynActorRef) -> usize {
        let matches: Vec<_> = self
            .uuid_map
            .iter()
            .filter(|rec| rec.1 == actor)
            .map(|rec| *rec.0)
            .collect();
        let existed = matches.len();
        for m in matches {
            self.uuid_map.remove(&m);
        }
        existed
    }

    fn remove_from_name_map(&mut self, actor: &DynActorRef) -> usize {
        let matches: Vec<_> = self
            .name_map
            .iter()
            .filter(|&(_, entry)| {
                if let ActorTreeEntry::Ref(other_actor) = entry {
                    actor == other_actor
                } else {
                    false
                }
            })
            .map(|(key, _)| {
                // Clone the entire Vec<&String> path to be able to remove entries below
                let mut cl = Vec::new();
                for k in key {
                    cl.push(k.clone());
                }
                cl
            })
            .collect();
        let existed = matches.len();
        for m in matches {
            self.name_map.remove(&m);
        }
        existed
    }

    /// Walks through the `name_map`and `uuid_map`, removing [DynActorRef]s which have been
    /// deallocated by the runtime lifecycle management.
    fn remove_deallocated_entries(&mut self) -> usize {
        let mut existed = 0;
        let matches: Vec<_> = self
            .name_map
            .iter()
            .filter(|&(_, entry)| {
                if let ActorTreeEntry::Ref(actor) = entry {
                    !actor.can_upgrade_component()
                } else {
                    false
                }
            })
            .map(|(key, _)| {
                // Clone the entire Vec<&String> path to be able to remove entries below
                let mut cl = Vec::new();
                for k in key {
                    cl.push(k.clone());
                }
                cl
            })
            .collect();
        existed += matches.len();
        for m in matches {
            self.name_map.remove(&m);
        }

        let matches: Vec<_> = self
            .uuid_map
            .iter()
            .filter(|&(_, actor)| !actor.can_upgrade_component())
            .map(|(key, _)| *key)
            .collect();
        existed += matches.len();
        for m in matches {
            self.uuid_map.remove(&m);
        }
        existed
    }
}
