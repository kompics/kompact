use super::*;

use futures::{
    future::BoxFuture,
    task::{waker_ref, ArcWake},
};
use std::{
    fmt,
    future::Future as RustFuture,
    ops::{Deref, DerefMut},
    task::{Context, Poll, Waker},
};

impl<CD> ArcWake for Component<CD>
where
    CD: ComponentTraits + ComponentLifecycle,
{
    fn wake_by_ref(arc_self: &Arc<Self>) {
        println!("wake by ref");
        arc_self.schedule()
    }
}

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
        CD: ComponentTraits + ComponentLifecycle,
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

/// A future that is supposed to be polled
/// while the component is running normally
pub struct NonBlockingFuture {
    future: BoxFuture<'static, Handled>,
    waker: Waker,
    tag: Uuid,
}
impl fmt::Debug for NonBlockingFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NonBlockingFuture")
            .field("future", &"<anonymous boxed future>")
            .finish()
    }
}

impl NonBlockingFuture {
    pub(super) fn run(&mut self) -> Poll<Handled> {
        let ctx = &mut Context::from_waker(&self.waker);
        let fut = self.future.as_mut();
        RustFuture::poll(fut, ctx)
    }

    pub(super) fn tag(&self) -> Uuid {
        self.tag
    }

    pub(super) fn schedule(&self) -> () {
        self.waker.wake_by_ref()
    }
}

pub(super) fn non_blocking<F, CD>(
    component: &mut CD,
    f: impl FnOnce(ComponentDefinitionAccess<CD>) -> F,
) -> NonBlockingFuture
where
    F: RustFuture<Output = Handled> + Send + 'static,
    CD: ComponentDefinition + 'static,
{
    let id = Uuid::new_v4();
    let future = f(ComponentDefinitionAccess::from_ref(component));
    let core = component.ctx().component();
    let waker = task_waker::create(id, core);
    let boxed = Box::pin(future);
    NonBlockingFuture {
        future: boxed,
        waker,
        tag: id,
    }
}

/// Gives access to the component definition within a future/async function
///
/// This is *only* guaranteed to be correct when accessed as part of the future's `poll` function.
///
/// In particular, under no circumstances should you ever send this anywhere else
/// (e.g. on a channel, or as part of spawned off closure).
/// The only reason this is marked as `Send` is so the futures it produces can be sent
/// with their parent component.
///
/// This also means you may not hold references to fields within the component across `await` points.
/// You must either drop them manually before calling `await`, or simply use a sub-scope and let them go
/// out of scope first.
/// It is then safe to reacquire them following the `await`.
///
/// If the future is not being used as a blocking variant,
/// no assumptions about the state of the component should be made after the `await` point
/// and everthing previously established must be re-verified on the new references
/// if it could at all have changed between two `poll` invocations.
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

mod task_waker {
    use super::*;
    use std::{
        mem::{self, ManuallyDrop},
        task::{RawWaker, RawWakerVTable, Waker},
    };

    pub fn create(id: Uuid, core: Arc<dyn CoreContainer>) -> Waker {
        let task = TaskWaker { id, core };
        let raw = Arc::into_raw(Arc::new(task)) as *const ();
        let vtable = &TaskWaker::VTABLE;
        unsafe { Waker::from_raw(RawWaker::new(raw, vtable)) }
    }

    #[derive(Debug, Clone)]
    struct TaskWaker {
        id: Uuid,
        core: Arc<dyn CoreContainer>,
    }

    impl TaskWaker {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            Self::clone_waker,
            Self::wake,
            Self::wake_by_ref,
            Self::drop_waker,
        );

        #[allow(clippy::redundant_clone)]
        unsafe fn clone_waker(ptr: *const ()) -> RawWaker {
            let arc = ManuallyDrop::new(Arc::from_raw(ptr as *const TaskWaker));
            mem::forget(arc.clone());
            RawWaker::new(ptr, &Self::VTABLE)
        }

        unsafe fn wake(ptr: *const ()) {
            let arc = Arc::from_raw(ptr as *const TaskWaker);
            arc.core.enqueue_control(ControlEvent::Poll(arc.id));
        }

        unsafe fn wake_by_ref(ptr: *const ()) {
            let arc = ManuallyDrop::new(Arc::from_raw(ptr as *const TaskWaker));
            arc.core.enqueue_control(ControlEvent::Poll(arc.id));
        }

        unsafe fn drop_waker(ptr: *const ()) {
            drop(Arc::from_raw(ptr as *const TaskWaker));
        }
    }
}
