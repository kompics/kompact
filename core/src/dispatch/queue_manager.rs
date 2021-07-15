use crate::messaging::dispatch::DispatchData;
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
};

/// Wrapper around a hashmap of frame queues.
///
/// Used when waiting for connections to establish and drained when possible.
/// `priority_queue` allows the NetworkDispatcher to maintain FIFO Order in the event of shaky connections
pub struct QueueManager {
    inner: HashMap<SocketAddr, VecDeque<DispatchData>>,
    priority_queue: HashMap<SocketAddr, VecDeque<DispatchData>>,
}

impl QueueManager {
    pub fn new() -> Self {
        QueueManager {
            inner: HashMap::new(),
            priority_queue: HashMap::new(),
        }
    }

    /// Appends the given frame onto the SocketAddr's queue
    pub fn enqueue_data(&mut self, data: DispatchData, dst: SocketAddr) {
        self.inner
            .entry(dst)
            .or_insert_with(VecDeque::new)
            .push_back(data);
    }

    /// Appends the given frame onto the SocketAddr's queue
    pub fn enqueue_priority_data(&mut self, data: DispatchData, dst: SocketAddr) {
        self.priority_queue
            .entry(dst)
            .or_insert_with(VecDeque::new)
            .push_back(data);
    }

    /// Extracts the next queue-up frame for the SocketAddr, if one exists
    ///
    /// If the SocketAddr exists but its queue is empty, the entry is removed.
    pub fn pop_data(&mut self, dst: &SocketAddr) -> Option<DispatchData> {
        let mut res = self.priority_queue.get_mut(dst).and_then(|q| q.pop_front());
        if self.priority_queue.contains_key(dst) && res.is_none() {
            self.priority_queue.remove(dst);
        }
        if res.is_none() {
            res = self.inner.get_mut(dst).and_then(|q| q.pop_front());
            if self.inner.contains_key(dst) && res.is_none() {
                self.inner.remove(dst);
            }
        }
        res
    }

    pub fn drop_queue(&mut self, addr: &SocketAddr) {
        self.priority_queue.remove(addr);
        self.inner.remove(addr);
    }

    pub fn has_data(&self, dst: &SocketAddr) -> bool {
        if self
            .priority_queue
            .get(dst)
            .map_or(false, |q| !q.is_empty())
        {
            return true;
        }
        self.inner.get(dst).map_or(false, |q| !q.is_empty())
    }
}
