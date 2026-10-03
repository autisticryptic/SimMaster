//! Keep prior-boot recovery ahead of every path that can provision UE workers.
//! Each refresh performs at most one proof, with a cooldown; failure never
//! authorizes a namespace, bearer, SIM write, or modem reset.

use std::{
    future::Future,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const PENDING: &str = "ims_startup_recovery_pending";

#[derive(Default)]
enum State {
    #[default]
    Ready,
    Pending(Option<Instant>),
}

#[derive(Default)]
pub(super) struct ImsStartupGate {
    state: Mutex<State>,
}

impl ImsStartupGate {
    pub async fn defer(&self) {
        *self.state.lock().await = State::Pending(None);
    }

    /// Returns true only when this call completed a previously deferred proof.
    pub async fn ensure_ready<F, Fut>(&self, recover: F) -> Result<bool, &'static str>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = bool>,
    {
        self.ensure_ready_at(Instant::now(), recover).await
    }

    async fn ensure_ready_at<F, Fut>(&self, now: Instant, recover: F) -> Result<bool, &'static str>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = bool>,
    {
        let mut state = self.state.lock().await;
        match &*state {
            State::Ready => return Ok(false),
            State::Pending(Some(next)) if now < *next => return Err(PENDING),
            State::Pending(_) => {}
        }
        // Arm the next observation before awaiting; cancellation also leaves
        // the gate closed rather than implicitly admitting a worker.
        *state = State::Pending(Some(now + RETRY_INTERVAL));
        if !recover().await {
            return Err(PENDING);
        }
        *state = State::Ready;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn prior_boot_transient_defers_provisioning_until_proof_succeeds() {
        let gate = ImsStartupGate::default();
        gate.defer().await;
        let now = Instant::now();
        let attempts = AtomicUsize::new(0);
        let provisioned = AtomicUsize::new(0);
        let first = gate
            .ensure_ready_at(now, || async {
                attempts.fetch_add(1, Ordering::SeqCst);
                false // MM/SIM not yet enumerated.
            })
            .await;
        if first.is_ok() {
            provisioned.fetch_add(1, Ordering::SeqCst);
        }
        assert_eq!(first, Err(PENDING));
        assert_eq!(provisioned.load(Ordering::SeqCst), 0);
        assert_eq!(
            gate.ensure_ready_at(now, || async {
                panic!("cooldown must not perform another proof")
            })
            .await,
            Err(PENDING)
        );
        let second = gate
            .ensure_ready_at(now + RETRY_INTERVAL, || async {
                attempts.fetch_add(1, Ordering::SeqCst);
                true
            })
            .await;
        if second.is_ok() {
            provisioned.fetch_add(1, Ordering::SeqCst);
        }
        assert_eq!(second, Ok(true));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(provisioned.load(Ordering::SeqCst), 1);
        assert_eq!(
            gate.ensure_ready(|| async { panic!("already recovered") })
                .await,
            Ok(false)
        );
    }

    #[tokio::test]
    async fn unresolved_prior_boot_proof_never_opens_gate() {
        let gate = ImsStartupGate::default();
        gate.defer().await;
        let now = Instant::now();
        for n in 0..4 {
            assert_eq!(
                gate.ensure_ready_at(now + RETRY_INTERVAL * n, || async { false })
                    .await,
                Err(PENDING)
            );
        }
    }

    #[tokio::test]
    async fn ordinary_startup_does_not_invoke_recovery() {
        assert_eq!(
            ImsStartupGate::default()
                .ensure_ready(|| async { panic!("no prior-boot gate was requested") })
                .await,
            Ok(false)
        );
    }

    #[tokio::test]
    async fn cancelling_proof_keeps_gate_closed() {
        let gate = ImsStartupGate::default();
        gate.defer().await;
        let now = Instant::now();
        {
            let proof = gate.ensure_ready_at(now, || std::future::pending::<bool>());
            tokio::pin!(proof);
            assert!(futures_util::poll!(&mut proof).is_pending());
        }
        assert_eq!(
            gate.ensure_ready_at(now, || async {
                panic!("cancelled proof cannot admit or immediately retry")
            })
            .await,
            Err(PENDING)
        );
        assert_eq!(
            gate.ensure_ready_at(now + RETRY_INTERVAL, || async { false })
                .await,
            Err(PENDING)
        );
    }
}
