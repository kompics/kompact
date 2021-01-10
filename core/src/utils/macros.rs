/// A macro that provides a shorthand for an irrefutable let binding,
/// that the compiler cannot determine on its own
///
/// This is equivalent to the following code:
/// `let name = if let Pattern(name) = expression { name } else { unreachable!(); };`
#[macro_export]
macro_rules! let_irrefutable {
    ($name:ident, $pattern:pat = $expression:expr) => {
        let $name = if let $pattern = $expression {
            $name
        } else {
            unreachable!("Pattern is irrefutable!");
        };
    };
}

/// A macro that provides an empty implementation of [ComponentLifecycle](ComponentLifecycle) for the given component
///
/// Use this in components that do not require any special treatment of lifecycle events.
///
/// # Example
///
/// To ignore lifecycle events for a component `TestComponent`, write:
/// `ignore_lifecycle!(TestComponent);`
///
/// This is equivalent to `impl ComponentLifecycle for TestComponent {}`
/// and uses the default implementations for each trait function.
#[macro_export]
macro_rules! ignore_lifecycle {
    ($component:ty) => {
        impl ComponentLifecycle for $component {}
    };
}

/// A macro that provides an implementation of [ComponentLifecycle](ComponentLifecycle) for the given component that logs
/// the lifecycle stages at INFO log level.
///
/// Use this in components that do not require any special treatment of lifecycle events besides logging.
///
/// # Example
///
/// To log lifecycle events for a component `TestComponent`, write:
/// `info_lifecycle!(TestComponent);`
#[macro_export]
macro_rules! info_lifecycle {
    ($component:ty) => {
        impl ComponentLifecycle for $component {
            fn on_start(&mut self) -> Handled {
                info!(self.log(), "Starting...");
                Handled::Ok
            }

            fn on_stop(&mut self) -> Handled {
                info!(self.log(), "Stopping...");
                Handled::Ok
            }

            fn on_kill(&mut self) -> Handled {
                info!(self.log(), "Killing...");
                Handled::Ok
            }
        }
    };
}

/// A macro that provides an empty implementation of [ComponentLifecycle](ComponentLifecycle) for the given component
///
/// Use this in components that do not require any special treatment of control events.
///
/// # Example
///
/// To ignore control events for a component `TestComponent`, write:
/// `ignore_control!(TestComponent);`
#[deprecated(
    since = "0.10.0",
    note = "Use the ignore_lifecyle macro instead. This macro will be removed in future version of Kompact"
)]
#[macro_export]
macro_rules! ignore_control {
    ($component:ty) => {
        ignore_lifecycle!($component);
    };
}

/// A macro that provides an empty implementation of the provided handler for the given `$port` on the given `$component`
///
/// Use this in components that do not require any treatment of requests on `$port`, for example
/// where `$port::Request` is the uninhabited `Never` type.
///
/// # Example
///
/// To ignore requests on a port `TestPort` for a component `TestComponent`, write:
/// `ignore_requests!(TestPort, TestComponent);`
#[macro_export]
macro_rules! ignore_requests {
    ($port:ty, $component:ty) => {
        impl Provide<$port> for $component {
            fn handle(&mut self, _event: <$port as Port>::Request) -> Handled {
                Handled::Ok // ignore all
            }
        }
    };
}

/// A macro that provides an empty implementation of the required handler for the given `$port` on the given `$component`
///
/// Use this in components that do not require any treatment of indications on `$port`, for example
/// where `$port::Indication` is the uninhabited `Never` type.
///
/// # Example
///
/// To ignore indications on a port `TestPort` for a component `TestComponent`, write:
/// `ignore_indications!(TestPort, TestComponent);`
#[macro_export]
macro_rules! ignore_indications {
    ($port:ty, $component:ty) => {
        impl Require<$port> for $component {
            fn handle(&mut self, _event: <$port as Port>::Indication) -> Handled {
                Handled::Ok // ignore all
            }
        }
    };
}
