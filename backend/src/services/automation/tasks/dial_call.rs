use crate::api::handlers::start_owned_automation_call;
use crate::services::automation::target::resolve_line_target;
use crate::services::automation::traits::AutomationTaskHandler;
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
async fn run_owned_call<S, W, WF, H, HF>(
    start: S,
    wait: W,
    hangup: H,
    mut cancelled: oneshot::Receiver<()>,
) -> Result<()>
where
    S: Future<Output = Result<String>>,
    W: FnOnce(String) -> WF,
    WF: Future<Output = Result<()>>,
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
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + duration;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(OperatorEvent::Rejected { call_id: id, status, .. }) if id == call_id => return Err(anyhow!("automation_call_rejected:sip_status={status}")),
                Ok(OperatorEvent::Unavailable { call_id: id }) if id == call_id => return Err(anyhow!("automation_call_access_unavailable")),
                Ok(OperatorEvent::Ended { call_id: id } | OperatorEvent::Cancelled { call_id: id }) if id == call_id => return Err(anyhow!("automation_call_ended_before_duration")),
                Ok(_) => {},
                Err(_) => return Err(anyhow!("automation_call_observation_lost")),
            },
            _ = tokio::time::sleep_until(deadline) => return Ok(()),
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
        async move {
            let phone = normalize_phone(params.get("country_code").and_then(|v| v.as_str()).unwrap_or(""),
                params.get("phone_number").and_then(|v| v.as_str()).unwrap_or(""))?;
            let duration = params.get("duration_seconds").and_then(|v| v.as_u64()).unwrap_or(0).clamp(1, 7200);
            let target = resolve_line_target(app, params).await?;
            let line = app.line_registry.get(&target.line_id).await.ok_or_else(|| anyhow!("automation_target_line_not_found"))?;
            let link = line.voice_access.operator_link();
            let events = link.subscribe_events(); // Subscribe BEFORE Start, including fast rejection.
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
            |_| async { Err(anyhow!("automation_call_rejected")) },
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
