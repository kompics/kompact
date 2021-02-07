// only build this module on nightly
#![cfg(nightly)]

pub struct ByteSliceMap<V> {
    inner: MaybeTree<V>,
}

impl<V> ByteSliceMap<V> {
    #[inline]
    pub fn new() -> ByteSliceMap<V> {
        ByteSliceMap {
            inner: MaybeTree::Empty,
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.inner = MaybeTree::Empty;
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    #[inline]
    pub fn get(&self, key: &[u8]) -> Option<&V> {
        self.inner.get(key)
    }

    #[inline]
    pub fn insert(&mut self, key: &[u8], value: V) -> Option<V> {
        self.inner.insert(key, value)
    }
}

impl<V> Default for ByteSliceMap<V> {
    fn default() -> Self {
        Self::new()
    }
}

enum MaybeTree<V> {
    Empty,
    Tree(Box<RadixTree<V>>),
}

impl<V> MaybeTree<V> {
    fn from_tree(tree: RadixTree<V>) -> MaybeTree<V> {
        MaybeTree::Tree(Box::new(tree))
    }

    fn is_empty(&self) -> bool {
        matches!(self, MaybeTree::Empty)
    }

    fn get(&self, key: &[u8]) -> Option<&V> {
        match self {
            MaybeTree::Empty => None,
            MaybeTree::Tree(tree) => tree.get(key),
        }
    }

    fn insert(&mut self, key: &[u8], value: V) -> Option<V> {
        match self {
            MaybeTree::Empty => {
                let node = RadixTree::value(key, value);
                let mut tree = MaybeTree::Tree(Box::new(node));
                std::mem::swap(self, &mut tree);
                None
            }
            MaybeTree::Tree(tree) => tree.insert(key, value),
        }
    }
}

impl<V> Default for MaybeTree<V> {
    fn default() -> Self {
        MaybeTree::Empty
    }
}

enum RadixTree<V> {
    Value { key_suffix: Box<[u8]>, value: V },
    Node { children: Box<[MaybeTree<V>; 256]> },
}

impl<V> RadixTree<V> {
    fn value(key: &[u8], value: V) -> RadixTree<V> {
        let key_suffix: Box<[u8]> = key.to_vec().into_boxed_slice();
        RadixTree::Value { key_suffix, value }
    }

    fn empty_children() -> RadixTree<V> {
        let mut children: Vec<MaybeTree<V>> = Vec::with_capacity(256);
        for _ in 0..256 {
            children.push(MaybeTree::Empty);
        }
        debug_assert_eq!(256, children.len());
        let boxed = children.into_boxed_slice();
        let fixed = unsafe { Box::from_raw(Box::into_raw(boxed) as *mut [MaybeTree<V>; 256]) };
        RadixTree::Node { children: fixed }
    }

    fn get(&self, key: &[u8]) -> Option<&V> {
        match self {
            RadixTree::Value {
                ref key_suffix,
                ref value,
            } => {
                if key == key_suffix.as_ref() {
                    Some(value)
                } else {
                    None
                }
            }
            RadixTree::Node { ref children } => match key.get(0) {
                Some(key_pos) => {
                    let pos = (*key_pos) as usize;
                    unsafe { children.get_unchecked(pos).get(&key[1..]) }
                }
                None => None,
            },
        }
    }

    #[allow(unused_assignments)]
    fn insert(&mut self, key: &[u8], value: V) -> Option<V> {
        let mut must_grow_pos: Option<usize> = None;
        match self {
            RadixTree::Value {
                ref mut key_suffix,
                value: ref mut current_value,
            } => {
                if key == key_suffix.as_ref() {
                    let mut temp_value = value;
                    std::mem::swap(current_value, &mut temp_value);
                    return Some(temp_value);
                } else {
                    let mut key_vec = key_suffix.to_vec();
                    if key_vec.is_empty() {
                        unimplemented!("What to do with empty suffixes?");
                    } else {
                        let i = key_vec.remove(0);
                        let pos = i as usize;
                        *key_suffix = key_vec.into_boxed_slice();
                        must_grow_pos = Some(pos);
                        //println!("Must grow tree at {}", pos);
                    }
                }
            }
            RadixTree::Node { ref mut children } => match key.get(0) {
                Some(key_pos) => {
                    let pos = (*key_pos) as usize;
                    unsafe {
                        children.get_unchecked_mut(pos).insert(&key[1..], value);
                    }
                    return None;
                }
                None => {
                    unimplemented!("Key already ended.");
                }
            },
        }

        match must_grow_pos {
            Some(pos) => {
                //println!("Growing tree at {}", pos);
                let mut node = RadixTree::empty_children();
                std::mem::swap(self, &mut node);
                let new_pos = match key.get(0) {
                    Some(i) => (*i) as usize,
                    None => {
                        unimplemented!("Key already ended.");
                    }
                };
                if let RadixTree::Node { ref mut children } = self {
                    if new_pos == pos {
                        node.insert(&key[1..], value);
                    } else {
                        let new_node = RadixTree::value(&key[1..], value);
                        let new_tree = MaybeTree::from_tree(new_node);
                        unsafe {
                            let target = children.get_unchecked_mut(new_pos);
                            *target = new_tree;
                        }
                    }
                    let node_tree = MaybeTree::from_tree(node);
                    unsafe {
                        let target = children.get_unchecked_mut(pos);
                        *target = node_tree;
                    }
                } else {
                    //#[cfg(debug_assertions)]
                    unimplemented!("Invalid tree composition?!?!?");
                    // #[cfg(not(debug_assertions))]
                    // () // ignore
                }
                None
            }
            None => None,
        }
    }
}

// fn slice_compare(left: &[u8], right: &[u8], offset: usize) -> bool {
// 	&left[offset..] == right
//     // let llen = left.len();
//     // let rlen = right.len();
//     // if (llen - offset) != rlen {
//     //     false
//     // } else {
//     //     let mut li = offset;
//     //     let mut ri = 0usize;
//     //     while li < llen {
//     //         if left[li] == right[ri] {
//     //             li += 1;
//     //             ri += 1;
//     //         } else {
//     //             return false;
//     //         }
//     //     }
//     //     true
//     // }
// }

#[cfg(test)]
mod tests {
    use super::*;

    //const TEST_DATA: [(u64, &'static str); 8] =
    const TEST_SIZE: usize = 100;

    fn int_to_key(i: usize) -> [u8; 8] {
        let key: [u8; 8] = unsafe { std::mem::transmute(i * 100) };
        key
    }

    fn key_to_value(key: [u8; 8]) -> String {
        let i: usize = unsafe { std::mem::transmute(key) };
        format!("{}", i)
    }

    // #[test]
    // fn compare_slices() {
    //     let left = [1u8, 2u8, 3u8, 4u8];
    //     let right = left;
    //     assert!(slice_compare(&left, &right, 0));
    //     let right2 = [3u8, 4u8];
    //     assert!(slice_compare(&left, &right2, 2));
    //     assert!(!slice_compare(&left, &right2, 0));
    //     assert!(!slice_compare(&left, &right2, 3));
    //     assert!(!slice_compare(&left, &right2, 4));
    //     let right3 = [1u8, 2u8, 3u8, 4u8, 5u8];
    //     assert!(!slice_compare(&left, &right3, 0));
    // }

    #[test]
    fn gaps() {
        let mut map: ByteSliceMap<String> = ByteSliceMap::new();
        let k1 = [44, 1, 0, 0, 0, 0, 0, 0];
        let v1 = key_to_value(k1);
        assert!(map.insert(&k1, v1.clone()).is_none());
        assert_eq!(Some(&v1), map.get(&k1));

        let k2 = [44, 26, 0, 0, 0, 0, 0, 0];
        let v2 = key_to_value(k2);
        assert!(map.insert(&k2, v2.clone()).is_none());
        assert_eq!(Some(&v2), map.get(&k2));

        assert_eq!(Some(&v1), map.get(&k1));
    }

    #[test]
    fn insert_and_read() {
        let mut map: ByteSliceMap<String> = ByteSliceMap::new();
        for i in 0..TEST_SIZE {
            let key = int_to_key(i);
            let value: String = format!("{}", i);
            let res = map.insert(&key, value);
            println!("Wrote {:?}", key);
            assert!(res.is_none(), "Could not add value for key={}", i);
        }
        for i in 0..TEST_SIZE {
            let key = int_to_key(i);
            let value: String = format!("{}", i);
            let res = map.get(&key);
            println!("Read {:?} -> {:?}", key, res);
            assert!(res.is_some(), "Could not find value for key={}", i);
            assert_eq!(&value, res.unwrap());
        }
    }
}
