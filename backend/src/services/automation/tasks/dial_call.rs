use crate::hardware::cellular::modem_manager::{
    hangup_call_on_modem, list_current_calls_for_modem, make_call_on_modem,
};
use crate::services::automation::target::resolve_modem_target;
use crate::services::automation::traits::AutomationTaskHandler;
use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use futures_util::future::{BoxFuture, FutureExt};
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

impl AutomationTaskHandler for DialCallHandler {
    fn task_type(&self) -> &'static str {
        "dial_call"
    }

    fn execute<'a>(
        &'a self,
        app: &'a AppState,
        params: &'a serde_json::Value,
    ) -> BoxFuture<'a, Result<()>> {
        let country = params
            .get("country_code")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        let number = params
            .get("phone_number")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        let duration = params
            .get("duration_seconds")
            .and_then(|value| value.as_u64())
            .unwrap_or(0)
            .clamp(1, 7_200);
        let target = params.clone();
        async move {
            let phone = normalize_phone(&country, &number)?;
            let target = resolve_modem_target(app, &target).await?;
            let call_path = make_call_on_modem(&app.dbus_conn, &target.modem_path, &phone)
                .await
                .context("定时拨号失败")?;
            let connection = app.dbus_conn.clone();
            let hangup_modem_path = target.modem_path.clone();
            let hangup = tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(duration)).await;
                match hangup_call_on_modem(&connection, &hangup_modem_path, &call_path).await {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        // A remote party may end the call before the configured
                        // hold time. Treat an already-absent call as a completed
                        // task, while preserving real hangup failures.
                        match list_current_calls_for_modem(&connection, &hangup_modem_path).await {
                            Ok(calls) if calls.calls.iter().all(|call| call.path != call_path) => {
                                Ok(())
                            }
                            _ => Err(error),
                        }
                    }
                }
            });
            hangup
                .await
                .context("自动挂机任务异常结束")?
                .context("自动挂机失败")?;
            info!(line_id = %target.line_id, phone = %phone, duration_seconds = duration, "automation dial call completed");
            Ok(())
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_phone;

    #[test]
    fn normalizes_country_code_and_body() {
        assert_eq!(
            normalize_phone("+86", "13800138000").unwrap(),
            "+8613800138000"
        );
        assert!(normalize_phone("86", "13800138000").is_err());
        assert!(normalize_phone("+86", "1380-0138000").is_err());
    }
}
