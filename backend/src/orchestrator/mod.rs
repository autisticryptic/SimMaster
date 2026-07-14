//! Multi-path SMS orchestration (design doc phase C).
//!
//! The orchestrator sits above the individual access legs (VoWiFi / VoLTE / CS)
//! and turns the user-configured [`SmsPathPolicy`] into concrete decisions:
//!
//!   - **Send routing** ([`sms_router`]): given the policy and a live readiness
//!     snapshot, produce a priority-ordered plan of legs to attempt, with
//!     fallback and a configurable mid-flight-disable behavior.
//!   - **Listener election** ([`listener_election`]): pick the *single* IMS leg
//!     that owns MT (received) SMS at any moment, so the same number is never
//!     registered on two IMS legs at once (which would double-deliver).
//!   - **Cross-transport dedup** ([`dedup`]): a stable content fingerprint plus
//!     a race-free DB claim so a message that arrives on more than one leg is
//!     stored exactly once.
//!
//! Design constraints honored here:
//!   - The leg set is a **closed enum** (`AccessPathKind`), so routing uses
//!     `match`/enum dispatch — no `dyn` trait objects, no heap allocation (see
//!     design doc §4.3 on why `async fn in trait` is avoided).
//!   - Everything in this module is **pure decision logic with no IO**, so it
//!     runs in full under `cargo test` on Windows CI. The actual byte-pushing is
//!     performed by a caller-supplied dispatcher which the live wiring
//!     implements once the per-leg live IO lands.

// Staged module: the pure planners below are exercised by unit tests now and
// wired into the live send/receive paths when per-leg live IO lands (design
// phase B-live). Mirror the `access::volte` module's allowance for staged code.
#![allow(dead_code)]

pub mod dedup;
pub mod listener_election;
pub mod sms_router;

// Re-exported as the module's public API surface; consumed by the live wiring
// (and by tests) rather than within this file.
#[allow(unused_imports)]
pub use dedup::{message_fingerprint, MessageFingerprintInput};
#[allow(unused_imports)]
pub use listener_election::{
    elect_listener, CsListenerAction, ElectionOutcome, LegReceiveReadiness,
};
#[allow(unused_imports)]
pub use sms_router::{
    plan_candidates, AttemptOutcome, Candidate, LegSendReadiness, RouteDecision, SendRouter,
    SkipReason,
};
