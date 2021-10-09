use super::*;
use executors::*;

/// API for a Kompact scheduler
///
/// Any scheduler implementation must implement this trait
/// so it can be used with Kompact.
///
/// Usually that means implementing some kind of wrapper
/// type for your particular scheduler, such as
/// [ExecutorScheduler](runtime::ExecutorScheduler), for example.
pub trait Scheduler: Send + Sync {
    /// Schedule `c` to be run on this scheduler
    ///
    /// Implementations must call [`c.execute()`](CoreContainer::execute)
    /// on the target thread.
    fn schedule(&self, c: Arc<dyn CoreContainer>) -> ();

    /// Shut this pool down asynchronously
    ///
    /// Implementations must eventually result in a correct
    /// shutdown, even when called from within one of its own threads.
    fn shutdown_async(&self) -> ();

    /// Shut this pool down synchronously
    ///
    /// Implementations must only return when the pool
    /// has been shut down, or upon an error.
    fn shutdown(&self) -> Result<(), String>;

    /// Clone an instance of this boxed
    ///
    /// Simply implement as `Box::new(self.clone())`.
    ///
    /// This is just a workaround for issues with boxed objects
    /// and [Clone](std::clone::Clone) implementations.
    fn box_clone(&self) -> Box<dyn Scheduler>;

    /// Handle the system being poisoned
    ///
    /// Usually this should just cause the scheduler to be
    /// shut down in an appropriate manner.
    fn poison(&self) -> ();

    /// Run a Future on this pool
    fn spawn(&self, future: futures::future::BoxFuture<'static, ()>) -> ();
}

impl Clone for Box<dyn Scheduler> {
    fn clone(&self) -> Self {
        (*self).box_clone()
    }
}

/// A wrapper for schedulers from the [executors](executors) crate
#[derive(Clone)]
pub struct ExecutorScheduler<E>
where
    E: FuturesExecutor + Sync,
{
    exec: E,
}

impl<E: FuturesExecutor + Sync + 'static> ExecutorScheduler<E> {
    /// Produce a new `ExecutorScheduler` from an [Executor](executors::Executor) `E`.
    pub fn with(exec: E) -> ExecutorScheduler<E> {
        ExecutorScheduler { exec }
    }

    /// Produce a new boxed [Scheduler](runtime::Scheduler) from an [Executor](executors::Executor) `E`.
    pub fn from(exec: E) -> Box<dyn Scheduler> {
        Box::new(ExecutorScheduler::with(exec))
    }
}

impl<E: FuturesExecutor + Sync + 'static> Scheduler for ExecutorScheduler<E> {
    fn schedule(&self, c: Arc<dyn CoreContainer>) -> () {
        self.exec.execute(move || maybe_reschedule(c));
    }

    fn shutdown_async(&self) -> () {
        self.exec.shutdown_async()
    }

    fn shutdown(&self) -> Result<(), String> {
        self.exec.shutdown_borrowed()
    }

    fn box_clone(&self) -> Box<dyn Scheduler> {
        Box::new(self.clone())
    }

    fn poison(&self) -> () {
        self.exec.shutdown_async();
    }

    fn spawn(&self, future: futures::future::BoxFuture<'static, ()>) -> () {
        let handle = self.exec.spawn(future);
        handle.detach();
    }
}

fn maybe_reschedule(c: Arc<dyn CoreContainer>) {
    match c.execute() {
        SchedulingDecision::Schedule => {
            if cfg!(feature = "use_local_executor") {
                let res = try_execute_locally(move || maybe_reschedule(c));
                assert!(!res.is_err(), "Only run with Executors that can support local execute or remove the avoid_executor_lookups feature!");
            } else {
                let c2 = c.clone();
                c.system().schedule(c2);
            }
        }
        SchedulingDecision::Resume => maybe_reschedule(c),
        _ => (),
    }
}
