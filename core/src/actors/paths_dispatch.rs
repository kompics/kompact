use super::*;
use crate::{
    messaging::{DispatchData, DispatchEnvelope, MsgEnvelope, NetMessage, SerialisedFrame},
    net::buffers::ChunkRef,
};

/// A factory trait for things that have associated [actor paths](ActorPath).
pub trait ActorPathFactory {
    /// Returns the associated actor path.
    fn actor_path(&self) -> ActorPath;
}

/// A temporary combination of an [ActorPath](ActorPath)
/// and something that can [dispatch](Dispatching) stuff.
///
/// This can be used when you want to use a different actor path
/// than the one of the current component for a [tell](ActorPath::tell)
/// but still need to use the component's dispatcher for the message.
/// This is useful for forwarding components, for example, when trying to
/// preserve the original sender.
///
/// See also [using_dispatcher](ActorPath::using_dispatcher).
pub struct DispatchingPath<'a, 'b> {
    path: &'a ActorPath,
    ctx: &'b dyn Dispatching,
}

impl Dispatching for DispatchingPath<'_, '_> {
    fn dispatcher_ref(&self) -> DispatcherRef {
        self.ctx.dispatcher_ref()
    }
}

impl ActorPathFactory for DispatchingPath<'_, '_> {
    fn actor_path(&self) -> ActorPath {
        self.path.clone()
    }
}

impl ActorPath {
    /// Send message `m` to the actor designated by this path.
    ///
    /// The `from` field is used as a source,
    /// and the [`ActorPath`] it resolved to will be supplied at the destination.
    pub fn tell<S, B>(&self, m: B, from: &S) -> ()
    where
        S: ActorPathFactory + Dispatching,
        B: Into<Box<dyn Serialisable>>,
    {
        let mut src = from.actor_path();
        src.set_protocol(self.protocol());
        self.tell_with_sender(m, from, src)
    }

    /// Send message `m` to the actor designated by this path with an explicit sender.
    pub fn tell_with_sender<B, D>(&self, m: B, dispatch: &D, from: ActorPath) -> ()
    where
        B: Into<Box<dyn Serialisable>>,
        D: Dispatching,
    {
        let msg: Box<dyn Serialisable> = m.into();
        let dst = self.clone();
        let env = DispatchEnvelope::Msg {
            src: from.clone(),
            dst: dst.clone(),
            msg: DispatchData::Lazy(msg, from, dst),
        };
        dispatch.dispatcher_ref().enqueue(MsgEnvelope::Typed(env))
    }

    /// Send message `m` to the actor designated by this path with eager serialisation.
    pub fn tell_serialised<CD, B>(&self, m: B, from: &CD) -> Result<(), SerError>
    where
        CD: ComponentTraits + ComponentLifecycle + ActorPathFactory,
        B: Serialisable + 'static,
    {
        let mut src = from.actor_path();
        src.set_protocol(self.protocol());
        self.tell_serialised_with_sender(m, from, src)
    }

    /// Send message `m` to the actor designated by this path with eager serialisation and an explicit sender.
    pub fn tell_serialised_with_sender<CD, B>(
        &self,
        m: B,
        dispatch: &CD,
        from: ActorPath,
    ) -> Result<(), SerError>
    where
        CD: ComponentTraits + ComponentLifecycle,
        B: Serialisable + 'static,
    {
        if self.protocol() == Transport::Local {
            self.tell_with_sender(m, dispatch, from);
            Ok(())
        } else {
            dispatch.ctx().with_buffer(|buffer| {
                let mut buf = buffer.get_buffer_encoder()?;
                let msg =
                    crate::serialisation::ser_helpers::serialise_msg(&from, self, &m, &mut buf)?;
                let env = DispatchEnvelope::Msg {
                    src: from,
                    dst: self.clone(),
                    msg: DispatchData::Serialised(SerialisedFrame::ChunkLease(msg)),
                };
                dispatch.dispatcher_ref().enqueue(MsgEnvelope::Typed(env));
                Ok(())
            })
        }
    }

    /// Send prepared message `content` to the actor designated by this path.
    pub fn tell_preserialised<CD>(&self, content: ChunkRef, from: &CD) -> Result<(), SerError>
    where
        CD: ComponentTraits + ComponentLifecycle + ActorPathFactory,
    {
        let mut src = from.actor_path();
        src.set_protocol(self.protocol());
        self.tell_preserialised_with_sender(content, from, src)
    }

    /// Send preserialised message `content` to the actor designated by this path with an explicit sender.
    pub fn tell_preserialised_with_sender<CD: ComponentTraits + ComponentLifecycle>(
        &self,
        content: ChunkRef,
        dispatch: &CD,
        from: ActorPath,
    ) -> Result<(), SerError> {
        dispatch.ctx().with_buffer(|buffer| {
            let mut buf = buffer.get_buffer_encoder()?;
            let msg = crate::serialisation::ser_helpers::serialise_msg_with_preserialised(
                &from, self, content, &mut buf,
            )?;
            let env = DispatchEnvelope::Msg {
                src: from,
                dst: self.clone(),
                msg: DispatchData::Serialised(SerialisedFrame::ChunkRef(msg)),
            };
            dispatch.dispatcher_ref().enqueue(MsgEnvelope::Typed(env));
            Ok(())
        })
    }

    /// Forwards the still serialised message to this path without changing the sender.
    pub fn forward_with_original_sender<D>(
        &self,
        mut serialised_message: NetMessage,
        dispatcher: &D,
    ) -> ()
    where
        D: Dispatching,
    {
        serialised_message.receiver = self.clone();
        let env = DispatchEnvelope::ForwardedMsg {
            msg: serialised_message,
        };
        dispatcher.dispatcher_ref().enqueue(MsgEnvelope::Typed(env));
    }

    /// Forwards the still serialised message to this path replacing the sender with the given one.
    pub fn forward_with_sender<D>(
        &self,
        mut serialised_message: NetMessage,
        dispatch: &D,
        from: ActorPath,
    ) -> ()
    where
        D: Dispatching,
    {
        serialised_message.receiver = self.clone();
        serialised_message.sender = from;
        let env = DispatchEnvelope::ForwardedMsg {
            msg: serialised_message,
        };
        dispatch.dispatcher_ref().enqueue(MsgEnvelope::Typed(env));
    }

    /// Returns a temporary combination of an [ActorPath](ActorPath)
    /// and something that can [dispatch](Dispatching) stuff.
    pub fn using_dispatcher<'a, 'b>(
        &'a self,
        disp: &'b dyn Dispatching,
    ) -> DispatchingPath<'a, 'b> {
        DispatchingPath {
            path: self,
            ctx: disp,
        }
    }
}
