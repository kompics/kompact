//! Data structures for looking up the dispatch/routing table.
//!
//! Suggested approaches are:
//!     1. Arc<Mutex<HashMap>>>
//!     2. Some concurrent hashmap crate (check crates.io)
//!     3. Thread-local caching backed by a slower lookup method (like Arc<Mutex<...>>)
//!     4. Broadcast to _all_ listeners, ensuring that the route/lookup exists in at least one of them.

use actors::ActorPath;
use actors::ActorRef;
use messaging::PathResolvable;
use std::collections::HashMap;
use trie::SequenceTrie;
use uuid::Uuid;
use std::sync::Arc;
use std::sync::Mutex;

pub trait ActorLookup {


    /// Inserts or replaces the `path` in the lookup structure.
    /// If an entry already exists, it is removed and returned before being replaced.
    fn insert(&mut self, actor: ActorRef, path: PathResolvable) -> Option<ActorRef>;

    fn get_by_actor_path(&self, path: &ActorPath) -> Option<&ActorRef> {
        match path {
            ActorPath::Unique(ref up) => self.get_by_uuid(up.uuid_ref()),
            ActorPath::Named(ref np) => self.get_by_named_path(np.path_ref()),
        }
    }

    fn get_by_uuid(&self, id: &Uuid) -> Option<&ActorRef>;

    fn get_by_named_path(&self, path: &Vec<String>) -> Option<&ActorRef>;

    fn get_mut_by_actor_path(&mut self, path: &ActorPath) -> Option<&mut ActorRef> {
        match path {
            ActorPath::Unique(ref up) => self.get_mut_by_uuid(up.uuid_ref()),
            ActorPath::Named(ref np) => self.get_mut_by_named_path(np.path_ref()),
        }
    }

    fn get_mut_by_uuid(&mut self, id: &Uuid) -> Option<&mut ActorRef>;

    fn get_mut_by_named_path(&mut self, path: &Vec<String>) -> Option<&mut ActorRef>;

}

/// Lookup structure for storing and retrieving `ActorRef`s.
///
/// UUID-based references are stored in a `HashMap`, and path-based named references
/// are stored in a Trie structure.
pub struct ActorStore {
    uuid_map: HashMap<Uuid, ActorRef>,
    name_map: SequenceTrie<String, ActorRef>,
}

// impl ActorStore
// TODO refactor ActorPaths so that there's an enum key that can be used for all get/put functions
impl ActorStore {
    pub fn new() -> Self {
        ActorStore {
            uuid_map: HashMap::new(),
            name_map: SequenceTrie::new(),
        }
    }
}

impl ActorLookup for ActorStore {
    /// Inserts or replaces the `path` in the lookup structure.
    /// If an entry already exists, it is removed and returned before being replaced.
    fn insert(&mut self, actor: ActorRef, path: PathResolvable) -> Option<ActorRef> {
        match path {
            PathResolvable::Path(actor_path) => match actor_path {
                ActorPath::Unique(up) => {
                    let key = up.clone_id();
                    self.uuid_map.insert(key, actor)
                }
                ActorPath::Named(np) => {
                    let keys = np.path_ref();
                    self.name_map.insert(keys, actor)
                }
            },
            PathResolvable::ActorId(uuid) => self.uuid_map.insert(uuid, actor),
            PathResolvable::System => {
                panic!("System paths should not be registered");
            }
        }
    }

    fn get_by_uuid(&self, id: &Uuid) -> Option<&ActorRef> {
        self.uuid_map.get(id)
    }

    fn get_by_named_path(&self, path: &Vec<String>) -> Option<&ActorRef> {
        self.name_map.get(path)
    }

    fn get_mut_by_uuid(&mut self, id: &Uuid) -> Option<&mut ActorRef> {
        self.uuid_map.get_mut(id)
    }

    fn get_mut_by_named_path(&mut self, path: &Vec<String>) -> Option<&mut ActorRef> {
        self.name_map.get_mut(path)
    }
}

impl Clone for ActorStore {
    fn clone(&self) -> Self {
        ActorStore {
            uuid_map: self.uuid_map.clone(),
            name_map: self.name_map.clone(),
        }
    }
}
