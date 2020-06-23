use super::*;

use futures::{
    future::BoxFuture,
    task::{waker_ref, ArcWake},
};
use std::{
    fmt,
    future::Future as RustFuture,
    pin::Pin,
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

pub(super) struct BlockingFuture {
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

pub(super) fn blocking<'a, F, CD>(
    component: &'a mut CD,
    f: impl FnOnce(Pin<&'static mut CD>) -> F,
) -> BlockingFuture
where
    F: RustFuture + Send + 'static,
    CD: ComponentDefinition + 'static,
{
    let pinned_component = unsafe {
        let p = Pin::new_unchecked(component);
        std::mem::transmute::<Pin<&'a mut CD>, Pin<&'static mut CD>>(p) // this is a very bad idea, but it avoids using a raw pointer right now
    };
    let future = f(pinned_component);
    let ignore_result = async move {
        let _ = future.await;
    };
    let boxed = Box::pin(ignore_result);
    BlockingFuture { future: boxed }
}

// pub(super) trait ComponentSettableFuture: RustFuture {
//     type Definition: ComponentDefinition;

//     fn poll_with_component(
//         self: Pin<&mut Self>,
//         component: &mut Self::Definition,
//         cx: &mut Context,
//     ) -> Poll<Self::Output>;
// }

// struct ComponentFuture<CD, F>
// where
//     F: RustFuture + Send,
//     CD: ComponentDefinition + 'static,
// {
//     reference_holder: *mut CD,
//     future: Option<F>,
// }

// impl<CD, F> ComponentFuture<CD, F>
// where
//     F: RustFuture + Send,
//     CD: ComponentDefinition + 'static,
// {
//     pub(super) fn new(f: F) -> Pin<Box<Self>> {
//         let cf = ComponentFuture {
//             reference_holder: ptr::null_mut(),
//             future: Some(f),
//         };
//         Box::pin(cf)
//     }

//     pub(super) fn with_component(component: &mut CD, f: impl FnOnce(Pin<&mut CD>) -> F) -> Pin<Box<Self>> {
//     	let future = f(Pin::new_unchecked(component));

//     	let cf = ComponentFuture {
//             reference_holder: ptr::null_mut(),
//             future: None,
//         };
//         let pinned = Box::pin(cf);
//         //pinned.as_mut().set_component(component);
//         //let c = pinned.as_mut().map_unchecked_mut(|cf| &mut cf.reference_holder);

//         pinned
//     }

//     fn set_component(self: Pin<&mut Self>, component: &mut CD) {
//     	unsafe {
//         	let ref_holder = &mut self.get_unchecked_mut().reference_holder;
//             *ref_holder = component as *mut CD;
//     	}
//     }
//     fn unset_component(self: Pin<&mut Self>) {
//     	unsafe {
//         	let ref_holder = &mut self.get_unchecked_mut().reference_holder;
//             *ref_holder = ptr::null_mut();
//     	}
//     }
// }

// impl<CD, F> RustFuture for ComponentFuture<CD, F>
// where
//     F: RustFuture + Send,
//     CD: ComponentDefinition + 'static,
// {
//     type Output = ();

//     fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
//         //self.future.poll(cx).map(|_| ())
//         let future = unsafe { self.map_unchecked_mut(|s| &mut s.future) };
//         RustFuture::poll(future, cx).map(|_| ())
//     }
// }

// impl<CD, F> ComponentSettableFuture for ComponentFuture<CD, F>
// where
//     F: RustFuture + Send,
//     CD: ComponentDefinition + 'static,
// {
//     type Definition = CD;

//     fn poll_with_component(
//         mut self: Pin<&mut Self>,
//         component: &mut Self::Definition,
//         cx: &mut Context,
//     ) -> Poll<Self::Output> {
//         unsafe {
//             let ref_holder = &mut self.as_mut().get_unchecked_mut().reference_holder;
//             *ref_holder = Some(component as *mut Self::Definition);
//         }
//         let res = self.as_mut().poll(cx);
//         unsafe {
//             let _ = self.get_unchecked_mut().reference_holder.take();
//         }
//         res
//     }

//     // fn set_component(&mut self, component: &mut Self::Definition) {
//     // 	unsafe {
//     //     	*self.reference_holder.get() = Some(component as *mut Self::Definition);
//     // 	}
//     // }

//     // fn unset_component(&mut self) {
//     // 	unsafe {
//     //     	let _ = self.reference_holder.get().as_mut().take();
//     // 	}
//     // }
// }
