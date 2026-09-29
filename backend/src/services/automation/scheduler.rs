use crate::platform::config::{
    AutomationAction, AutomationTarget, AutomationTask, AutomationTrigger,
};
use crate::platform::db::beijing_sms_now_string;
use crate::services::automation::target::target_line_id;
use crate::services::automation::tasks::TaskRegistry;
use crate::services::automation::traits::AutomationExecutionReport;
use crate::services::notify::notification::AutomationEvent;
use crate::state::AppState;
use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDateTime, TimeZone, Timelike, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};

fn beijing_offset() -> FixedOffset {
    FixedOffset::east_opt(8 * 60 * 60).unwrap()
}

fn beijing_now() -> DateTime<FixedOffset> {
    Utc::now().with_timezone(&beijing_offset())
}

fn cron_field_matches(field: &str, value: u32, min: u32, max: u32) -> bool {
    field.split(',').any(|part| {
        let part = part.trim();
        let (base, step) = part.split_once('/').map_or((part, 1), |(base, step)| {
            (base, step.parse::<u32>().unwrap_or(0))
        });
        if step == 0 {
            return false;
        }
        let (start, end) = if base == "*" {
            (min, max)
        } else if let Some((a, b)) = base.split_once('-') {
            (a.parse().unwrap_or(max + 1), b.parse().unwrap_or(0))
        } else {
            let n = base.parse().unwrap_or(max + 1);
            (n, n)
        };
        value >= start && value <= end && (value - start).is_multiple_of(step)
    })
}

fn cron_matches(expression: &str, now: DateTime<FixedOffset>) -> bool {
    let fields = expression.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 5 {
        return false;
    }
    cron_field_matches(fields[0], now.minute(), 0, 59)
        && cron_field_matches(fields[1], now.hour(), 0, 23)
        && cron_field_matches(fields[2], now.day(), 1, 31)
        && cron_field_matches(fields[3], now.month(), 1, 12)
        && cron_field_matches(fields[4], now.weekday().number_from_sunday() - 1, 0, 6)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationStartResult {
    Started,
    AlreadyRunning,
}

struct AutomationRunGuard {
    active: Arc<Mutex<HashSet<String>>>,
    keys: Vec<String>,
}

impl AutomationRunGuard {
    fn try_acquire(active: Arc<Mutex<HashSet<String>>>, task: &AutomationTask) -> Option<Self> {
        let keys = automation_run_keys(task);
        let mut running = active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if keys.iter().any(|key| running.contains(key)) {
            return None;
        }
        running.extend(keys.iter().cloned());
        drop(running);
        Some(Self { active, keys })
    }
}

impl Drop for AutomationRunGuard {
    fn drop(&mut self) {
        let mut running = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for key in &self.keys {
            running.remove(key);
        }
    }
}

fn automation_run_keys(task: &AutomationTask) -> Vec<String> {
    let mut keys = vec![format!("task:{}", task.id.trim())];
    let target_key = match task.target.as_ref() {
        Some(AutomationTarget::ModemLine { line_id }) => {
            format!("line:{}", line_id.trim())
        }
        Some(AutomationTarget::StandaloneSimSlot { slot_id }) => {
            format!("reader:{}", slot_id.trim())
        }
        None => "device".to_string(),
    };
    keys.push(target_key);
    keys
}

pub fn spawn_automation_task(
    app: AppState,
    registry: Arc<TaskRegistry>,
    task: AutomationTask,
) -> AutomationStartResult {
    let Some(run_guard) =
        AutomationRunGuard::try_acquire(Arc::clone(&app.automation_running_scopes), &task)
    else {
        return AutomationStartResult::AlreadyRunning;
    };

    tokio::spawn(async move {
        let _run_guard = run_guard;
        if let Err(error) = execute_task(&app, registry.as_ref(), &task).await {
            error!(task_id = %task.id, ?error, "Automation task failed");
        }
    });
    AutomationStartResult::Started
}

pub fn spawn_automation_scheduler(app: AppState) {
    tokio::spawn(async move {
        info!("Starting automation center scheduler...");
        let registry = Arc::new(TaskRegistry::new());

        // 用于防止定点定时任务在同一分钟内重复运行
        // 键为 task_id，值为执行时的分钟数字符串，例如 "2026-06-10 04:00"
        let mut fixed_last_run: HashMap<String, String> = HashMap::new();

        loop {
            // 每隔 30 秒执行一次评估
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

            let config = app.config_manager.get_automation_config();
            if !config.enabled {
                continue;
            }

            for task in config.tasks {
                if !task.enabled {
                    continue;
                }

                // 判断是否应当触发
                let should_trigger = match &task.trigger {
                    AutomationTrigger::Fixed { weekdays, times } => {
                        let now = beijing_now();
                        let day_of_week = now.weekday().number_from_monday() as u8; // 1 to 7
                        let current_minute_str = now.format("%H:%M").to_string();

                        if weekdays.contains(&day_of_week) && times.contains(&current_minute_str) {
                            let unique_minute = now.format("%Y-%m-%d %H:%M").to_string();
                            // 检查是否在此分钟内已经运行过
                            if fixed_last_run.get(&task.id) == Some(&unique_minute) {
                                false
                            } else {
                                fixed_last_run.insert(task.id.clone(), unique_minute);
                                true
                            }
                        } else {
                            false
                        }
                    }
                    AutomationTrigger::Interval {
                        interval_value,
                        interval_unit,
                    } => {
                        // 查询上一次运行历史
                        let last_log = match app.database.get_last_log_for_task(&task.id) {
                            Ok(res) => res,
                            Err(e) => {
                                error!("Failed to query last log for task {}: {:?}", task.id, e);
                                None
                            }
                        };

                        match last_log {
                            Some(log) => {
                                if let Ok(parsed) = NaiveDateTime::parse_from_str(
                                    &log.created_at,
                                    "%Y-%m-%d %H:%M:%S",
                                ) {
                                    let last_run_time =
                                        beijing_offset().from_local_datetime(&parsed).unwrap();
                                    let now = beijing_now();

                                    let duration = match interval_unit.as_str() {
                                        "mins" => Duration::minutes(*interval_value as i64),
                                        "hours" => Duration::hours(*interval_value as i64),
                                        "days" => Duration::days(*interval_value as i64),
                                        _ => Duration::days(180), // 默认 Giffgaff 保号大间隔
                                    };

                                    now.signed_duration_since(last_run_time) >= duration
                                } else {
                                    true
                                }
                            }
                            None => true, // 从无历史记录，触发首次运行
                        }
                    }
                    AutomationTrigger::Cron { expression } => {
                        let now = beijing_now();
                        if !cron_matches(expression, now) {
                            false
                        } else {
                            let minute = now.format("%Y-%m-%d %H:%M").to_string();
                            if fixed_last_run.get(&task.id) == Some(&minute) {
                                false
                            } else {
                                fixed_last_run.insert(task.id.clone(), minute);
                                true
                            }
                        }
                    }
                };

                if should_trigger {
                    if spawn_automation_task(app.clone(), registry.clone(), task.clone())
                        == AutomationStartResult::AlreadyRunning
                    {
                        info!(
                            task_id = %task.id,
                            line_id = target_line_id(task.target.as_ref()).unwrap_or("device"),
                            "Skipped automation trigger because its task or target is already running"
                        );
                    }
                }
            }

            // 定期执行自动清理策略 (清理旧的自动化日志)
            let config_notifications = app.config_manager.get_notifications();
            let cleanup = config_notifications.log_cleanup;
            let retention_days = if cleanup.retention_days_enabled {
                Some(cleanup.retention_days)
            } else {
                None
            };
            let max_entries = if cleanup.max_entries_enabled {
                Some(cleanup.max_entries)
            } else {
                None
            };
            if retention_days.is_some() || max_entries.is_some() {
                let _ = app
                    .database
                    .cleanup_automation_logs(retention_days, max_entries);
            }
        }
    });
}

async fn execute_task(
    app: &AppState,
    registry: &TaskRegistry,
    task: &AutomationTask,
) -> Result<()> {
    info!("Triggering automation task: {} ({})", task.name, task.id);

    let task_type = match &task.action {
        AutomationAction::RestartBaseband => "restart_baseband",
        AutomationAction::RebootDevice { .. } => "reboot_device",
        AutomationAction::SendSms { .. } => "send_sms",
        AutomationAction::ConsumeData { .. } => "consume_data",
        AutomationAction::DialCall { .. } => "dial_call",
    };

    let handler = match registry.get(task_type) {
        Some(h) => h,
        None => {
            let err_msg = format!("No handler found for task type: {}", task_type);
            let _ = app.database.insert_automation_log(
                target_line_id(task.target.as_ref()),
                &task.id,
                &task.name,
                task_type,
                "failed",
                &err_msg,
            );
            return Err(anyhow::anyhow!(err_msg));
        }
    };

    let mut delay_secs = 0u64;
    // 参数转换
    let params = match &task.action {
        AutomationAction::RestartBaseband => serde_json::Value::Null,
        AutomationAction::RebootDevice { delay_seconds } => {
            serde_json::json!({ "delay_seconds": delay_seconds })
        }
        AutomationAction::SendSms {
            phone_number,
            content,
            random_delay_seconds,
            retry_limit,
        } => {
            delay_secs = u64::from(random_delay_seconds.unwrap_or(0));
            serde_json::json!({
                "phone_number": phone_number,
                "content": content,
                "random_delay_seconds": random_delay_seconds,
                "retry_limit": retry_limit
            })
        }
        AutomationAction::ConsumeData { bytes, unit } => {
            delay_secs = crate::services::automation::tasks::consume_data::execution_timeout_secs(
                *bytes, unit,
            );
            serde_json::json!({
                "bytes": bytes,
                "unit": unit,
                "target": &task.target,
            })
        }
        AutomationAction::DialCall {
            country_code,
            phone_number,
            duration_seconds,
        } => {
            delay_secs = u64::from(*duration_seconds).min(7_200);
            serde_json::json!({
                "country_code": country_code,
                "phone_number": phone_number,
                "duration_seconds": duration_seconds,
                "target": &task.target,
            })
        }
    };

    let params = if params.get("target").is_some() {
        params
    } else {
        let mut params = params;
        if let Some(target) = &task.target {
            params["target"] = serde_json::to_value(target)?;
        }
        params
    };

    // 执行任务并控制超时（基准60秒 + 动作需要的等待时间）
    let timeout_seconds = 60 + delay_secs;
    let result = tokio::time::timeout(
        tokio::time::Duration::from_secs(timeout_seconds),
        handler.execute_report(app, &params),
    )
    .await;

    let (status, detail) = task_outcome(result.ok(), timeout_seconds, &task.action);
    if status == "failed" {
        warn!(task_id = %task.id, task_type, detail = %detail, "Automation execution failed");
    }

    // 1. 写入 SQLite 日志表
    let _ = app.database.insert_automation_log(
        target_line_id(task.target.as_ref()),
        &task.id,
        &task.name,
        task_type,
        status,
        &detail,
    );

    // 2. 发出通知事件
    let event = AutomationEvent {
        line_id: target_line_id(task.target.as_ref()).map(str::to_string),
        task_id: task.id.clone(),
        task_name: task.name.clone(),
        task_type: task_type.to_string(),
        status: status.to_string(),
        message: detail.clone(),
        timestamp: beijing_sms_now_string(),
    };

    if let Err(e) = app
        .notification_sender
        .forward_automation_event(&event)
        .await
    {
        warn!("Failed to forward automation notification event: {:?}", e);
    }

    Ok(())
}

fn dial_failure_summary(error: &anyhow::Error) -> String {
    // A provider error can embed phone numbers, URLs or authentication values.
    // Preserve actionable stage/codes, not its arbitrary text, in DB/notifications.
    const CODES: &[&str] = &[
        "voice_vowifi_only_required",
        "voice_registered_home_required",
        "voice_call_binding_changed",
        "voice_access_router_unavailable",
        "voice_access_router_timeout",
        "voice_call_already_pending",
        "voice_ims_access_unavailable",
        "voice_modem_busy_or_unknown",
        "voice_at_call_ownership_unverified",
        "vowifi_voice_disabled",
        "vowifi_voice_ready_not_reached",
        "ims_operator_channel_unavailable",
        "line_not_found",
        "line_not_present",
        "line_disabled",
        "cs_blocked_by_airplane_mode",
        "airplane_mode_state_unavailable",
        "modem_call_failed",
        "call_not_found_on_selected_line",
        "automation_target_line_required",
        "automation_target_refresh_failed",
        "automation_target_line_not_found",
        "automation_target_line_not_present",
        "automation_target_line_disabled",
        "automation_target_reader_slot_not_found",
        "automation_call_ownership_unverified",
        "automation_call_rejected",
        "automation_call_access_unavailable",
        "automation_call_ended_before_duration",
        "automation_call_ended_before_answer",
        "automation_call_ended_without_remote_evidence",
        "automation_call_delivery_unconfirmed",
        "automation_call_local_failure",
        "automation_call_observation_lost",
        "automation_call_cancelled",
        "automation_call_cleanup_unavailable",
        "automation_call_task_failed",
        "org.freedesktop.DBus.Error.UnknownMethod",
        "org.freedesktop.DBus.Error.UnknownInterface",
        "org.freedesktop.ModemManager1.Error.Core.Unsupported",
        "org.freedesktop.DBus.Error.NoReply",
    ];
    let mut parts: Vec<String> = Vec::new();
    for cause in error.chain().take(8) {
        let text: String = cause.to_string().chars().take(8192).collect();
        for stage in [
            "定时拨号失败",
            "自动挂机失败",
            "国家区号格式必须为 +数字",
            "手机号码主体只能包含数字",
        ] {
            if text == stage && !parts.iter().any(|part| part == stage) {
                parts.push(stage.into());
            }
        }
        for token in text.split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.')))
        {
            if CODES.contains(&token) && !parts.iter().any(|part| part == token) {
                parts.push(token.into());
            }
        }
    }
    if parts.is_empty() {
        parts.push("automation_call_failed_detail_redacted".into());
    }
    format!("执行失败: {}", parts.join(": "))
}

/// One outcome supplies both the database and notification. Keep the dial
/// stage AND its cause: Display alone drops anyhow's context chain. Do not
/// include the dial target in forwarded failure diagnostics.
fn task_outcome(
    result: Option<Result<AutomationExecutionReport>>,
    timeout_seconds: u64,
    action: &AutomationAction,
) -> (&'static str, String) {
    match result {
        Some(Ok(report)) => ("success", report.detail()),
        Some(Err(error)) => {
            let detail = if let AutomationAction::DialCall {
                country_code,
                phone_number,
                ..
            } = action
            {
                let mut detail = dial_failure_summary(&error);
                for number in [
                    format!("{}{}", country_code.trim(), phone_number.trim()),
                    phone_number.trim().to_string(),
                ] {
                    if !number.is_empty() {
                        detail = detail.replace(&number, "[number-redacted]");
                    }
                }
                detail
            } else {
                format!("执行失败: {error}")
            };
            (
                "failed",
                detail
                    .chars()
                    .take(4096)
                    .map(|ch| if ch.is_control() { ' ' } else { ch })
                    .collect(),
            )
        }
        None => ("failed", format!("执行超时 (超过{timeout_seconds}秒限制)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_preserves_peer_no_answer_but_cleanup_failure_still_wins() {
        use super::super::tasks::dial_outcome::{DialOutcome, DialReport};
        let report = DialReport {
            outcome: DialOutcome::PeerNoAnswer,
            ringing_observed: true,
            answered_observed: false,
            sip_status: Some(408),
            q850_cause: Some(31),
        };
        let action = AutomationAction::DialCall {
            country_code: "+1".into(),
            phone_number: "2025550100".into(),
            duration_seconds: 60,
        };
        let (status, detail) = task_outcome(
            Some(Ok(AutomationExecutionReport::Dial(report))),
            120,
            &action,
        );
        assert_eq!(status, "success");
        assert!(detail.contains("对方未接听"));
        assert!(detail.contains("SIP=408"));
        assert!(detail.contains("answered_observed=false"));
        assert!(!detail.contains("2025550100"));
        let cleanup =
            anyhow::anyhow!("automation_call_cleanup_unavailable").context("自动挂机失败");
        assert_eq!(task_outcome(Some(Err(cleanup)), 120, &action).0, "failed");
        assert_eq!(task_outcome(None, 120, &action).0, "failed");
    }

    #[test]
    fn dial_outcome_preserves_stage_and_cause_without_the_number() {
        let action = AutomationAction::DialCall {
            country_code: "+86".into(),
            phone_number: "13800138000".into(),
            duration_seconds: 30,
        };
        for stage in ["定时拨号失败", "自动挂机失败"] {
            let cause = anyhow::anyhow!(
                "voice_ims_access_unavailable;voice_vowifi_only_required:+8613800138000"
            )
            .context(stage);
            let (status, detail) = task_outcome(Some(Err(cause)), 90, &action);
            assert_eq!(status, "failed");
            assert!(detail.contains(stage));
            assert!(detail.contains("voice_vowifi_only_required"));
            assert!(!detail.contains("13800138000"));
        }
        assert_eq!(
            task_outcome(None, 90, &action),
            ("failed", "执行超时 (超过90秒限制)".into())
        );
        assert_eq!(
            task_outcome(Some(Ok(AutomationExecutionReport::Completed)), 90, &action).0,
            "success"
        );
    }

    #[test]
    fn dial_outcome_bounds_and_flattens_untrusted_errors() {
        let action = AutomationAction::DialCall {
            country_code: "+1".into(),
            phone_number: "2025550100".into(),
            duration_seconds: 10,
        };
        let error =
            anyhow::anyhow!(format!("bad\nreply\r\n{}", "x".repeat(5000))).context("定时拨号失败");
        let (_, detail) = task_outcome(Some(Err(error)), 70, &action);
        assert!(detail.chars().count() <= 4096);
        assert!(!detail.chars().any(char::is_control));
        let error = anyhow::anyhow!("voice_registered_home_required: cookie=private-value https://name:password@example.invalid/ phone=12025550101").context("定时拨号失败");
        let (_, detail) = task_outcome(Some(Err(error)), 70, &action);
        assert!(detail.contains("voice_registered_home_required"));
        for secret in [
            "cookie",
            "private-value",
            "password",
            "example.invalid",
            "12025550101",
        ] {
            assert!(!detail.contains(secret));
        }
    }

    fn line_task(task_id: &str, line_id: &str) -> AutomationTask {
        AutomationTask {
            id: task_id.to_string(),
            name: task_id.to_string(),
            enabled: true,
            trigger: AutomationTrigger::Interval {
                interval_value: 1,
                interval_unit: "hours".to_string(),
            },
            target: Some(AutomationTarget::ModemLine {
                line_id: line_id.to_string(),
            }),
            action: AutomationAction::RestartBaseband,
        }
    }

    #[test]
    fn matches_five_field_cron_with_steps_and_ranges() {
        let now = beijing_offset()
            .with_ymd_and_hms(2026, 7, 19, 18, 30, 0)
            .unwrap();
        assert!(cron_matches("*/15 18 * * 0", now));
        assert!(cron_matches("30 18 19 7 0", now));
        assert!(!cron_matches("31 18 * * *", now));
    }

    #[test]
    fn automation_run_guard_serializes_each_task_and_target_only() {
        let active = Arc::new(Mutex::new(HashSet::new()));
        let line_a =
            AutomationRunGuard::try_acquire(Arc::clone(&active), &line_task("task-a", "line-a"))
                .expect("first task reserves line A");

        assert!(AutomationRunGuard::try_acquire(
            Arc::clone(&active),
            &line_task("task-a", "line-b"),
        )
        .is_none());
        assert!(AutomationRunGuard::try_acquire(
            Arc::clone(&active),
            &line_task("task-b", "line-a"),
        )
        .is_none());

        let line_b =
            AutomationRunGuard::try_acquire(Arc::clone(&active), &line_task("task-b", "line-b"))
                .expect("a different line can run concurrently");
        drop(line_a);
        let line_a_again =
            AutomationRunGuard::try_acquire(Arc::clone(&active), &line_task("task-c", "line-a"))
                .expect("line A is reusable after completion");

        drop(line_a_again);
        drop(line_b);
        assert!(active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
    }
}
