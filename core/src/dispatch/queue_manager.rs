use KompicsLogger;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use spnl::frames::Frame;

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
        debug!(self.log, "Enqueuing frame");
        let queue = self.inner.entry(dst).or_insert(VecDeque::new());
        queue.push_back(frame);
    }

    /// Extracts the next queue-up frame for the SocketAddr, if one exists
    pub fn dequeue_frame(&mut self, dst: &SocketAddr) -> Option<Frame> {
        debug!(self.log, "Dequeuing frame");
        self.inner.get_mut(dst).and_then(|q| q.pop_back())
    }
}
