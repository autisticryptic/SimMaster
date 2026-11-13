//! Actions-only owning notification deadlines and uncertain remote outcomes.
use super::*;
struct EagerDispatcher { calls: AtomicUsize, active: AtomicUsize, outcome: Option<NoticeOutcome> }
impl EagerDispatcher { fn new(outcome: Option<NoticeOutcome>) -> Self { Self { calls: AtomicUsize::new(0), active: AtomicUsize::new(0), outcome } } }
impl NoticeDispatcher for EagerDispatcher {
    fn deliver<'a>(&'a self, _: &'a NoticeWork) -> NoticeFuture<'a> {
        // Deliberately observable BEFORE the returned future is polled. This
        // models a dispatcher that may already have handed data to a sender.
        self.calls.fetch_add(1, Ordering::SeqCst); self.active.fetch_add(1, Ordering::SeqCst);
        let active = Active(&self.active); let outcome = self.outcome;
        Box::pin(async move { let _active = active; match outcome { Some(result) => result, None => std::future::pending().await } })
    }
}
fn claimed() -> (Arc<Database>, String, NoticeWork) {
    let (db, line, context) = setup(1); context.commit_fixture(&db, 1, &message()).unwrap();
    let work = claim_next(&db, now()).unwrap().unwrap(); (db, line, work)
}
fn assert_uncertain(db: &Database, line: &str) {
    let jobs = status(db, line); assert_eq!(jobs.len(), 1); assert_eq!(jobs[0].state, "uncertain"); assert_eq!(jobs[0].attempts, 1);
    assert_eq!(sms_status(db, jobs[0].sms_id), "failed"); assert!(claim_next(db, now() + 61).unwrap().is_none());
    let scrubbed: bool = db.with_conn(|c| c.query_row("SELECT title='' AND body='' AND claim IS NULL FROM app_voice_notification_jobs", [], |r| r.get(0)).map_err(|_| STORAGE)).unwrap();
    assert!(scrubbed); assert_eq!(count(db, "sms_messages"), 1);
    assert_eq!(db.get_sms_messages_for_channel(10, 0, None, Some(line)).unwrap()[0].content, "fixture-private-text");
}
#[tokio::test]
async fn expired_owning_deadline_never_even_constructs_a_synchronous_dispatch_future() {
    let (db, line, work) = claimed(); let dispatcher = Arc::new(EagerDispatcher::new(Some(NoticeOutcome::Delivered)));
    let (_control, signal) = crate::platform::shutdown::channel(); let deadline = tokio::time::Instant::now() - Duration::from_secs(1);
    send_one_before(db.clone(), dispatcher.clone(), work, signal, deadline).await;
    assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 0); assert_eq!(dispatcher.active.load(Ordering::SeqCst), 0); assert_uncertain(&db, &line);
}
#[tokio::test]
async fn owner_timeout_after_dispatch_entry_is_uncertain_and_never_automatically_replayed() {
    let (db, line, work) = claimed(); let dispatcher = Arc::new(EagerDispatcher::new(None));
    let (_control, signal) = crate::platform::shutdown::channel(); let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    tokio::time::timeout(Duration::from_secs(3), send_one_before(db.clone(), dispatcher.clone(), work, signal, deadline)).await.unwrap();
    assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 1); assert_eq!(dispatcher.active.load(Ordering::SeqCst), 0); assert_uncertain(&db, &line);
}
#[tokio::test]
async fn an_already_requested_shutdown_does_not_construct_dispatch_after_a_claim() {
    let (db, line, work) = claimed(); let dispatcher = Arc::new(EagerDispatcher::new(Some(NoticeOutcome::Delivered)));
    let (control, signal) = crate::platform::shutdown::channel(); control.trigger();
    send_one_before(db.clone(), dispatcher.clone(), work, signal, tokio::time::Instant::now() + ATTEMPT_TIMEOUT).await;
    assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 0); assert_uncertain(&db, &line);
}
#[tokio::test]
async fn cancellation_drops_inflight_sender_and_preserves_the_original_uncertain_claim() {
    let (db, line, work) = claimed(); let dispatcher = Arc::new(EagerDispatcher::new(None)); let (_control, signal) = crate::platform::shutdown::channel();
    let task = tokio::spawn(send_one_before(db.clone(), dispatcher.clone(), work, signal, tokio::time::Instant::now() + ATTEMPT_TIMEOUT));
    tokio::time::timeout(Duration::from_secs(2), async { while dispatcher.calls.load(Ordering::SeqCst) == 0 { tokio::task::yield_now().await; } }).await.unwrap();
    task.abort(); assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(dispatcher.active.load(Ordering::SeqCst), 0); assert_uncertain(&db, &line);
}
#[tokio::test]
async fn an_explicit_failed_outcome_keeps_the_existing_finite_retry_policy() {
    let (db, line, work) = claimed(); let dispatcher = Arc::new(EagerDispatcher::new(Some(NoticeOutcome::Failed))); let (_control, signal) = crate::platform::shutdown::channel();
    send_one_before(db.clone(), dispatcher.clone(), work, signal, tokio::time::Instant::now() + ATTEMPT_TIMEOUT).await;
    assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 1); assert_eq!(status(&db, &line)[0].state, "pending"); assert_eq!(status(&db, &line)[0].attempts, 1);
    let next = claim_next(&db, now() + 61).unwrap().unwrap(); assert_eq!(next.attempt, 2); finish(&db, &next, NoticeOutcome::Delivered, now() + 61).unwrap();
}
#[tokio::test]
async fn an_explicit_delivered_outcome_is_committed_and_payload_is_scrubbed() {
    let (db, line, work) = claimed(); let dispatcher = Arc::new(EagerDispatcher::new(Some(NoticeOutcome::Delivered))); let (_control, signal) = crate::platform::shutdown::channel();
    send_one_before(db.clone(), dispatcher.clone(), work, signal, tokio::time::Instant::now() + ATTEMPT_TIMEOUT).await;
    assert_eq!(status(&db, &line)[0].state, "delivered"); assert_eq!(sms_status(&db, status(&db, &line)[0].sms_id), "success"); assert!(claim_next(&db, now() + 61).unwrap().is_none());
    assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 1); assert_eq!(dispatcher.active.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn an_explicit_deferral_still_refunds_attempt_without_changing_the_expiry() {
    let (db, line, work) = claimed(); let expires = work.expires; let dispatcher = Arc::new(EagerDispatcher::new(Some(NoticeOutcome::Deferred(Duration::from_secs(5)))));
    let (_control, signal) = crate::platform::shutdown::channel();
    send_one_before(db.clone(), dispatcher, work, signal, tokio::time::Instant::now() + ATTEMPT_TIMEOUT).await;
    let jobs = status(&db, &line); assert_eq!(jobs[0].state, "pending"); assert_eq!(jobs[0].attempts, 0); assert_eq!(jobs[0].expires_at, expires);
    let next = claim_next(&db, now() + 6).unwrap().unwrap(); assert_eq!(next.attempt, 1); finish(&db, &next, NoticeOutcome::Delivered, now() + 6).unwrap();
}
struct CancelBeforeReturning { db: Arc<Database>, line: String }
impl NoticeDispatcher for CancelBeforeReturning {
    fn deliver<'a>(&'a self, _: &'a NoticeWork) -> NoticeFuture<'a> {
        self.db.with_conn(|conn| cancel_account(conn, &self.line)).unwrap(); Box::pin(async { NoticeOutcome::Delivered })
    }
}
#[tokio::test]
async fn late_success_after_account_cancellation_cannot_restore_the_original_payload_or_claim() {
    let (db, line, work) = claimed(); let dispatcher = Arc::new(CancelBeforeReturning { db: db.clone(), line: line.clone() }); let (_control, signal) = crate::platform::shutdown::channel();
    send_one_before(db.clone(), dispatcher, work, signal, tokio::time::Instant::now() + ATTEMPT_TIMEOUT).await;
    assert_eq!(status(&db, &line)[0].state, "cancelled"); assert!(claim_next(&db, now() + 61).unwrap().is_none());
    let scrubbed: bool = db.with_conn(|c| c.query_row("SELECT title='' AND body='' AND claim IS NULL FROM app_voice_notification_jobs", [], |r| r.get(0)).map_err(|_| STORAGE)).unwrap();
    assert!(scrubbed); assert_eq!(count(&db, "sms_messages"), 1);
}
