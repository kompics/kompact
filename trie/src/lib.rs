//! Trie -- a hashmap-based trie data structure for storing values along paths

use std::collections::HashMap;
use std::hash::Hash;
use std::mem;

pub struct Trie<K, V> {
    value: Option<V>,
    children: HashMap<K, Trie<K, V>>,
}

// impl Trie
impl<K, V> Trie<K, V>
where
    K: Hash + Eq,
{
    pub fn new() -> Self {
        Trie {
            value: None,
            children: HashMap::new(),
        }
    }
    pub fn with_capacity(capacity: usize) -> Self {
        Trie {
            value: None,
            children: HashMap::with_capacity(capacity),
        }
    }

    pub fn value(&self) -> Option<&V> {
        self.value.as_ref()
    }

    pub fn value_mut(&mut self) -> Option<&mut V> {
        self.value.as_mut()
    }

    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_none() && self.is_leaf()
    }

    pub fn contains_keys<IK>(&self, keys: IK) -> bool
    where
        IK: IntoIterator<Item = K>,
    {
        self.find_node(keys).is_some()
    }

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

    pub fn find<IK>(&self, keys: IK) -> Option<&V>
    where
        IK: IntoIterator<Item = K>,
    {
        self.find_node(keys).and_then(|node| node.value())
    }

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
}
