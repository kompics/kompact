use crate::net::ConnectionState;
use futures::sync;
use crate::net::frames::*;
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
};
use crate::messaging::SerializedFrame;

/// Wrapper around a hashmap of frame queues.
///
/// Used when waiting for connections to establish and drained when possible.
pub struct QueueManager {
    inner: HashMap<SocketAddr, VecDeque<SerializedFrame>>,
}

impl QueueManager {
    pub fn new() -> Self {
        QueueManager {
            inner: HashMap::new(),
        }
    }

    pub fn stop(self) -> () {
        drop(self); // doesn't need any cleanup, yet
    }

    /// Appends the given frame onto the SocketAddr's queue
    pub fn enqueue_frame(&mut self, frame: SerializedFrame, dst: SocketAddr) {
        self.inner
            .entry(dst)
            .or_insert(VecDeque::new())
            .push_back(frame);
    }

    /// Extracts the next queue-up frame for the SocketAddr, if one exists
    ///
    /// If the SocketAddr exists but its queue is empty, the entry is removed.
    pub fn pop_frame(&mut self, dst: &SocketAddr) -> Option<SerializedFrame> {
        let res = self.inner.get_mut(dst).and_then(|q| q.pop_back());
        if self.inner.contains_key(dst) && res.is_none() {
            self.inner.remove(dst);
        }
        res
    }

    /// Attempts to drain all SerializedFrame entries stored for the provided SocketAddr into the Sender
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
    // }

    pub fn has_frame(&self, dst: &SocketAddr) -> bool {
        self.inner.get(dst).map_or(false, |q| !q.is_empty())
    }
}
