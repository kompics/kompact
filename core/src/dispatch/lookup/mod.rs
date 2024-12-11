//! Data structures for looking up the dispatch/routing table.
//!
//! Suggested approaches are:
//!     1. Arc<Mutex<HashMap>>>
//!     2. Some concurrent hashmap crate (check crates.io)
//!     3. Thread-local caching backed by a slower lookup method (like Arc<Mutex<...>>)
//!     4. Broadcast to _all_ listeners, ensuring that the route/lookup exists in at least one of them.

use crate::{
    actors::{ActorPath, DynActorRef, PathParseError},
    messaging::{NetMessage, PathResolvable},
    routing::groups::{
        RoutingGroup,
        RoutingPolicy,
        StorePolicy,
        DEFAULT_BROADCAST_POLICY,
        DEFAULT_SELECT_POLICY,
    },
};
use rustc_hash::FxHashMap;
use std::ops::Deref;
use uuid::Uuid;

pub(crate) mod gc;
pub(crate) mod path_trie;
use path_trie::*;

/// Result of an actor path lookup
#[derive(Debug)]
pub enum LookupResult<'a> {
    /// Result is a single concrete actor
    Ref(&'a DynActorRef),
    /// Result is a group of actors with a routing policy
    Group(RoutingGroup<'a>),
    /// There was no match
    None,
    /// Some error occurred during lookup
    Err(String),
}
impl LookupResult<'_> {
    /// Returns true if there is no result
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        matches!(self, LookupResult::None)
    }
}
impl<'a> From<Option<&'a DynActorRef>> for LookupResult<'a> {
    fn from(entry: Option<&'a DynActorRef>) -> Self {
        match entry {
            Some(aref) => LookupResult::Ref(aref),
            None => LookupResult::None,
        }
    }
}

/// Result of an actor path insert
///
/// That is, the returned value is the previous entry.
#[derive(Debug)]
pub enum InsertResult {
    /// Entry is a single concrete actor
    Ref(DynActorRef),
    /// Entry is a routing policy
    Policy(StorePolicy),
    /// There was no previous entry
    None,
}
impl InsertResult {
    /// Returns true if there is no result
    pub fn is_empty(&self) -> bool {
        matches!(self, InsertResult::None)
    }
}
impl From<Option<ActorTreeEntry>> for InsertResult {
    fn from(entry: Option<ActorTreeEntry>) -> Self {
        match entry {
            Some(ActorTreeEntry::Ref(aref)) => InsertResult::Ref(aref),
            Some(ActorTreeEntry::Policy(policy)) => InsertResult::Policy(policy),
            None => InsertResult::None,
        }
    }
}
impl From<Option<DynActorRef>> for InsertResult {
    fn from(entry: Option<DynActorRef>) -> Self {
        match entry {
            Some(aref) => InsertResult::Ref(aref),
            None => InsertResult::None,
        }
    }
}

/// A trait for actor lookup mechanisms
pub trait ActorLookup: Clone {
    /// Inserts or replaces the `path` in the lookup structure.
    ///
    /// If an entry already exists, it is removed and returned before being replaced.
    fn insert(
        &mut self,
        path: PathResolvable,
        actor: DynActorRef,
    ) -> Result<InsertResult, PathParseError>;

    /// Set the routing policy at the provided place in the actor tree to be `policy`
    ///
    /// Returns the old entry, if one existed (could be a policy or an actor reference).
    fn set_routing_policy(
        &mut self,
        path: &[String],
        policy: StorePolicy,
    ) -> Result<InsertResult, PathParseError>;

    /// Returns true if the evaluated path is already stored.
    fn contains(&self, path: &PathResolvable) -> bool;

    /// Lookup the `path` in the store
    fn get_by_actor_path<'a>(&'a self, path: &ActorPath) -> LookupResult<'a> {
        match path {
            ActorPath::Unique(ref up) => match self.get_by_uuid(&up.id()) {
                Some(aref) => LookupResult::Ref(aref),
                None => LookupResult::None,
            },
            ActorPath::Named(ref np) => self.get_by_named_path(np.path_ref()),
        }
    }

    /// Lookup the `id` in the store
    fn get_by_uuid(&self, id: &Uuid) -> Option<&DynActorRef>;

    /// Lookup the `path` in the store
    fn get_by_named_path<'a>(&'a self, path: &[String]) -> LookupResult<'a>;

    // fn get_mut_by_actor_path(&mut self, path: &ActorPath) -> Option<&mut DynActorRef> {
    //     match path {
    //         ActorPath::Unique(ref up) => self.get_mut_by_uuid(&up.id()),
    //         ActorPath::Named(ref np) => self.get_mut_by_named_path(np.path_ref()),
    //     }
    // }

    // fn get_mut_by_uuid(&mut self, id: &Uuid) -> Option<&mut DynActorRef>;

    // fn get_mut_by_named_path(&mut self, path: &[String]) -> Option<&mut DynActorRef>;

    /// Removes all entries that point to the provided actor, returning how many were removed
    /// (most likely O(n), subject to implementation details)
    fn remove(&mut self, actor: DynActorRef) -> usize;

    /// Removes the value at the given key, returning `true` if it existed.
    fn remove_by_uuid(&mut self, id: &Uuid) -> bool;

    /// Removes the value at the given key, returning `true` if it existed.
    fn remove_by_named_path(&mut self, path: &[String]) -> bool;

    /// Performs cleanup on this lookup table, returning how many entries were affected.
    fn cleanup(&mut self) -> usize {
        0
    }
}

#[derive(Debug, Clone)]
enum ActorTreeEntry {
    Ref(DynActorRef),
    Policy(StorePolicy),
}

/// Lookup structure for storing and retrieving `DynActorRef`s.
///
/// UUID-based references are stored in a `HashMap`, and path-based named references
/// are stored in a Trie structure.
///
/// # Notes
/// The sequence trie supports the use case of grouping many [DynActorRef] under the same path,
/// similar to a directory structure. Thus, actors can broadcast to all actors under a certain path
/// without requiring explicit identifiers for them.
///
/// Ex: Broadcasting a message to actors stored under the system path "tcp://127.0.0.1:8080/pongers/*"
/// Ex: Selecting one of the actors stored under the system path "tcp://127.0.0.1:8080/pongers/?" for delivery
#[derive(Clone)]
pub struct ActorStore {
    uuid_map: FxHashMap<Uuid, DynActorRef>,
    name_map: PathTrie<ActorTreeEntry>,
    deadletter: Option<DynActorRef>,
}

impl ActorStore {
    /// Return a new, empty instance
    pub fn new() -> Self {
        ActorStore {
            uuid_map: FxHashMap::default(),
            name_map: PathTrie::new(),
            deadletter: None,
        }
    }
}

impl Default for ActorStore {
    fn default() -> Self {
        Self::new()
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
                    let keys = np.into_path();
                    let prev = self.name_map.insert_owned(keys, ActorTreeEntry::Ref(actor));
                    Ok(prev.into())
                }
            },
            PathResolvable::Alias(ref alias) => {
                let path = crate::actors::parse_path(alias);
                crate::actors::validate_insert_path(&path)?;
                let prev = self.name_map.insert_owned(path, ActorTreeEntry::Ref(actor));
                Ok(prev.into())
            }
            PathResolvable::Segments(path) => {
                crate::actors::validate_insert_path(&path)?;
                let prev = self.name_map.insert_owned(path, ActorTreeEntry::Ref(actor));
                Ok(prev.into())
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
        crate::actors::validate_insert_path(path)?;
        let prev = self
            .name_map
            .insert_owned(path.to_vec(), ActorTreeEntry::Policy(policy));
        Ok(prev.into())
    }

    fn contains(&self, path: &PathResolvable) -> bool {
        match path {
            PathResolvable::Path(actor_path) => match actor_path {
                ActorPath::Unique(ref up) => self.uuid_map.contains_key(&up.id()),
                ActorPath::Named(ref np) => {
                    let keys = np.path_ref();
                    debug_assert!(
                        crate::actors::validate_lookup_path(keys).is_ok(),
                        "Path contains illegal characters: {:?}",
                        keys
                    );
                    self.name_map.get(keys).is_some()
                }
            },
            PathResolvable::Alias(ref alias) => {
                let path = crate::actors::parse_path(alias);
                debug_assert!(
                    crate::actors::validate_lookup_path(&path).is_ok(),
                    "Path contains illegal characters: {:?}",
                    path
                );
                self.name_map.get(&path).is_some()
            }
            PathResolvable::Segments(ref path) => {
                debug_assert!(
                    crate::actors::validate_lookup_path(path).is_ok(),
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
        use crate::actors::{BROADCAST_MARKER, SELECT_MARKER};
        if path.is_empty() {
            self.deadletter.as_ref().into()
        } else {
            debug_assert!(
                crate::actors::validate_lookup_path(path).is_ok(),
                "Path contains illegal characters: {:?}",
                path
            );
            let last_index = path.len() - 1;
            let (lookup_path, marker_opt): (&'b [String], Option<char>) = match path[last_index]
                .chars()
                .next()
                .expect("Should not be empty")
            {
                // this is validated above
                BROADCAST_MARKER => (&path[..last_index], Some(BROADCAST_MARKER)),
                SELECT_MARKER => (&path[..last_index], Some(SELECT_MARKER)),
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
                        if cfg!(feature = "implicit_routes") {
                            // implicit routing
                            let policy: &(dyn RoutingPolicy<DynActorRef, NetMessage>
                                  + Send
                                  + Sync) = match marker {
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
                    } else {
                        LookupResult::None
                    }
                }
                None => LookupResult::None,
            }
        }
    }

    // fn get_mut_by_uuid(&mut self, id: &Uuid) -> Option<&mut DynActorRef> {
    //     self.uuid_map.get_mut(id)
    // }

    // fn get_mut_by_named_path(&mut self, path: &[String]) -> Option<&mut DynActorRef> {
    //     if path.is_empty() {
    //         self.deadletter.as_mut()
    //     } else {
    //         self.name_map.get_mut(path)
    //     }
    // }

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
        // let matches: Vec<_> = self
        //     .name_map
        //     .iter()
        //     .filter(|&(_, entry)| {
        //         if let ActorTreeEntry::Ref(other_actor) = entry {
        //             actor == other_actor
        //         } else {
        //             false
        //         }
        //     })
        //     .map(|(key, _)| {
        //         // Clone the entire Vec<&String> path to be able to remove entries below
        //         let mut cl = Vec::new();
        //         for k in key {
        //             cl.push(k.clone());
        //         }
        //         cl
        //     })
        //     .collect();
        // let existed = matches.len();
        // for m in matches {
        //     self.name_map.remove(&m);
        // }
        // existed
        self.name_map.retain(|entry| {
            if let ActorTreeEntry::Ref(other_actor) = entry {
                actor != other_actor
            } else {
                true
            }
        })
    }

    /// Walks through the `name_map`and `uuid_map`, removing [DynActorRef]s which have been
    /// deallocated by the runtime lifecycle management.
    fn remove_deallocated_entries(&mut self) -> usize {
        let mut existed = 0;
        // let matches: Vec<_> = self
        //     .name_map
        //     .iter()
        //     .filter(|&(_, entry)| {
        //         if let ActorTreeEntry::Ref(actor) = entry {
        //             !actor.can_upgrade_component()
        //         } else {
        //             false
        //         }
        //     })
        //     .map(|(key, _)| {
        //         // Clone the entire Vec<&String> path to be able to remove entries below
        //         let mut cl = Vec::new();
        //         for k in key {
        //             cl.push(k.clone());
        //         }
        //         cl
        //     })
        //     .collect();
        // existed += matches.len();
        // for m in matches {
        //     self.name_map.remove(&m);
        // }
        existed += self.name_map.retain(|entry| {
            if let ActorTreeEntry::Ref(actor) = entry {
                actor.can_upgrade_component()
            } else {
                true
            }
        });

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
