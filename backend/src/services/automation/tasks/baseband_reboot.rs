use crate::services::automation::target::resolve_modem_path;
use crate::services::automation::traits::AutomationTaskHandler;
use crate::hardware::cellular::modem_manager::restart_baseband_via_modem;
use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use futures_util::future::{BoxFuture, FutureExt};
use std::sync::atomic::Ordering;

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
            let modem_path = resolve_modem_path(app, params).await?;
            let profile = app
                .line_registry
                .for_modem_path(&modem_path)
                .await
                .map(|line| app.config_manager.get_line_profile(&line.binding().line_id));
            let auto_connect_data = profile
                .as_ref()
                .map(|profile| profile.data_connection_enabled)
                .unwrap_or_else(|| !app.data_user_disabled.load(Ordering::SeqCst));
            let allow_roaming = profile
                .as_ref()
                .map(|profile| profile.roaming_allowed)
                .unwrap_or_else(|| app.config_manager.get_roaming_allowed());
            let apn_config = app.config_manager.get_apn_config();

            restart_baseband_via_modem(
                &app.dbus_conn,
                &modem_path,
                auto_connect_data,
                allow_roaming,
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
