//! Keep all queue regressions under the existing account_calls::tests filter.
use super::super::invites::queue_tests;
#[test] fn invite_queue_same_id_coalesces_original_holders_without_second_worker(){queue_tests::case(0);}
#[test] fn invite_queue_duplicate_does_not_refresh_original_deadline(){queue_tests::case(1);}
#[test] fn invite_queue_foreign_owner_scope_same_bits_cannot_replace_original_group(){queue_tests::case(2);}
#[test] fn invite_queue_same_id_changed_opaque_values_is_not_a_new_notification(){queue_tests::case(3);}
#[test] fn invite_queue_group_capacity_keeps_unconsumed_associations(){queue_tests::case(4);}
#[test] fn invite_queue_holder_capacity_is_shared_across_direct_and_batch(){queue_tests::case(5);}
#[test] fn invite_queue_retained_bytes_reject_before_over_budget_admission(){queue_tests::case(6);}
#[test] fn invite_queue_inflight_data_keeps_budget_through_retirement_and_real_drop(){queue_tests::case(7);}
#[test] fn invite_queue_retire_latches_claim_and_drops_only_pending_original_data(){queue_tests::case(8);}
#[test] fn invite_queue_terminal_duplicate_restarts_only_receipt_worker_not_consumer_deadline(){queue_tests::case(9);}
#[test] fn invite_queue_closed_scope_cannot_admit_even_previously_captured_data(){queue_tests::case(10);}
#[test] fn invite_queue_session_revocation_rejects_before_budget_or_group_mutation(){queue_tests::case(11);}
#[test] fn invite_queue_empty_to_idle_is_atomic_with_new_holder_admission(){queue_tests::case(12);}
