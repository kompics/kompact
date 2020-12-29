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

    /*
    The queuemanager is really just a struct, no need to stop it?
    pub fn stop(self) -> () {
        drop(self); // doesn't need any cleanup, yet
    }
    */
    /// Appends the given frame onto the SocketAddr's queue
    pub fn enqueue_data(&mut self, data: DispatchData, dst: SocketAddr) {
        self.inner
            .entry(dst)
            .or_insert_with(VecDeque::new)
            .push_front(data);
    }

    /// Appends the given frame onto the SocketAddr's queue
    pub fn enqueue_priority_data(&mut self, data: DispatchData, dst: SocketAddr) {
        self.priority_queue
            .entry(dst)
            .or_insert_with(VecDeque::new)
            .push_front(data);
    }

    /// Extracts the next queue-up frame for the SocketAddr, if one exists
    ///
    /// If the SocketAddr exists but its queue is empty, the entry is removed.
    pub fn pop_data(&mut self, dst: &SocketAddr) -> Option<DispatchData> {
        let mut res = self.priority_queue.get_mut(dst).and_then(|q| q.pop_back());
        if self.priority_queue.contains_key(dst) && res.is_none() {
            self.priority_queue.remove(dst);
        }
        if res.is_none() {
            res = self.inner.get_mut(dst).and_then(|q| q.pop_back());
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

    /*
    This is turned off for the moment, since it's using pop_frame it's already adapted to the priority queue
    /// Attempts to drain all SerializedFrame entries stored for the provided SocketAddr into the Sender
    /// Currently unused, will be brought back into play soon...
    #[allow(dead_code)]
    pub fn try_drain(
        &mut self,
        dst: SocketAddr,
        tx: &mut sync::mpsc::UnboundedSender<SerializedFrame>,
    ) -> Option<ConnectionState> {
        while let Some(frame) = self.pop_frame(&dst) {
            if let Err(err) = tx.unbounded_send(frame) {
                let next = Some(ConnectionState::Closed);

                // Consume error and retrieve failed SerializedFrame
                let frame = err.into_inner();
                self.enqueue_frame(frame, dst);

                // End looping, return next state
                return next;
            }
        }
        None
    }
    // pub fn exists(&self, dst: &SocketAddr) -> bool {
    //     self.inner.contains_key(dst)
    */
    // }

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
