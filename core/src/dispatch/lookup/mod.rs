//! Data structures for looking up the dispatch/routing table.
//!
//! Suggested approaches are:
//!     1. Arc<Mutex<HashMap>>>
//!     2. Some concurrent hashmap crate (check crates.io)
//!     3. Thread-local caching backed by a slower lookup method (like Arc<Mutex<...>>)
//!     4. Broadcast to _all_ listeners, ensuring that the route/lookup exists in at least one of them.

use crate::{
    actors::{ActorPath, DynActorRef},
    messaging::PathResolvable,
};
use rustc_hash::FxHashMap;
use sequence_trie::SequenceTrie;
use uuid::Uuid;

pub mod gc;

pub trait ActorLookup: Clone {
    /// Inserts or replaces the `path` in the lookup structure.
    /// If an entry already exists, it is removed and returned before being replaced.
    fn insert(&mut self, actor: DynActorRef, path: PathResolvable) -> Option<DynActorRef>;

    /// Returns true if the evaluated path is already stored.
    fn contains(&self, path: &PathResolvable) -> bool;

    fn get_by_actor_path(&self, path: &ActorPath) -> Option<&DynActorRef> {
        match path {
            ActorPath::Unique(ref up) => self.get_by_uuid(&up.id()),
            ActorPath::Named(ref np) => self.get_by_named_path(np.path_ref()),
        }
    }

    fn get_by_uuid(&self, id: &Uuid) -> Option<&DynActorRef>;

    fn get_by_named_path(&self, path: &[String]) -> Option<&DynActorRef>;

    fn get_mut_by_actor_path(&mut self, path: &ActorPath) -> Option<&mut DynActorRef> {
        match path {
            ActorPath::Unique(ref up) => self.get_mut_by_uuid(&up.id()),
            ActorPath::Named(ref np) => self.get_mut_by_named_path(np.path_ref()),
        }
    }

    fn get_mut_by_uuid(&mut self, id: &Uuid) -> Option<&mut DynActorRef>;

    fn get_mut_by_named_path(&mut self, path: &[String]) -> Option<&mut DynActorRef>;

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
    name_map: SequenceTrie<String, DynActorRef>,
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
    /// Inserts or replaces the `path` in the lookup structure.
    /// If an entry already exists, it is removed and returned before being replaced.
    fn insert(&mut self, actor: DynActorRef, path: PathResolvable) -> Option<DynActorRef> {
        match path {
            PathResolvable::Path(actor_path) => match actor_path {
                ActorPath::Unique(up) => {
                    let key = up.id();
                    self.uuid_map.insert(key, actor)
                }
                ActorPath::Named(np) => {
                    let keys = np.path_ref();
                    self.name_map.insert(keys, actor)
                }
            },
            PathResolvable::Alias(alias) => self.name_map.insert(&vec![alias], actor),
            PathResolvable::ActorId(uuid) => self.uuid_map.insert(uuid, actor),
            PathResolvable::System => self.deadletter.replace(actor),
        }
    }

    fn contains(&self, path: &PathResolvable) -> bool {
        match path {
            PathResolvable::Path(actor_path) => match actor_path {
                ActorPath::Unique(ref up) => self.uuid_map.contains_key(&up.id()),
                ActorPath::Named(ref np) => {
                    let keys = np.path_ref();
                    self.name_map.get(keys).is_some()
                }
            },
            PathResolvable::Alias(ref alias) => {
                //  TODO avoid clone here
                self.name_map.get(&[alias.clone()]).is_some()
            }
            PathResolvable::ActorId(ref uuid) => self.uuid_map.contains_key(uuid),
            PathResolvable::System => self.deadletter.is_some(),
        }
    }

    fn get_by_uuid(&self, id: &Uuid) -> Option<&DynActorRef> {
        self.uuid_map.get(id)
    }

    fn get_by_named_path(&self, path: &[String]) -> Option<&DynActorRef> {
        if path.is_empty() {
            self.deadletter.as_ref()
        } else {
            self.name_map.get(path)
        }
    }

    fn get_mut_by_uuid(&mut self, id: &Uuid) -> Option<&mut DynActorRef> {
        self.uuid_map.get_mut(id)
    }

    fn get_mut_by_named_path(&mut self, path: &[String]) -> Option<&mut DynActorRef> {
        if path.is_empty() {
            self.deadletter.as_mut()
        } else {
            self.name_map.get_mut(path)
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
            .filter(|rec| rec.1 == actor)
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
            .filter(|&(_, actor)| !actor.can_upgrade_component())
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
