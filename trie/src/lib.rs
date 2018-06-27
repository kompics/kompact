//! Trie -- a hashmap-based trie data structure for storing values along paths

use std::collections::HashMap;
use std::hash::Hash;
use std::mem;

//  TODOs:
//  - implement removal

pub struct Trie<K, V> {
    value: Option<V>,
    children: HashMap<K, Trie<K, V>>,
}

// impl Trie
impl<K, V> Trie<K, V>
where
    K: Hash + Eq,
{
    /// Creates a new trie with default capacity.
    pub fn new() -> Self {
        Trie {
            value: None,
            children: HashMap::new(),
        }
    }

    /// Returns a reference to this node's value.
    pub fn value(&self) -> Option<&V> {
        self.value.as_ref()
    }

    /// Returns a mutable reference to this node's value.
    pub fn value_mut(&mut self) -> Option<&mut V> {
        self.value.as_mut()
    }

    /// Returns true if this node has no children.
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    /// Returns true if this node is a leaf node and has no assigned value.
    pub fn is_empty(&self) -> bool {
        self.value.is_none() && self.is_leaf()
    }

    /// Returns true if this trie contains all keys in `keys`.
    pub fn contains_keys<IK>(&self, keys: IK) -> bool
    where
        IK: IntoIterator<Item = K>,
    {
        self.find_node(keys).is_some()
    }

    /// Inserts `value` at the last element in `keys`. If a value existed previously, it is returned.
    ///
    /// If any of the elements in `keys` does not yet exist, a node will be created for it.
    pub fn insert<IK>(&mut self, keys: IK, value: V) -> Option<V>
    where
        IK: IntoIterator<Item = K>,
    {
        let value_node = keys
            .into_iter()
            .fold(self, |node: &mut Trie<K, V>, key: K| {
                node.children.entry(key).or_insert(Trie::new())
            });
        mem::replace(&mut value_node.value, Some(value))
    }

    /// Retrieves a reference to the value at the final key in `keys`, if it exists.
    pub fn find<IK>(&self, keys: IK) -> Option<&V>
    where
        IK: IntoIterator<Item = K>,
    {
        self.find_node(keys).and_then(|node| node.value())
    }

    /// Retrieves the trie node at the final key in `keys`, if it exists.
    pub fn find_node<IK>(&self, keys: IK) -> Option<&Trie<K, V>>
    where
        IK: IntoIterator<Item = K>,
    {
        let mut cur = self;
        for key in keys {
            match cur.children.get(&key) {
                None => return None,
                Some(next) => cur = next,
            }
        }
        Some(cur)
    }

    /// Retrieves a mutable reference to the value at the final key in `keys`, if it exists.
    pub fn find_mut<IK>(&mut self, keys: IK) -> Option<&mut V>
        where
            IK: IntoIterator<Item = K>,
    {
        self.find_node_mut(keys).and_then(|node| node.value_mut())
    }

    /// Retrieves a mutable reference to the trie node at the final key in `keys`, if it exists.
    pub fn find_node_mut<IK>(&mut self, keys: IK) -> Option<&mut Trie<K, V>>
        where
            IK: IntoIterator<Item = K>,
    {
        let mut cur = Some(self);
        for key in keys {
            match cur.and_then(|node| node.children.get_mut(&key)) {
                None => return None,
                Some(next) => cur = Some(next),
            }
        }
        cur
    }

}

#[cfg(test)]
mod tests {
    use super::Trie;

    #[test]
    fn creation_and_insertion() {
        let mut t = Trie::new();

        t.insert(vec!["a", "b", "c"], 10);

        // normal insertion
        assert_eq!(t.find(vec!["a", "b", "c"]), Some(&10));
        // replace and take ownership of previous inserted value
        assert_eq!(t.insert(vec!["a", "b", "c"], 99), Some(10));
        // verify new value
        assert_eq!(t.find(vec!["a", "b", "c"]), Some(&99));
        // negative case: node but no value
        assert_eq!(t.find(vec!["a"]), None);
        assert_eq!(t.find(vec!["a", "b"]), None);
        // negative case: no node
        assert_eq!(t.find(vec![]), None);
        assert_eq!(t.find(vec!["a", "b", "d"]), None);

        // verify sequences exist
        assert_eq!(t.contains_keys(vec!["a"]), true);
        assert_eq!(t.contains_keys(vec!["a", "b"]), true);
        assert_eq!(t.contains_keys(vec!["a", "b", "c"]), true);
        assert_eq!(t.contains_keys(vec!["a", "b", "d"]), false);
    }

    #[test]
    fn creation_and_insertion_mut() {
        let mut t = Trie::new();

        t.insert(vec!["a", "b", "c"], 10);

        assert_eq!(t.find(vec!["a", "b", "c"]), Some(&10));

        t.find_mut(vec!["a", "b", "c"]).map(|val| {
            *val = 99;
        });
        assert_eq!(t.find(vec!["a", "b", "c"]), Some(&99));
    }
}
