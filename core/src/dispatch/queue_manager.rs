use spnl::frames::Frame;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use KompicsLogger;

/// Wrapper around a hashmap of frame queues.
///
/// Used when waiting for connections to establish and drained when possible.
pub struct QueueManager {
    log: KompicsLogger,
    inner: HashMap<SocketAddr, VecDeque<Frame>>,
}

impl QueueManager {
    pub fn new(log: KompicsLogger) -> Self {
        QueueManager {
            log,
            inner: HashMap::new(),
        }
    }

    /// Appends the given frame onto the SocketAddr's queue
    pub fn enqueue_frame(&mut self, frame: Frame, dst: SocketAddr) {
        self.inner.entry(dst).or_insert(VecDeque::new()).push_back(frame);
    }

    /// Extracts the next queue-up frame for the SocketAddr, if one exists
    ///
    /// If the SocketAddr exists but its queue is empty, the entry is removed.
    pub fn pop_frame(&mut self, dst: &SocketAddr) -> Option<Frame> {
        let res = self.inner.get_mut(dst).and_then(|q| q.pop_back());
        if self.inner.contains_key(dst) && res.is_none() {
            self.inner.remove(dst);
        }
        res
    }

    pub fn exists(&self, dst: &SocketAddr) ->  bool {
        self.inner.contains_key(dst)
    }

    pub fn has_frame(&self, dst: &SocketAddr) -> bool {
        self.inner.get(dst).map_or(false, |q| !q.is_empty())
    }

}
