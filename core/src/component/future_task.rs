use super::*;

use futures::{
    future::BoxFuture,
    task::{waker_ref, ArcWake},
};
use std::{
    fmt,
    future::Future as RustFuture,
    ops::{Deref, DerefMut},
    task::{Context, Poll},
};

impl<CD> ArcWake for Component<CD>
where
    CD: ComponentDefinition + 'static,
{
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.schedule()
    }
}

//pub type BlockingFuture = BoxFuture<'static, ()>;

pub(super) enum BlockingRunResult {
    BlockOn(BlockingFuture),
    Unblock,
}

/// A future that is supposed to be blocking the
/// component's execution until completed
pub struct BlockingFuture {
    future: BoxFuture<'static, ()>,
}
impl fmt::Debug for BlockingFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockingFuture")
            .field("future", &"<anonymous boxed future>")
            .finish()
    }
}

impl BlockingFuture {
    pub(super) fn run<CD>(mut self, component: &Arc<Component<CD>>) -> BlockingRunResult
    where
        CD: ComponentDefinition + 'static,
    {
        let waker = waker_ref(component);
        let ctx = &mut Context::from_waker(&waker);
        let fut = self.future.as_mut();
        let res = RustFuture::poll(fut, ctx);
        match res {
            Poll::Pending => BlockingRunResult::BlockOn(self),
            Poll::Ready(_) => BlockingRunResult::Unblock,
        }
    }
}

pub(super) fn blocking<F, CD>(
    component: &mut CD,
    f: impl FnOnce(ComponentDefinitionAccess<CD>) -> F,
) -> BlockingFuture
where
    F: RustFuture + Send + 'static,
    CD: ComponentDefinition + 'static,
{
    let future = f(ComponentDefinitionAccess::from_ref(component));
    let ignore_result = async move {
        let _ = future.await;
    };
    let boxed = Box::pin(ignore_result);
    BlockingFuture { future: boxed }
}

pub struct ComponentDefinitionAccess<CD>
where
    CD: ComponentDefinition + 'static,
{
    raw: *mut CD,
}
impl<CD> ComponentDefinitionAccess<CD>
where
    CD: ComponentDefinition + 'static,
{
    fn from_ref(definition: &mut CD) -> Self {
        ComponentDefinitionAccess {
            raw: definition as *mut CD,
        }
    }
}
impl<CD> Deref for ComponentDefinitionAccess<CD>
where
    CD: ComponentDefinition + 'static,
{
    type Target = CD;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.raw } // poll will never be called from a context where a reference to CD would not be available
    }
}
impl<CD> DerefMut for ComponentDefinitionAccess<CD>
where
    CD: ComponentDefinition + 'static,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.raw } // poll will never be called from a context where a mutable reference to CD would not be available
    }
}

unsafe impl<CD> Send for ComponentDefinitionAccess<CD> where CD: ComponentDefinition + 'static {} // will only be sent WITH the component, which is safe
