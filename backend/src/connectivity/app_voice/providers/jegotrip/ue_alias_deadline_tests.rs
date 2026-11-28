//! Deterministic distinction of expiration from pre-deadline retirement.
use super::*;
use n::batch_scheduler::AliasOutcome;
#[test]
fn ue_alias_deadline_sample_is_typed_even_when_earlier_actor_tick_preceded_expiry(){
    let mut f=Case::new();let deadline=Instant::now()+Duration::from_millis(20);let wait=f.b.begin_ue_alias_read(&f.owner,&f.scope,deadline).unwrap();
    let earlier=Instant::now();assert!(earlier<deadline);assert!(matches!(wait.sampled_outcome(),AliasOutcome::Live(None)));
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
    assert!(matches!(wait.sampled_outcome(),AliasOutcome::Expired));assert_eq!(wait.outcome(),Some(Err(CodecError::InvalidField)));assert!(f.scope.is_open());
}
#[test]
fn ue_alias_deadline_sampling_keeps_current_gate_retirement_distinct_from_timeout(){
    let mut f=Case::new();let wait=f.begin();f.gate.retire();
    assert!(matches!(wait.sampled_outcome(),AliasOutcome::Live(Some(Err(CodecError::InvalidField)))));assert!(f.scope.is_open());
}
