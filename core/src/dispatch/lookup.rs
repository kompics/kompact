//! Data structures for looking up the dispatch/routing table.
//!
//! Suggested approaches are:
//!     1. Arc<Mutex<HashMap>>>
//!     2. Some concurrent hashmap crate (check crates.io)
//!     3. Thread-local caching backed by a slower lookup method (like Arc<Mutex<...>>)
//!     4. Broadcast to _all_ listeners, ensuring that the route/lookup exists in at least one of them.

use actors::ActorPath;
use actors::ActorRef;
use actors::NamedPath;
use actors::UniquePath;
use std::collections::HashMap;
use trie::SequenceTrie;
use uuid::Uuid;

/// Lookup structure for storing and retrieving `ActorRef`s.
///
/// UUID-based references are stored in a `HashMap`, and path-based named references
/// are stored in a Trie structure.
pub struct ActorLookup {
    uuid_map: HashMap<Uuid, ActorRef>,
    name_map: SequenceTrie<String, ActorRef>,
}

// impl ActorLookup
// TODO refactor ActorPaths so that there's an enum key that can be used for all get/put functions
impl ActorLookup {
    pub fn new() -> Self {
        ActorLookup {
            uuid_map: HashMap::new(),
            name_map: SequenceTrie::new(),
        }
    }

    pub fn get_by_uuid(&self, id: &Uuid) -> Option<&ActorRef> {
        self.uuid_map.get(id)
    }

    pub fn get_by_named_path(&self, path: &Vec<String>) -> Option<&ActorRef> {
        self.name_map.get(path)
    }

    pub fn get_mut_by_uuid(&mut self, id: &Uuid) -> Option<&mut ActorRef> {
        self.uuid_map.get_mut(id)
    }

    pub fn get_mut_by_named_path(&mut self, path: &Vec<String>) -> Option<&mut ActorRef> {
        self.name_map.get_mut(path)
    }

    /// Inserts or replaces the `path` in the lookup structure.
    /// If an entry already exists, it is removed and returned before being replaced.
    pub fn insert(&mut self, actor: ActorRef) -> Option<ActorRef> {
        unimplemented!(); // FIXME @Johan update for new code
                          // use actors::ActorRefFactory;

        // let path = actor.actor_path();
        // match path {
        //     ActorPath::Unique(up) => {
        //         let key = up.clone_id();
        //         self.uuid_map.insert(key, actor)
        //     }
        //     ActorPath::Named(np) => {
        //         let keys = np.path_ref();
        //         self.name_map.insert(keys, actor)
        //     }
        // }
    }
}
