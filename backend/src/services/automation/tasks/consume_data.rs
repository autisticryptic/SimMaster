use crate::hardware::cellular::modem_manager::{connect_data_via_modem, data_interface_for_modem};
use crate::platform::utils::read_network_interfaces;
use crate::services::automation::target::resolve_modem_target;
use crate::services::automation::traits::AutomationTaskHandler;
use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use futures_util::future::{BoxFuture, FutureExt};
use std::net::IpAddr;
use tracing::info;

pub struct ConsumeDataHandler;

fn requested_bytes(value: u64, unit: &str) -> Result<u64> {
    if value == 0 {
        return Err(anyhow!("流量大小必须大于 0"));
    }
    let multiplier = match unit {
        "auto" | "bytes" => 1u64,
        "kb" => 1024,
        "mb" => 1024 * 1024,
        _ => return Err(anyhow!("不支持的流量单位")),
    };
    let amount = value
        .checked_mul(multiplier)
        .ok_or_else(|| anyhow!("流量大小超出范围"))?;
    if amount > 1024 * 1024 * 1024 {
        return Err(anyhow!("单次自动化流量不能超过 1 GiB"));
    }
    Ok(amount)
}

fn source_ip_for_interface(
    interfaces: &[crate::api::models::NetworkInterfaceInfo],
    interface_name: &str,
) -> Option<IpAddr> {
    interfaces
        .iter()
        .filter(|interface| interface.name == interface_name && interface.status != "down")
        .flat_map(|interface| interface.ip_addresses.iter())
        .filter_map(|address| address.address.parse::<IpAddr>().ok())
        .find(|address| address.is_ipv4())
        .or_else(|| {
            interfaces
                .iter()
                .filter(|interface| interface.name == interface_name && interface.status != "down")
                .flat_map(|interface| interface.ip_addresses.iter())
                .filter_map(|address| address.address.parse::<IpAddr>().ok())
                .next()
        })
}

impl AutomationTaskHandler for ConsumeDataHandler {
    fn task_type(&self) -> &'static str {
        "consume_data"
    }

    fn execute<'a>(
        &'a self,
        app: &'a AppState,
        params: &'a serde_json::Value,
    ) -> BoxFuture<'a, Result<()>> {
        let value = params
            .get("bytes")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let unit = params
            .get("unit")
            .and_then(|value| value.as_str())
            .unwrap_or("auto");
        let target = params.clone();
        async move {
            let amount = requested_bytes(value, unit)?;
            let target = resolve_modem_target(app, &target).await?;
            let profile = app.config_manager.get_line_profile(&target.line_id);
            let apn = app.config_manager.get_line_apn_config(&target.line_id);
            connect_data_via_modem(
                &app.dbus_conn,
                &target.modem_path,
                profile.roaming_allowed,
                Some(&apn),
            )
            .await
            .map_err(anyhow::Error::msg)
            .context("移动数据连接建立失败")?;

            let mut source_ip = None;
            let mut selected_interface = None;
            for _ in 0..10 {
                selected_interface = data_interface_for_modem(&app.dbus_conn, &target.modem_path)
                    .await
                    .ok()
                    .flatten();
                let interfaces = read_network_interfaces(Some(&app.dbus_conn))
                    .await
                    .unwrap_or_default();
                source_ip = selected_interface
                    .as_deref()
                    .and_then(|name| source_ip_for_interface(&interfaces, name));
                if source_ip.is_some() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            let selected_interface =
                selected_interface.ok_or_else(|| anyhow!("未找到目标线路的数据接口"))?;
            let source_ip = source_ip.ok_or_else(|| anyhow!("未找到目标线路的数据源地址"))?;
            let endpoint = format!("https://speed.cloudflare.com/__down?bytes={amount}");
            let client = reqwest::Client::builder()
                .local_address(source_ip)
                .timeout(std::time::Duration::from_secs(45))
                .build()
                .context("创建蜂窝数据专用客户端失败")?;
            let response = client
                .get(endpoint)
                .send()
                .await
                .context("蜂窝流量请求失败")?;
            if !response.status().is_success() {
                return Err(anyhow!("蜂窝流量服务返回 {}", response.status()));
            }
            let payload = response.bytes().await.context("读取蜂窝流量响应失败")?;
            if payload.len() as u64 != amount {
                return Err(anyhow!(
                    "蜂窝流量响应大小不符：期望 {amount} Byte，实际 {} Byte",
                    payload.len()
                ));
            }
            info!(
                bytes = payload.len(),
                source_ip = %source_ip,
                interface = selected_interface,
                line_id = target.line_id,
                "automation cellular data consumption completed"
            );
            Ok(())
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::{requested_bytes, source_ip_for_interface};
    use crate::api::models::{IpAddress, NetworkInterfaceInfo};

    fn interface(name: &str, address: &str) -> NetworkInterfaceInfo {
        NetworkInterfaceInfo {
            name: name.to_string(),
            status: "up".to_string(),
            is_wireless: false,
            is_cellular: true,
            is_default_ipv4: false,
            is_default_ipv6: false,
            mac_address: None,
            mtu: 1500,
            ip_addresses: vec![IpAddress {
                address: address.to_string(),
                prefix_len: 24,
                ip_type: "IPv4".to_string(),
                scope: "global".to_string(),
            }],
            rx_bytes: 0,
            tx_bytes: 0,
            rx_packets: 0,
            tx_packets: 0,
            rx_errors: 0,
            tx_errors: 0,
        }
    }

    #[test]
    fn converts_small_data_units_without_rounding() {
        assert_eq!(requested_bytes(100, "bytes").unwrap(), 100);
        assert_eq!(requested_bytes(1, "kb").unwrap(), 1024);
        assert_eq!(requested_bytes(2, "mb").unwrap(), 2 * 1024 * 1024);
    }

    #[test]
    fn selects_only_the_requested_lines_data_interface() {
        let interfaces = vec![
            interface("wwan-line-a", "10.1.0.2"),
            interface("wwan-line-b", "10.2.0.2"),
        ];
        assert_eq!(
            source_ip_for_interface(&interfaces, "wwan-line-b")
                .expect("line B source address")
                .to_string(),
            "10.2.0.2"
        );
        assert!(source_ip_for_interface(&interfaces, "wwan-line-c").is_none());
    }
}
