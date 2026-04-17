#![allow(clippy::unused_unit)]
use kompact::prelude::*;
use std::time::Duration;

const COUNT_TIMEOUT: Duration = Duration::from_millis(10);
const STATE_TIMEOUT: Duration = Duration::from_millis(1000);

// ANCHOR: state
#[derive(ComponentDefinition, Actor)]
struct UnstableCounter {
    ctx: ComponentContext<Self>,
    count: u8,
    count_timeout: Option<ScheduledTimer>,
    state_timeout: Option<ScheduledTimer>,
}
// ANCHOR_END: state

impl UnstableCounter {
    // ANCHOR: with_state
    fn with_state(count: u8) -> Self {
        UnstableCounter {
            ctx: ComponentContext::uninitialised(),
            count,
            count_timeout: None,
            state_timeout: None,
        }
    }

    // ANCHOR_END: with_state

    // ANCHOR: timeouts
    fn handle_count_timeout(&mut self, _timeout_id: ScheduledTimer) -> Handled {
        info!(self.log(), "Incrementing count of {}", self.count);
        self.count = self.count.checked_add(1).expect("Count overflowed!");
        Handled::Ok
    }

    fn handle_state_timeout(&mut self, _timeout_id: ScheduledTimer) -> Handled {
        info!(
            self.log(),
            "Saving recovery state with count of {}", self.count
        );
        let mut count_timeout = self.count_timeout.clone();
        let mut state_timeout = self.state_timeout.clone();
        let count = self.count;
        self.ctx.set_recovery_function(move |fault| {
            fault.recover_with(move |_ctx, system, logger| {
                warn!(
                    logger,
                    "Recovering UnstableCounter based on last state count={}", count
                );
                // Clean up now invalid timers
                if let Some(timeout) = count_timeout.take() {
                    system.cancel_timer(timeout);
                }
                if let Some(timeout) = state_timeout.take() {
                    system.cancel_timer(timeout);
                }
                let counter_component = system.create(move || Self::with_state(count));
                system.start(&counter_component);
            })
        });
        Handled::Ok
    }
    // ANCHOR_END: timeouts
}

// ANCHOR: default
impl Default for UnstableCounter {
    fn default() -> Self {
        UnstableCounter {
            ctx: ComponentContext::uninitialised(),
            count: 0,
            count_timeout: None,
            state_timeout: None,
        }
    }
}
// ANCHOR_END: default

// ANCHOR: lifecycle
impl ComponentLifecycle for UnstableCounter {
    fn on_start(&mut self) -> Handled {
        let count_timeout = self.schedule_periodic(
            COUNT_TIMEOUT,
            COUNT_TIMEOUT,
            UnstableCounter::handle_count_timeout,
        );
        self.count_timeout = Some(count_timeout.clone());
        let state_timeout = self.schedule_periodic(
            STATE_TIMEOUT,
            STATE_TIMEOUT,
            UnstableCounter::handle_state_timeout,
        );
        self.state_timeout = Some(state_timeout.clone());
        let count = self.count;
        self.ctx.set_recovery_function(move |fault| {
            fault.recover_with(move |_ctx, system, logger| {
                warn!(
                    logger,
                    "Recovering UnstableCounter based on last state count={}", count
                );
                // Clean up now invalid timers
                system.cancel_timer(count_timeout);
                system.cancel_timer(state_timeout);
                let counter_component = system.create(move || Self::with_state(count));
                system.start(&counter_component);
            })
        });
        Handled::Ok
    }

    fn on_stop(&mut self) -> Handled {
        if let Some(timeout) = self.count_timeout.take() {
            self.cancel_timer(timeout);
        }
        if let Some(timeout) = self.state_timeout.take() {
            self.cancel_timer(timeout);
        }
        Handled::Ok
    }

    fn on_kill(&mut self) -> Handled {
        self.on_stop()
    }
}
// ANCHOR_END: lifecycle

// ANCHOR: main
pub fn main() {
    let system = KompactConfig::default().build().expect("system");
    let component = system.create(UnstableCounter::default);
    system.start(&component);
    drop(component); // avoid it from holding on to memory after crashing
    std::thread::sleep(Duration::from_millis(5000));
    println!("Shutting down system");
    system.shutdown().expect("shutdown");
}
// ANCHOR_END: main

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unstable_counter() {
        main();
    }
}
