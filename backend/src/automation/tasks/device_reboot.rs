use crate::automation::traits::AutomationTaskHandler;
use crate::handlers::run_safe_os_reboot_sequence;
use crate::state::AppState;
use anyhow::Result;
use futures_util::future::{BoxFuture, FutureExt};

pub struct DeviceRebootHandler;

impl AutomationTaskHandler for DeviceRebootHandler {
    fn task_type(&self) -> &'static str {
        "reboot_device"
    }

    fn execute<'a>(
        &'a self,
        app: &'a AppState,
        params: &'a serde_json::Value,
    ) -> BoxFuture<'a, Result<()>> {
        let delay_seconds = params
            .get("delay_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as u32;

        let system_events = app.system_event_emitter.clone();

        async move {
            // 启动安全重启序列，它在后台异步执行（带有延迟）
            tokio::spawn(async move {
                run_safe_os_reboot_sequence(delay_seconds, system_events).await;
            });

            Ok(())
        }
        .boxed()
    }
}
