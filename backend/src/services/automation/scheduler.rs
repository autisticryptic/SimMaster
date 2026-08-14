use crate::platform::config::{
    AutomationAction, AutomationTarget, AutomationTask, AutomationTrigger,
};
use crate::platform::db::beijing_sms_now_string;
use crate::services::automation::target::target_line_id;
use crate::services::automation::tasks::TaskRegistry;
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
    let result = tokio::time::timeout(
        tokio::time::Duration::from_secs(60 + delay_secs),
        handler.execute(app, &params),
    )
    .await;

    let (status, detail) = match result {
        Ok(Ok(_)) => ("success", "执行成功".to_string()),
        Ok(Err(e)) => ("failed", format!("执行失败: {}", e)),
        Err(_) => ("failed", "执行超时 (超过60秒限制)".to_string()),
    };

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

#[cfg(test)]
mod tests {
    use super::*;

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
