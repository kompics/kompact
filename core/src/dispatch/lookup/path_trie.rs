use rustc_hash::FxHashMap;
use std::{borrow::Borrow, collections::hash_map, hash::Hash};

#[derive(Debug, Clone)]
pub struct PathTrie<V> {
    value: Option<V>,
    children: FxHashMap<String, PathTrie<V>>,
}

impl<V> PathTrie<V> {
    pub fn new() -> Self {
        PathTrie {
            value: None,
            children: FxHashMap::default(),
        }
    }

    #[allow(dead_code)]
    pub fn with_value(value: V) -> Self {
        PathTrie {
            value: Some(value),
            children: FxHashMap::default(),
        }
    }

    pub fn value(&self) -> Option<&V> {
        self.value.as_ref()
    }

    #[allow(dead_code)]
    pub fn insert<'key>(&mut self, key: &[&'key str], value: V) -> Option<V> {
        self.insert_recursive(key, value)
    }

    fn insert_recursive<'key>(&mut self, key: &[&'key str], value: V) -> Option<V> {
        if let Some(fragment) = key.first() {
            match self.children.get_mut(*fragment) {
                Some(child) => child.insert_recursive(&key[1..], value),
                None => {
                    let mut new_child = PathTrie::new();
                    let result = new_child.insert_recursive(&key[1..], value);
                    self.children.insert(fragment.to_string(), new_child);
                    result
                }
            }
        } else {
            let mut result = Some(value);
            std::mem::swap(&mut result, &mut self.value);
            result
        }
    }

    pub fn insert_owned<I>(&mut self, key: I, value: V) -> Option<V>
    where
        I: IntoIterator<Item = String>,
    {
        let key_node = key.into_iter().fold(self, |current_node, fragment| {
            current_node
                .children
                .entry(fragment)
                .or_insert_with(Self::new)
        });

        std::mem::replace(&mut key_node.value, Some(value))
    }

    /// Get the node at `key`
    pub fn get_node<K>(&self, key: &[K]) -> Option<&Self>
    where
        String: Borrow<K>,
        K: Hash + Eq,
    {
        let mut current_node = self;

        for fragment in key {
            match current_node.children.get(fragment) {
                Some(node) => current_node = node,
                None => return None,
            }
        }

        Some(current_node)
    }

    /// Get the node at `key`
    ///
    /// This is a slightly silly specialisation, because
    /// String does not implement Borrow<&str>
    #[allow(dead_code)] // used for testing
    pub fn get_node_static(&self, key: &[&str]) -> Option<&Self> {
        let mut current_node = self;

        for fragment in key {
            match current_node.children.get(*fragment) {
                Some(node) => current_node = node,
                None => return None,
            }
        }

        Some(current_node)
    }

    /// Get the value at `key`
    pub fn get<K>(&self, key: &[K]) -> Option<&V>
    where
        String: Borrow<K>,
        K: Hash + Eq,
    {
        self.get_node(key).and_then(|node| node.value.as_ref())
    }

    /// Get the value at `key`
    ///
    /// This is a slightly silly specialisation, because
    /// String does not implement Borrow<&str>
    #[allow(dead_code)] // used for testing
    pub fn get_static(&self, key: &[&str]) -> Option<&V> {
        self.get_node_static(key)
            .and_then(|node| node.value.as_ref())
    }

    /// Removes the node corresponding to the given key.
    ///
    /// This operation is like the reverse of `insert` in that
    /// it also deletes extraneous nodes on the path from the root.
    ///
    /// If the key node has children, its value is set to `None` and no further
    /// action is taken. If the key node is a leaf, then it and its ancestors with
    /// empty values and no other children are deleted. Deletion proceeds up the tree
    /// from the key node until a node with a non-empty value or children is reached.
    ///
    /// If the key doesn't match a node in the Trie, no action is taken.
    pub fn remove<K>(&mut self, key: &[K]) -> Option<V>
    where
        String: Borrow<K>,
        K: Hash + Eq,
    {
        let mut result: Option<V> = None;
        self.remove_recursive(key, &mut result);
        result
    }

    fn remove_recursive<K>(&mut self, key: &[K], result: &mut Option<V>) -> bool
    where
        String: Borrow<K>,
        K: Hash + Eq,
    {
        if let Some(fragment) = key.first() {
            let delete_child = match self.children.get_mut(fragment) {
                Some(child) => child.remove_recursive(&key[1..], result),
                None => false,
            };
            if delete_child {
                self.children.remove(fragment.borrow());
            }
        } else {
            std::mem::swap(result, &mut self.value);
        }

        // If the node is childless and valueless, mark it for deletion.
        self.is_empty()
    }

    /// Removes all values that do not meet `condition` from their nodes
    ///
    /// Makes sure to clean up the tree so no empty nodes are left behind.
    ///
    /// Returns the total number of values removed.
    pub fn retain<F>(&mut self, condition: F) -> usize
    where
        F: Fn(&V) -> bool,
    {
        let mut count = 0;
        self.retain_recursive(&condition, &mut count); // don't remove root
        count
    }

    fn retain_recursive<F>(&mut self, condition: &F, count: &mut usize) -> bool
    where
        F: Fn(&V) -> bool,
    {
        // if let Some(fragment) = key.first() {
        //     let delete_child = match self.children.get_mut(fragment.borrow()) {
        //         Some(child) => child.remove_recursive(&key[1..], result),
        //         None => false,
        //     };
        //     if delete_child {
        //         self.children.remove(fragment.borrow());
        //     }
        // } else {
        //     std::mem::swap(result, &mut self.value);
        // }
        if let Some(ref v) = self.value {
            if !(*condition)(v) {
                self.value = None;
                *count += 1;
            }
        }
        self.children.retain(|_, child| {
            let delete = child.retain_recursive(condition, count);
            !delete
        });

        // If the node is childless and valueless, mark it for deletion.
        self.is_empty()
    }

    /// Checks if this node is empty
    ///
    /// A node is considered empty when it has no value and no children.
    pub fn is_empty(&self) -> bool {
        self.is_leaf() && self.value.is_none()
    }

    /// Checks if this node has no descendants
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    pub fn values(&self) -> Values<V> {
        Values {
            root: self,
            root_visited: false,
            stack: Vec::new(),
        }
    }
}

/// Iterator over the values of a [PathTrie](PathTrie)
pub struct Values<'a, V: 'a> {
    root: &'a PathTrie<V>,
    root_visited: bool,
    stack: Vec<StackItem<'a, V>>,
}
/// Information stored on the iteration stack whilst exploring
struct StackItem<'a, V: 'a> {
    child_iter: hash_map::Iter<'a, String, PathTrie<V>>,
}

impl<'a, V> Iterator for Values<'a, V> {
    type Item = &'a V;

    fn next(&mut self) -> Option<Self::Item> {
        // Special handling for the root.
        if !self.root_visited {
            self.root_visited = true;
            self.stack.push(StackItem {
                child_iter: self.root.children.iter(),
            });
            if let Some(ref root_val) = self.root.value {
                return Some(root_val);
            }
        }

        loop {
            match self.stack.last_mut() {
                Some(stack_top) => match stack_top.child_iter.next() {
                    Some((_, child_node)) => {
                        self.stack.push(StackItem {
                            child_iter: child_node.children.iter(),
                        });
                        if let Some(ref value) = child_node.value {
                            return Some(value);
                        }
                    }
                    None => {
                        self.stack.pop();
                    }
                },
                None => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_PATH: [String; 0] = [];

    #[test]
    fn test_empty_get() {
        let empty: PathTrie<()> = PathTrie::new();
        assert!(empty.get_node(&EMPTY_PATH).is_some());
        assert!(empty.get_node_static(&["test"]).is_none());
        assert!(empty.get_node(&["test".to_string()]).is_none());
    }

    #[test]
    fn test_insert_get() {
        let mut trie: PathTrie<usize> = PathTrie::new();
        assert!(trie.get_node(&EMPTY_PATH).is_some());
        assert!(trie.get_node_static(&["test"]).is_none());
        assert!(trie.get_node(&["test".to_string()]).is_none());

        assert!(trie.insert_owned(vec!["test".to_string()], 1).is_none());
        assert_eq!(Some(&1), trie.get_static(&["test"]));
        assert_eq!(Some(&1), trie.get(&["test".to_string()]));

        assert!(trie.insert(&["test", "me"], 2).is_none());
        assert_eq!(Some(&1), trie.get_static(&["test"]));
        assert_eq!(Some(&1), trie.get(&["test".to_string()]));
        assert_eq!(Some(&2), trie.get_static(&["test", "me"]));
        assert_eq!(Some(&2), trie.get(&["test".to_string(), "me".to_string()]));
    }

    #[test]
    fn test_remove() {
        let mut trie: PathTrie<usize> = PathTrie::new();

        assert!(trie.insert(&[], 0).is_none());
        assert!(trie.insert(&["test"], 1).is_none());
        assert!(trie.insert(&["test", "me"], 2).is_none());
        assert!(trie.insert(&["test", "you"], 3).is_none());
        assert!(trie.insert(&["test", "me", "further"], 4).is_none());
        assert!(trie.insert(&["test", "you", "not"], 5).is_none());

        assert_eq!(Some(1), trie.remove(&["test".to_string()]));
        assert_eq!(None, trie.remove(&["test".to_string()]));
        assert_eq!(
            Some(2),
            trie.remove(&["test".to_string(), "me".to_string()])
        );
        assert_eq!(None, trie.remove(&["test".to_string(), "me".to_string()]));
        assert_eq!(2, trie.retain(|v| *v < 4));
        assert_eq!(None, trie.get_static(&["test", "me", "further"]));
        assert_eq!(None, trie.get_static(&["test", "you", "not"]));
        assert_eq!(Some(&3), trie.get_static(&["test", "you"]));
    }

    #[test]
    fn test_iter() {
        let mut trie: PathTrie<usize> = PathTrie::new();

        assert!(trie.insert(&[], 0).is_none());
        assert!(trie.insert(&["test"], 1).is_none());
        assert!(trie.insert(&["test", "me"], 2).is_none());
        assert!(trie.insert(&["test", "you"], 3).is_none());
        assert!(trie.insert(&["test", "me", "further"], 4).is_none());
        assert!(trie.insert(&["test", "you", "not"], 5).is_none());

        let values: Vec<usize> = trie.values().copied().collect();
        for i in 0..=5 {
            assert!(values.contains(&i));
        }
    }
}
