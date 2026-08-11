use crate::hardware::cellular::modem_manager::restart_baseband_via_modem;
use crate::services::automation::target::resolve_modem_target;
use crate::services::automation::traits::AutomationTaskHandler;
use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use futures_util::future::{BoxFuture, FutureExt};

pub struct BasebandRebootHandler;

impl AutomationTaskHandler for BasebandRebootHandler {
    fn task_type(&self) -> &'static str {
        "restart_baseband"
    }

    fn execute<'a>(
        &'a self,
        app: &'a AppState,
        params: &'a serde_json::Value,
    ) -> BoxFuture<'a, Result<()>> {
        async move {
            let target = resolve_modem_target(app, params).await?;
            let profile = app.config_manager.get_line_profile(&target.line_id);
            let apn_config = app.config_manager.get_line_apn_config(&target.line_id);

            restart_baseband_via_modem(
                &app.dbus_conn,
                &target.line_id,
                &target.modem_path,
                profile.data_connection_enabled,
                profile.roaming_allowed,
                Some(apn_config),
            )
            .await
            .map_err(|e| anyhow!("{}", e))
            .context("重启基带失败")?;

            Ok(())
        }
        .boxed()
    }
}
