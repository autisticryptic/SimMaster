use crate::api::handlers::{start_line_data_runtime, stop_line_data_runtime};
use crate::hardware::cellular::data_proxy::DataProxyStatus;
use crate::platform::config::{LineDataProxyConfig, LineProfileConfig};
use crate::services::automation::target::resolve_modem_target;
use crate::services::automation::traits::AutomationTaskHandler;
use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use futures_util::future::{BoxFuture, FutureExt};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use tracing::{info, warn};

pub struct ConsumeDataHandler;

const DATA_CONSUMPTION_ENDPOINT: &str = "https://speed.cloudflare.com/__down";
const DATA_CONSUMPTION_HOST: &str = "speed.cloudflare.com";

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

pub(crate) fn execution_timeout_secs(value: u64, unit: &str) -> u64 {
    let amount = requested_bytes(value, unit).unwrap_or(0);
    // Allow roughly one MiB/s, with enough fixed time for bearer setup and a
    // ceiling that prevents one unhealthy endpoint from occupying a line
    // indefinitely. The scheduler adds a separate cleanup margin.
    amount
        .div_ceil(1024 * 1024)
        .saturating_add(60)
        .clamp(120, 1_800)
}

fn automation_proxy_config(
    configured: &LineDataProxyConfig,
    temporary: bool,
) -> LineDataProxyConfig {
    if !temporary {
        return configured.clone();
    }
    LineDataProxyConfig {
        listen_ip: "127.0.0.1".to_string(),
        listen_port: 0,
        username: String::new(),
        password: String::new(),
    }
}

fn automation_runtime_profile(configured: &LineProfileConfig) -> (LineProfileConfig, bool) {
    let temporary = !configured.data_connection_enabled;
    let mut runtime = configured.clone();
    // The persisted switch describes the state to restore after this task. The
    // runtime copy must explicitly request data so future start-path guards do
    // not accidentally turn a disabled keep-alive task into a no-op.
    runtime.data_connection_enabled = true;
    runtime.data_proxy = automation_proxy_config(&configured.data_proxy, temporary);
    (runtime, temporary)
}

fn local_proxy_url(status: &DataProxyStatus) -> Result<String> {
    if !status.running {
        return Err(anyhow!("目标线路的流量代理未运行"));
    }
    let listen_ip = status
        .listen_ip
        .as_deref()
        .ok_or_else(|| anyhow!("目标线路的流量代理没有监听地址"))?
        .parse::<IpAddr>()
        .context("目标线路的流量代理监听地址无效")?;
    let connect_ip = match listen_ip {
        IpAddr::V4(address) if address.is_unspecified() => {
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        }
        IpAddr::V6(address) if address.is_unspecified() => {
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        }
        address => address,
    };
    let port = status
        .port
        .ok_or_else(|| anyhow!("目标线路的流量代理没有监听端口"))?;
    Ok(match connect_ip {
        IpAddr::V4(address) => format!("socks5://{address}:{port}"),
        IpAddr::V6(address) => format!("socks5://[{address}]:{port}"),
    })
}

fn data_consumption_addresses() -> [SocketAddr; 2] {
    // DATA6 is intentionally isolated from the host's default DNS route. Give
    // SOCKS5 numeric anycast destinations while TLS keeps validating the
    // Cloudflare service hostname.
    [
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(172, 66, 0, 218), 443)),
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(162, 159, 140, 220), 443)),
    ]
}

fn proxy_client(
    proxy_url: &str,
    config: &LineDataProxyConfig,
    timeout_secs: u64,
) -> Result<reqwest::Client> {
    let mut proxy = reqwest::Proxy::all(proxy_url).context("创建线路代理配置失败")?;
    if !config.username.is_empty() {
        proxy = proxy.basic_auth(&config.username, &config.password);
    }
    reqwest::Client::builder()
        .proxy(proxy)
        .resolve_to_addrs(DATA_CONSUMPTION_HOST, &data_consumption_addresses())
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .context("创建线路代理下载客户端失败")
}

fn data_consumption_url(amount: u64) -> String {
    format!("{DATA_CONSUMPTION_ENDPOINT}?bytes={amount}")
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
            let timeout_secs = execution_timeout_secs(value, unit);
            let target = resolve_modem_target(app, &target).await?;
            let line = app
                .line_registry
                .get(&target.line_id)
                .await
                .ok_or_else(|| anyhow!("automation_target_line_not_found"))?;
            let profile = app.config_manager.get_line_profile(&target.line_id);
            if profile.airplane_mode_enabled {
                return Err(anyhow!("飞行模式已启用，无法建立移动数据连接"));
            }

            // A disabled persistent switch must not prevent a keep-alive task.
            // In that case open a loopback-only proxy for this run and restore
            // the disabled state afterwards instead of exposing the saved
            // listener configuration temporarily.
            let (runtime_profile, temporary_runtime) = automation_runtime_profile(&profile);

            let task_result = match tokio::time::timeout(
                std::time::Duration::from_secs(timeout_secs),
                async {
                    start_line_data_runtime(app, &line, &runtime_profile)
                        .await
                        .map_err(anyhow::Error::msg)
                        .context("目标线路的数据承载或代理启动失败")?;
                    let proxy_status = line.data_proxy.status().await;
                    let proxy_url = local_proxy_url(&proxy_status)?;
                    let client = proxy_client(
                        &proxy_url,
                        &runtime_profile.data_proxy,
                        timeout_secs,
                    )?;
                    // SOCKS5 carries a numeric edge address through the isolated
                    // cellular interface, while HTTPS retains the service hostname
                    // for certificate validation and exact-size response integrity.
                    let mut response = client
                        .get(data_consumption_url(amount))
                        .send()
                        .await
                        .context("线路代理流量请求失败")?;
                    if !response.status().is_success() {
                        return Err(anyhow!("蜂窝流量服务返回 {}", response.status()));
                    }
                    let mut downloaded = 0u64;
                    while let Some(chunk) =
                        response.chunk().await.context("读取蜂窝流量响应失败")?
                    {
                        downloaded = downloaded.saturating_add(chunk.len() as u64);
                        if downloaded > amount {
                            return Err(anyhow!(
                                "蜂窝流量响应大小不符：期望 {amount} Byte，实际超过 {downloaded} Byte"
                            ));
                        }
                    }
                    if downloaded != amount {
                        return Err(anyhow!(
                            "蜂窝流量响应大小不符：期望 {amount} Byte，实际 {downloaded} Byte"
                        ));
                    }
                    info!(
                        bytes = downloaded,
                        proxy = proxy_url,
                        line_id = target.line_id,
                        modem_path = target.modem_path,
                        "automation proxied cellular data consumption completed"
                    );
                    Ok(())
                },
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(anyhow!("蜂窝流量任务执行超时")),
            };

            let cleanup_result = if temporary_runtime {
                let current_profile = app.config_manager.get_line_profile(&target.line_id);
                if current_profile.data_connection_enabled
                    && !current_profile.airplane_mode_enabled
                {
                    start_line_data_runtime(app, &line, &current_profile)
                        .await
                        .map_err(anyhow::Error::msg)
                        .context("流量任务结束后恢复已启用的数据代理失败")
                } else {
                    stop_line_data_runtime(app, &line).await;
                    Ok(())
                }
            } else {
                Ok(())
            };
            app.line_registry.flush_data_traffic().await;

            if let Err(error) = cleanup_result {
                warn!(line_id = target.line_id, error = %error, "Failed to restore line data state after automation task");
                task_result?;
                return Err(error);
            }
            task_result
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        automation_proxy_config, automation_runtime_profile, data_consumption_addresses,
        data_consumption_url, execution_timeout_secs, local_proxy_url, requested_bytes,
    };
    use crate::hardware::cellular::data_proxy::DataProxyStatus;
    use crate::platform::config::{LineDataProxyConfig, LineProfileConfig};

    #[test]
    fn converts_small_data_units_without_rounding() {
        assert_eq!(requested_bytes(100, "bytes").unwrap(), 100);
        assert_eq!(requested_bytes(1, "kb").unwrap(), 1024);
        assert_eq!(requested_bytes(2, "mb").unwrap(), 2 * 1024 * 1024);
    }

    #[test]
    fn cellular_download_uses_the_https_exact_size_endpoint() {
        assert_eq!(
            data_consumption_url(65_536),
            "https://speed.cloudflare.com/__down?bytes=65536"
        );
        assert_eq!(
            data_consumption_addresses()[0].to_string(),
            "172.66.0.218:443"
        );
    }

    #[test]
    fn execution_timeout_scales_and_stays_bounded() {
        assert_eq!(execution_timeout_secs(1, "mb"), 120);
        assert_eq!(execution_timeout_secs(256, "mb"), 316);
        assert_eq!(execution_timeout_secs(1_024, "mb"), 1_084);
        assert_eq!(execution_timeout_secs(u64::MAX, "mb"), 120);
    }

    #[test]
    fn disabled_data_uses_a_private_ephemeral_proxy() {
        let configured = LineDataProxyConfig {
            listen_ip: "0.0.0.0".to_string(),
            listen_port: 1080,
            username: "user".to_string(),
            password: "secret".to_string(),
        };
        assert_eq!(automation_proxy_config(&configured, false), configured);
        assert_eq!(
            automation_proxy_config(&configured, true),
            LineDataProxyConfig {
                listen_ip: "127.0.0.1".to_string(),
                listen_port: 0,
                username: String::new(),
                password: String::new(),
            }
        );
    }

    #[test]
    fn disabled_persistent_switch_still_requests_a_temporary_runtime() {
        let mut configured = LineProfileConfig::for_line("line-a");
        configured.data_connection_enabled = false;
        configured.data_proxy.listen_ip = "0.0.0.0".to_string();
        configured.data_proxy.listen_port = 1080;
        configured.data_proxy.username = "user".to_string();
        configured.data_proxy.password = "secret".to_string();

        let (runtime, temporary) = automation_runtime_profile(&configured);
        assert!(temporary);
        assert!(runtime.data_connection_enabled);
        assert_eq!(runtime.data_proxy.listen_ip, "127.0.0.1");
        assert_eq!(runtime.data_proxy.listen_port, 0);
        assert!(runtime.data_proxy.username.is_empty());
    }

    #[test]
    fn wildcard_proxy_listener_is_reached_through_loopback() {
        let status = DataProxyStatus {
            running: true,
            listen_ip: Some("0.0.0.0".to_string()),
            port: Some(12345),
            ..DataProxyStatus::default()
        };
        assert_eq!(
            local_proxy_url(&status).unwrap(),
            "socks5://127.0.0.1:12345"
        );
    }
}
