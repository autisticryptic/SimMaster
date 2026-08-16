use crate::api::handlers::{hangup_call_for_automation, start_call_for_automation};
use crate::services::automation::target::resolve_line_target;
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
            let target = resolve_line_target(app, &target).await?;
            let (_, call_path, transport) = start_call_for_automation(app, &target.line_id, &phone)
                .await
                .map_err(anyhow::Error::msg)
                .context("定时拨号失败")?;
            tokio::time::sleep(std::time::Duration::from_secs(duration)).await;
            hangup_call_for_automation(app, &target.line_id, &call_path)
                .await
                .map_err(anyhow::Error::msg)
                .context("自动挂机失败")?;
            info!(line_id = %target.line_id, phone = %phone, duration_seconds = duration, transport, "automation dial call completed");
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
