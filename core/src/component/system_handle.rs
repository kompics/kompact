use super::*;

pub(super) struct ContextSystemHandle {
    component: Arc<dyn CoreContainer>,
}

impl ContextSystemHandle {
    pub(super) fn from(component: Arc<dyn CoreContainer>) -> Self {
        ContextSystemHandle { component }
    }

    fn send_registration(&self, env: RegistrationEnvelope) -> () {
        let envelope = MsgEnvelope::Typed(DispatchEnvelope::Registration(env));
        self.component.system().dispatcher_ref().enqueue(envelope);
    }
}

impl SystemHandle for ContextSystemHandle {
    fn create<C, F>(&self, f: F) -> Arc<Component<C>>
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        self.component.system().create(f)
    }

    #[cfg(all(nightly, feature = "type_erasure"))]
    fn create_erased<M: MessageBounds>(
        &self,
        a: Box<dyn CreateErased<M>>,
    ) -> Arc<dyn AbstractComponent<Message = M>> {
        self.component.system().create_erased(a)
    }

    fn register(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        reply_to: &dyn Receiver<RegistrationResponse>,
    ) -> RegistrationId {
        let id = RegistrationId(Uuid::new_v4());
        let recipient = reply_to.recipient();
        let env = RegistrationEnvelope::with_recipient(
            c.as_ref(),
            PathResolvable::ActorId(*c.id()),
            false,
            id,
            recipient,
        );
        self.send_registration(env);
        id
    }

    fn register_without_response(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> () {
        let env = RegistrationEnvelope::basic(c.as_ref(), PathResolvable::ActorId(*c.id()), false);
        self.send_registration(env);
    }

    fn register_by_alias<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
        reply_to: &dyn Receiver<RegistrationResponse>,
    ) -> RegistrationId
    where
        A: Into<String>,
    {
        let id = RegistrationId(Uuid::new_v4());
        let recipient = reply_to.recipient();
        let env = RegistrationEnvelope::with_recipient(
            c.as_ref(),
            PathResolvable::Alias(alias.into()),
            false,
            id,
            recipient,
        );
        self.send_registration(env);
        id
    }

    fn register_by_alias_without_response<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
    ) -> ()
    where
        A: Into<String>,
    {
        let env =
            RegistrationEnvelope::basic(c.as_ref(), PathResolvable::Alias(alias.into()), false);
        self.send_registration(env);
    }

    fn update_alias_registration<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
        reply_to: &dyn Receiver<RegistrationResponse>,
    ) -> RegistrationId
    where
        A: Into<String>,
    {
        let id = RegistrationId(Uuid::new_v4());
        let recipient = reply_to.recipient();
        let env = RegistrationEnvelope::with_recipient(
            c.as_ref(),
            PathResolvable::Alias(alias.into()),
            true,
            id,
            recipient,
        );
        self.send_registration(env);
        id
    }

    fn update_alias_registration_without_response<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
    ) -> ()
    where
        A: Into<String>,
    {
        let env =
            RegistrationEnvelope::basic(c.as_ref(), PathResolvable::Alias(alias.into()), true);
        self.send_registration(env);
    }

    fn start(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> () {
        self.component.system().start(c)
    }

    fn stop(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> () {
        self.component.system().stop(c)
    }

    fn kill(&self, c: Arc<impl AbstractComponent + ?Sized>) -> () {
        self.component.system().kill(c)
    }

    fn throughput(&self) -> usize {
        self.component.system().throughput()
    }

    fn max_messages(&self) -> usize {
        self.component.system().max_messages()
    }

    fn shutdown_async(&self) -> () {
        self.component.system().shutdown_async()
    }

    fn system_path(&self) -> SystemPath {
        self.component.system().system_path()
    }

    fn deadletter_ref(&self) -> ActorRef<Never> {
        self.component.system().actor_ref()
    }
}

impl Dispatching for ContextSystemHandle {
    fn dispatcher_ref(&self) -> DispatcherRef {
        self.component.system().dispatcher_ref()
    }
}
