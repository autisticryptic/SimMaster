use super::dial_outcome::{DialEvidence, DialReport};
use crate::api::handlers::start_owned_automation_call;
use crate::services::automation::target::resolve_line_target;
use crate::services::automation::traits::{AutomationExecutionReport, AutomationTaskHandler};
use crate::services::trunk::bridge::{OperatorCommand, OperatorEvent};
use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use futures_util::future::{BoxFuture, FutureExt};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{broadcast, oneshot};
use tracing::info;

pub struct DialCallHandler;

fn normalize_phone(country_code: &str, phone_number: &str) -> Result<String> {
    let country = country_code.trim();
    let number = phone_number.trim();
    if !country.starts_with('+')
        || country.len() < 2
        || !country[1..].chars().all(|c| c.is_ascii_digit())
    {
        return Err(anyhow!("国家区号格式必须为 +数字"));
    }
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return Err(anyhow!("手机号码主体只能包含数字"));
    }
    Ok(format!("{country}{number}"))
}

struct CancelCallTask {
    flag: Arc<AtomicBool>,
    signal: Option<oneshot::Sender<()>>,
}
impl Drop for CancelCallTask {
    fn drop(&mut self) {
        self.flag.store(true, Ordering::Release);
        if let Some(sender) = self.signal.take() {
            let _ = sender.send(());
        }
    }
}

/// This future runs in a shielded task. A dropped scheduler waiter cancels its
/// admission and wait, but cannot abandon the exact call ID after dispatch.
async fn run_owned_call<S, W, WF, H, HF, T>(
    start: S,
    wait: W,
    hangup: H,
    mut cancelled: oneshot::Receiver<()>,
) -> Result<T>
where
    S: Future<Output = Result<String>>,
    W: FnOnce(String) -> WF,
    WF: Future<Output = Result<T>>,
    H: FnOnce(String) -> HF,
    HF: Future<Output = Result<()>>,
{
    let call_id = start.await.context("定时拨号失败")?;
    let outcome = tokio::select! {
        biased;
        _ = &mut cancelled => Err(anyhow!("automation_call_cancelled")),
        outcome = wait(call_id.clone()) => outcome,
    };
    hangup(call_id).await.context("自动挂机失败")?;
    outcome
}

async fn observe_call(
    mut events: broadcast::Receiver<OperatorEvent>,
    call_id: String,
    duration: Duration,
) -> Result<DialReport> {
    let deadline = tokio::time::Instant::now() + duration;
    let mut evidence = DialEvidence::default();
    loop {
        // Bound a drain pass. Lag/overload fails closed instead of using stale
        // ringing; already buffered failure/cancel wins over deadline success.
        for index in 0..=64 {
            match events.try_recv() {
                Ok(event) => {
                    if index == 64 {
                        return Err(anyhow!("automation_call_observation_lost"));
                    }
                    if let Some(result) = evidence.event(&call_id, &event) {
                        return result;
                    }
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(_) => return Err(anyhow!("automation_call_observation_lost")),
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return evidence.deadline();
        }
        tokio::select! {
            biased;
            event = events.recv() => match event {
                Ok(event) => if let Some(result) = evidence.event(&call_id, &event) { return result; },
                Err(_) => return Err(anyhow!("automation_call_observation_lost")),
            },
            _ = tokio::time::sleep_until(deadline) => {}, // recheck buffered events
        }
    }
}

impl AutomationTaskHandler for DialCallHandler {
    fn task_type(&self) -> &'static str {
        "dial_call"
    }

    fn execute<'a>(
        &'a self,
        app: &'a AppState,
        params: &'a serde_json::Value,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { self.execute_report(app, params).await.map(|_| ()) })
    }

    fn execute_report<'a>(
        &'a self,
        app: &'a AppState,
        params: &'a serde_json::Value,
    ) -> BoxFuture<'a, Result<AutomationExecutionReport>> {
        async move {
            let phone = normalize_phone(params.get("country_code").and_then(|v| v.as_str()).unwrap_or(""),
                params.get("phone_number").and_then(|v| v.as_str()).unwrap_or(""))?;
            let duration = params.get("duration_seconds").and_then(|v| v.as_u64()).unwrap_or(0).clamp(1, 7200);
            let target = resolve_line_target(app, params).await?;
            let line = app.line_registry.get(&target.line_id).await.ok_or_else(|| anyhow!("automation_target_line_not_found"))?;
            let link = line.voice_access.operator_link();
            let events = link.subscribe_call_events(); // Before Start; includes attempt/loss metadata.
            let app = app.clone();
            let flag = Arc::new(AtomicBool::new(false));
            let (signal, cancelled) = oneshot::channel();
            let _guard = CancelCallTask { flag: Arc::clone(&flag), signal: Some(signal) };
            let task = tokio::spawn(async move {
                let result = run_owned_call(
                    async { start_owned_automation_call(&app, &target.line_id, &phone, flag).await.map_err(anyhow::Error::msg) },
                    |call_id| observe_call(events, call_id, Duration::from_secs(duration)),
                    |call_id| async move {
                        link.send_command(OperatorCommand::HangupCall { call_id }).map_err(|_| anyhow!("automation_call_cleanup_unavailable"))
                    }, cancelled,
                ).await;
                if result.is_ok() {
                    info!(line_id = %target.line_id, duration_seconds = duration, "automation dial interval completed; hangup requested (not proof of answered audio)");
                } else {
                    // Error details are projected by scheduler, never log raw numbers/auth.
                    tracing::warn!(line_id = %target.line_id, "automation owned call stopped; cleanup requested if a call ID was allocated");
                }
                result
            });
            task.await.map_err(|_| anyhow!("automation_call_task_failed"))?
                .map(AutomationExecutionReport::Dial)
        }.boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn normalizes_country_code_and_body() {
        assert_eq!(
            normalize_phone("+86", "13800138000").unwrap(),
            "+8613800138000"
        );
        assert!(normalize_phone("86", "13800138000").is_err());
        assert!(normalize_phone("+86", "1380-0138000").is_err());
    }

    #[tokio::test]
    async fn scheduled_call_cancellation_keeps_cleanup_owner_through_late_start() {
        let (cancel, cancelled) = oneshot::channel();
        let (start, started) = oneshot::channel();
        let stopped = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&stopped);
        let task = tokio::spawn(run_owned_call(
            async move {
                started.await.unwrap();
                Ok("owned-call".into())
            },
            |_| std::future::pending::<Result<()>>(),
            move |id| async move {
                assert_eq!(id, "owned-call");
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            cancelled,
        ));
        cancel.send(()).unwrap();
        start.send(()).unwrap();
        assert!(task.await.unwrap().is_err());
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn scheduled_call_failure_does_not_hide_rejection_or_skip_exact_hangup() {
        let (_signal, cancelled) = oneshot::channel();
        let calls = AtomicUsize::new(0);
        let error = run_owned_call(
            async { Ok("owned".into()) },
            |_| async { Err::<(), _>(anyhow!("automation_call_rejected")) },
            |id| {
                let calls = &calls;
                async move {
                    assert_eq!(id, "owned");
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
            cancelled,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("automation_call_rejected"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn completed_report_defaults_preserve_non_dial_handlers() {
        use crate::services::automation::traits::AutomationExecutionReport;
        assert_eq!(AutomationExecutionReport::Completed.detail(), "执行成功");
    }

    #[tokio::test]
    async fn no_answer_cleanup_failure_does_not_publish_success() {
        let (_sender, cancelled) = oneshot::channel();
        let error = run_owned_call(
            async { Ok("owned".into()) },
            |_| async { Ok(()) },
            |_| async { Err(anyhow!("automation_call_cleanup_unavailable")) },
            cancelled,
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("自动挂机失败"));
    }

    #[tokio::test]
    async fn deadline_after_ringing_succeeds_but_buffered_failure_or_cancel_wins() {
        use super::super::dial_outcome::DialOutcome;
        use crate::services::trunk::bridge::VoiceCallObservation;
        for terminal in [
            None,
            Some(VoiceCallObservation::LocalCancelled),
            Some(VoiceCallObservation::EvidenceLost),
        ] {
            let (tx, rx) = broadcast::channel(8);
            tx.send(OperatorEvent::Observation {
                call_id: "owned".into(),
                fact: VoiceCallObservation::RemoteRinging,
            })
            .unwrap();
            if let Some(fact) = terminal {
                tx.send(OperatorEvent::Observation {
                    call_id: "owned".into(),
                    fact,
                })
                .unwrap();
            }
            let result = observe_call(rx, "owned".into(), Duration::ZERO).await;
            if terminal.is_some() {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap().outcome, DialOutcome::RingingAtDeadline);
            }
        }
        let (_tx, rx) = broadcast::channel(8);
        assert!(observe_call(rx, "owned".into(), Duration::ZERO)
            .await
            .unwrap_err()
            .to_string()
            .contains("delivery_unconfirmed"));
    }

    #[tokio::test]
    async fn observation_lag_fails_closed_and_duplicate_terminal_commits_once() {
        use crate::services::trunk::bridge::VoiceCallObservation;
        let (tx, rx) = broadcast::channel(2);
        for _ in 0..6 {
            tx.send(OperatorEvent::Observation {
                call_id: "owned".into(),
                fact: VoiceCallObservation::RemoteRinging,
            })
            .unwrap();
        }
        assert!(observe_call(rx, "owned".into(), Duration::ZERO)
            .await
            .unwrap_err()
            .to_string()
            .contains("observation_lost"));
        let (tx, rx) = broadcast::channel(8);
        let diagnostic =
            crate::connectivity::core::ims_failure::ImsFailureDiagnostic::from_response(
                b"SIP/2.0 486 Busy Here\r\n\r\n",
            )
            .unwrap()
            .for_initial_invite(true);
        for _ in 0..2 {
            tx.send(OperatorEvent::Rejected {
                call_id: "owned".into(),
                status: 486,
                diagnostic: diagnostic.clone(),
            })
            .unwrap();
        }
        tx.send(OperatorEvent::Unavailable {
            call_id: "owned".into(),
        })
        .unwrap();
        assert!(observe_call(rx, "owned".into(), Duration::ZERO)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn scheduled_call_observer_ignores_other_calls_but_reports_fast_rejection() {
        let (tx, rx) = broadcast::channel(8);
        tx.send(OperatorEvent::Unavailable {
            call_id: "someone-else".into(),
        })
        .unwrap();
        tx.send(OperatorEvent::Rejected {
            call_id: "owned".into(),
            status: 480,
            diagnostic: crate::connectivity::core::ims_failure::ImsFailureDiagnostic::from_status(
                480,
            ),
        })
        .unwrap();
        assert!(observe_call(rx, "owned".into(), Duration::from_secs(30))
            .await
            .unwrap_err()
            .to_string()
            .contains("sip_status=480"));
    }
}
