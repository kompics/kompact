use super::*;

use crate::{messaging::RegistrationResult, timer::timer_manager::CanCancelTimers};

/// The [SystemHandle](SystemHandle) provided by a [ComponentContext](ComponentContext)
pub struct ContextSystemHandle {
    component: Arc<dyn CoreContainer>,
}

impl ContextSystemHandle {
    pub(super) fn from(component: Arc<dyn CoreContainer>) -> Self {
        ContextSystemHandle { component }
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

    fn register(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> KFuture<RegistrationResult> {
        self.component.system().register(c)
    }

    fn create_and_register<C, F>(&self, f: F) -> (Arc<Component<C>>, KFuture<RegistrationResult>)
    where
        F: FnOnce() -> C,
        C: ComponentDefinition + 'static,
    {
        self.component.system().create_and_register(f)
    }

    fn register_by_alias<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
    ) -> KFuture<RegistrationResult>
    where
        A: Into<String>,
    {
        self.component.system().register_by_alias(c, alias)
    }

    fn update_alias_registration<A>(
        &self,
        c: &Arc<impl AbstractComponent + ?Sized>,
        alias: A,
    ) -> KFuture<RegistrationResult>
    where
        A: Into<String>,
    {
        self.component.system().update_alias_registration(c, alias)
    }

    fn start(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> () {
        self.component.system().start(c)
    }

    fn start_notify(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> KFuture<()> {
        self.component.system().start_notify(c)
    }

    fn stop(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> () {
        self.component.system().stop(c)
    }

    fn stop_notify(&self, c: &Arc<impl AbstractComponent + ?Sized>) -> KFuture<()> {
        self.component.system().stop_notify(c)
    }

    fn kill(&self, c: Arc<impl AbstractComponent + ?Sized>) -> () {
        self.component.system().kill(c)
    }

    fn kill_notify(&self, c: Arc<impl AbstractComponent + ?Sized>) -> KFuture<()> {
        self.component.system().kill_notify(c)
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

    fn spawn<R: Send + 'static>(
        &self,
        future: impl futures::Future<Output = R> + 'static + Send,
    ) -> JoinHandle<R> {
        self.component.system().spawn(future)
    }
}

impl Dispatching for ContextSystemHandle {
    fn dispatcher_ref(&self) -> DispatcherRef {
        self.component.system().dispatcher_ref()
    }
}

impl CanCancelTimers for ContextSystemHandle {
    fn cancel_timer(&self, handle: ScheduledTimer) {
        self.component.system().cancel_timer(handle);
    }
}
