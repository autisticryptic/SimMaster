//! Process-wide shutdown signal.
//!
//! Axum's `with_graceful_shutdown` stops accepting new connections and then
//! waits for the in-flight ones to finish. A Server-Sent Events response never
//! finishes on its own, so without a way to tell those streams to end, a single
//! browser with the UI open holds the drain open until the force-exit watchdog
//! kills the process. That matters well beyond a slow restart: the forced exit
//! skips every teardown path, which is what leaves a DATA netdev stranded inside
//! a UE namespace (see [`crate::platform::netns::reclaim_all_stranded_hardware_links`]).
//!
//! This is a `watch` channel rather than a `oneshot` or a `Notify` because it
//! has to be observable from an arbitrary number of long-lived streams, stay
//! readable *after* it fires (a stream created mid-shutdown must see it
//! immediately, not block forever waiting for an edge that already passed), and
//! be cheap to clone into application state.

use std::sync::Arc;

use tokio::sync::watch;

/// Fires the shutdown signal. Held by the signal handler only.
#[derive(Debug)]
pub struct ShutdownController {
    tx: watch::Sender<bool>,
}

/// Observes the shutdown signal. Clone freely into tasks and handlers.
#[derive(Clone, Debug)]
pub struct ShutdownSignal {
    rx: watch::Receiver<bool>,
    /// Only set by [`ShutdownSignal::never`], to keep its channel open. `wait`
    /// treats a closed channel as shutdown, so an inert signal has to hold its
    /// own sender alive.
    _keepalive: Option<Arc<watch::Sender<bool>>>,
}

/// Create a fresh, un-fired shutdown signal.
pub fn channel() -> (ShutdownController, ShutdownSignal) {
    let (tx, rx) = watch::channel(false);
    (
        ShutdownController { tx },
        ShutdownSignal {
            rx,
            _keepalive: None,
        },
    )
}

impl ShutdownController {
    /// Announce shutdown. Idempotent, and safe to call with no receivers left.
    pub fn trigger(&self) {
        // A send error only means every receiver is gone, which is precisely
        // the state this call is trying to bring about.
        let _ = self.tx.send(true);
    }

    /// An observer for this controller.
    pub fn signal(&self) -> ShutdownSignal {
        ShutdownSignal {
            rx: self.tx.subscribe(),
            _keepalive: None,
        }
    }
}

impl ShutdownSignal {
    /// A signal that never fires. For tests and for call sites that construct
    /// application state without a running server.
    pub fn never() -> Self {
        let (tx, rx) = watch::channel(false);
        // The sender is kept alive inside the signal rather than dropped: `wait`
        // treats a closed channel as shutdown, so an inert signal has to own it.
        Self {
            rx,
            _keepalive: Some(Arc::new(tx)),
        }
    }

    /// True once shutdown has been announced.
    pub fn is_shutting_down(&self) -> bool {
        *self.rx.borrow()
    }

    /// Resolve when shutdown is announced, immediately if it already was.
    ///
    /// Takes `&mut self` because it advances this observer's seen-version, which
    /// is what makes the "already fired" case return without waiting.
    pub async fn wait(&mut self) {
        if self.is_shutting_down() {
            return;
        }
        // A closed channel means the controller is gone, which can only happen
        // while the process is on its way down. Treat it as shutdown rather
        // than waiting for a signal that can no longer arrive.
        let _ = self.rx.changed().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_returns_immediately_when_already_triggered() {
        let (controller, mut signal) = channel();
        controller.trigger();
        assert!(signal.is_shutting_down());
        // Would hang rather than fail if the already-fired case were missed.
        signal.wait().await;
    }

    /// A stream that subscribes after the signal fired must still observe it.
    /// This is the case a `Notify` or a consumed `oneshot` would silently lose.
    #[tokio::test]
    async fn a_late_observer_still_sees_shutdown() {
        let (controller, _signal) = channel();
        controller.trigger();
        let mut late = controller.signal();
        assert!(late.is_shutting_down());
        late.wait().await;
    }

    #[tokio::test]
    async fn wait_resolves_when_the_controller_is_dropped() {
        let (controller, mut signal) = channel();
        drop(controller);
        // Not `is_shutting_down`, but nothing can ever set it now either.
        signal.wait().await;
    }

    #[tokio::test]
    async fn never_does_not_fire() {
        let mut signal = ShutdownSignal::never();
        assert!(!signal.is_shutting_down());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), signal.wait())
                .await
                .is_err(),
            "ShutdownSignal::never() must not resolve"
        );
    }
}
