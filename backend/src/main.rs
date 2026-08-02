//! SimAdmin - Debian SIM Management Service
//!
//! A backend service for managing Debian-based modem and SIM devices.
//! Built with Rust + Axum + zbus.
//!

use anyhow::Result;
use axum::{
    extract::DefaultBodyLimit,
    http::{StatusCode, Uri},
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use clap::{Args as ClapArgs, Parser, Subcommand};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use zbus::Connection;

mod api;
mod connectivity;
mod hardware;
mod platform;
mod services;
mod state;

use api::handlers::*;
use hardware::cellular::modem_manager::{ensure_nm_modem_profile, set_airplane_mode_for_modem};
use platform::config::{get_default_config_path, ConfigManager};
use platform::db::Database;
use services::network::device_network::DdnsManager;
use services::notify::notification::NotificationSender;
use services::notify::notification_queue::*;
use hardware::sim::esim::EsimSupervisor;
use state::{AppState, AppStateDependencies};
use services::system::system_event::{
    codes as system_event_codes, severity as system_event_severity, status as system_event_status,
    SystemEventEmitter,
};

/// How often per-line proxy traffic counters are written to disk. Short enough
/// that an unexpected power loss costs little, long enough not to write on
/// every request.
const LINE_TRAFFIC_FLUSH_SECS: u64 = 30;

/// 获取二进制文件同级目录下的 www 目录路径
fn get_www_dir() -> PathBuf {
    // 获取当前可执行文件的路径
    let exe_path = std::env::current_exe().expect("Failed to get executable path");

    // 获取可执行文件所在目录
    let exe_dir = exe_path
        .parent()
        .expect("Failed to get executable directory");

    // 拼接 www 目录
    exe_dir.join("www")
}

fn get_data_db_path() -> PathBuf {
    std::env::current_exe()
        .expect("Failed to get executable path")
        .parent()
        .expect("Failed to get executable directory")
        .join("data.db")
}

/// SPA fallback handler - 对于所有前端路由返回 index.html
async fn spa_fallback(uri: Uri) -> Response {
    let path = uri.path();

    // 如果是 API 路由，返回 404（不应该走到这里，但作为保险）
    if path.starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "API endpoint not found").into_response();
    }

    // 获取 www 目录的绝对路径
    let www_dir = get_www_dir();

    // 构建请求文件的完整路径
    let requested_path = if path == "/" { "/index.html" } else { path };
    let file_path = www_dir.join(requested_path.trim_start_matches('/'));

    // 如果文件存在，返回文件内容
    if let Ok(content) = tokio::fs::read(&file_path).await {
        // 根据文件扩展名设置正确的 Content-Type
        let content_type = match file_path.extension().and_then(|ext| ext.to_str()) {
            Some("html") => "text/html; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("js") => "application/javascript; charset=utf-8",
            Some("json") => "application/json",
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("svg") => "image/svg+xml",
            Some("ico") => "image/x-icon",
            _ => "application/octet-stream",
        };

        return (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, content_type)],
            content,
        )
            .into_response();
    }

    // 如果文件不存在，返回 index.html（SPA 路由）
    let index_path = www_dir.join("index.html");
    match tokio::fs::read(&index_path).await {
        Ok(content) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            content,
        )
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            format!(
                "index.html not found at {:?}. Please build the frontend first.",
                index_path
            ),
        )
            .into_response(),
    }
}

/// 确保 ModemManager 开启 debug 提权模式，但为了防止日志爆炸，
/// 启动后立刻通过 ExecStartPost 将日志级别降回 INFO。
/// 这完美绕过了 Modem.Command 的 Unauthorized 限制，同时保持系统纯净。
fn ensure_modemmanager_debug_override() {
    let override_dir = "/etc/systemd/system/ModemManager.service.d";
    // `zz-` must sort after vendor drop-ins such as `mobile-tweaks.conf`, which
    // also reset ExecStart on the Qualcomm image.
    let override_file = "/etc/systemd/system/ModemManager.service.d/zz-simadmin-debug.conf";
    let legacy_override = "/etc/systemd/system/ModemManager.service.d/99-simadmin-debug.conf";

    let desired_content = "\
# SimAdmin: enable ModemManager debug mode so that Modem.Command D-Bus
# interface is available for AT+CRSM (SIM file read) and AT+CSCA? (SMSC query).
# We immediately set logging level back to INFO via ExecStartPost to prevent log spam.
[Service]
ExecStart=
ExecStart=/usr/sbin/ModemManager --debug
ExecStartPost=-/usr/bin/busctl call org.freedesktop.ModemManager1 /org/freedesktop/ModemManager1 org.freedesktop.ModemManager1 SetLogging s \"INFO\"
";

    let needs_update = match std::fs::read_to_string(override_file) {
        Ok(content) => content != desired_content,
        Err(_) => true,
    };

    if needs_update {
        tracing::info!("Applying ModemManager debug override (with silent logging)...");
        let _ = std::fs::create_dir_all(override_dir);
        if let Err(e) = std::fs::write(override_file, desired_content) {
            tracing::warn!("Failed to write MM debug override: {}", e);
            return;
        }
        let _ = std::fs::remove_file(legacy_override);

        // Reload systemd & restart ModemManager silently
        let _ = std::process::Command::new("systemctl")
            .arg("daemon-reload")
            .output();
        let _ = std::process::Command::new("systemctl")
            .args(["restart", "ModemManager.service"])
            .output();
        tracing::info!("ModemManager debug override applied and service restarted.");
    }
}

/// Prepare each baseband's secondary QMI endpoint, and hide every spare QMI port
/// from ModemManager.
///
/// Intended to run before ModemManager starts. For every discovered modem this
/// binds a spare QMI channel belonging to *that* baseband and verifies it really
/// speaks QMI (`wds` present). Endpoint state is published under `/run/simadmin/`
/// so the service can pick it up without re-probing.
///
/// The allocator can place either ordinary data or IMS on DATA6. The other
/// bearer remains on qmi0, so both functions coexist without sharing one WDS
/// slot.
///
/// The udev rules cover *every* spare QMI port, not just the one bound here:
/// the kernel module publishes one port per registered channel, and any spare
/// left visible gets claimed by ModemManager as an extra modem port.
async fn run_secondary_qmi_init(write_udev_rule: bool, dry_run: bool) -> Result<()> {
    use hardware::cellular::secondary_qmi;

    const STATE_DIR: &str = "/run/simadmin";
    // Keep a distinct basename from the packaged /etc fallback rule. udev gives
    // /etc precedence over /run for duplicate basenames, which would otherwise
    // hide the runtime DATA6-specific rule completely.
    const UDEV_RULE_PATH: &str =
        "/run/udev/rules.d/99-simadmin-secondary-qmi-runtime.rules";

    // Discovering modems needs ModemManager, which by design is not up yet. Fall
    // back to enumerating the primary QMI control ports straight from sysfs.
    let primaries = secondary_qmi::discover_primary_qmi_ports();
    if primaries.is_empty() {
        println!("secondary-qmi-init: no QMI control port found; nothing to do");
        return Ok(());
    }

    let mut rules = Vec::new();
    let mut prepared = Vec::new();
    for primary in &primaries {
        if dry_run {
            match secondary_qmi::baseband_key_for_device(primary) {
                Ok(baseband) => {
                    println!("would prepare an IMS endpoint for {primary} (baseband {baseband})");
                }
                Err(error) => println!("would skip {primary}: {error}"),
            }
            continue;
        }
        match secondary_qmi::ensure_endpoint(primary).await {
            Ok(endpoint) => {
                println!(
                    "secondary QMI ready: {} -> {} (channel {}, baseband {}, mode {:?})",
                    primary,
                    endpoint.device_path,
                    endpoint.channel,
                    endpoint.remoteproc,
                    endpoint.open_mode
                );
                rules.push(format!(
                    "SUBSYSTEM==\"wwan\", KERNEL==\"{}\", ENV{{ID_MM_PORT_IGNORE}}=\"1\"",
                    endpoint.port_name
                ));
                prepared.push(endpoint);
            }
            Err(error) => {
                // Not fatal: the line simply falls back to the ModemManager
                // bearer. Report it and keep going with the other basebands.
                eprintln!("secondary QMI unavailable for {primary}: {error}");
            }
        }
    }

    if dry_run {
        return Ok(());
    }

    if write_udev_rule && !rules.is_empty() {
        if let Some(parent) = std::path::Path::new(UDEV_RULE_PATH).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let body = format!(
            "# Generated by `simadmin secondary-qmi-init`.\n\
             # These endpoints carry IMS/VoLTE and must stay under SimAdmin's control;\n\
             # ModemManager must not claim them as extra modem ports.\n{}\n",
            rules.join("\n")
        );
        match std::fs::write(UDEV_RULE_PATH, body) {
            Ok(()) => {
                println!("wrote {UDEV_RULE_PATH}");
                let _ = tokio::process::Command::new("udevadm")
                    .args(["control", "--reload-rules"])
                    .status()
                    .await;
                let _ = tokio::process::Command::new("udevadm")
                    .args(["trigger", "--subsystem-match=wwan"])
                    .status()
                    .await;
            }
            Err(error) => eprintln!("could not write {UDEV_RULE_PATH}: {error}"),
        }
    }

    // Publish the endpoint map for the running service.
    let _ = std::fs::create_dir_all(STATE_DIR);
    let payload = serde_json::to_string_pretty(
        &prepared
            .iter()
            .map(|endpoint| {
                serde_json::json!({
                    "baseband": endpoint.remoteproc,
                    "channel": endpoint.channel,
                    "port_name": endpoint.port_name,
                    "device_path": endpoint.device_path,
                    "netdev": endpoint.netdev,
                    "driver": endpoint.driver,
                })
            })
            .collect::<Vec<_>>(),
    )?;
    let state_file = std::path::Path::new(STATE_DIR).join("secondary-qmi-endpoints.json");
    if let Err(error) = std::fs::write(&state_file, payload) {
        eprintln!("could not write {}: {error}", state_file.display());
    }

    // Beta8 publishes the singular state file as the plain device path. Its
    // qmicli command builder reads this file directly, so JSON here would turn
    // the complete document into an invalid `-d` argument. Keep the richer JSON
    // map above for multi-baseband diagnostics.
    if let Some(endpoint) = prepared.first() {
        if let Err(error) = std::fs::write(
            secondary_qmi::SECONDARY_QMI_STATE_FILE,
            &endpoint.device_path,
        ) {
            eprintln!(
                "could not write {}: {error}",
                secondary_qmi::SECONDARY_QMI_STATE_FILE
            );
        }
    }

    // The systemd unit is Type=notify; tell it we are up so ModemManager can
    // start. Harmless when run by hand outside systemd.
    if std::env::var_os("NOTIFY_SOCKET").is_some() {
        let _ = tokio::process::Command::new("systemd-notify")
            .args([
                "--ready",
                &format!(
                    "--status=DATA6 stock RPMSG initialized at {}",
                    prepared
                        .first()
                        .map(|endpoint| endpoint.device_path.as_str())
                        .unwrap_or("unavailable")
                ),
            ])
            .status()
            .await;
    }

    println!(
        "secondary-qmi-init: {} of {} baseband(s) have an IMS endpoint",
        prepared.len(),
        primaries.len()
    );
    if prepared.is_empty() {
        anyhow::bail!("stock RPMSG driver did not expose a DATA6 WWAN port");
    }

    // Type=notify service remains alive and verifies that the endpoint is still
    // the same character node. A disappearance/replacement makes systemd retry
    // initialization instead of letting ModemManager run against stale state.
    let mut monitors = tokio::task::JoinSet::new();
    for endpoint in prepared {
        monitors.spawn(async move { secondary_qmi::hold_endpoint(&endpoint).await });
    }
    match monitors.join_next().await {
        Some(Ok(Err(error))) => Err(anyhow::anyhow!(error)),
        Some(Err(error)) => Err(anyhow::anyhow!("secondary QMI monitor failed: {error}")),
        Some(Ok(Ok(()))) => Err(anyhow::anyhow!("secondary QMI monitor exited unexpectedly")),
        None => Err(anyhow::anyhow!("secondary QMI monitor was not started")),
    }
}

fn run_extract_zip(archive: &str, target: &str) -> Result<()> {
    use std::io;

    let file = std::fs::File::open(archive)
        .map_err(|e| anyhow::anyhow!("Failed to open {}: {}", archive, e))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| anyhow::anyhow!("Invalid zip archive {}: {}", archive, e))?;

    let target_dir = std::path::Path::new(target);
    std::fs::create_dir_all(target_dir)
        .map_err(|e| anyhow::anyhow!("Failed to create target dir {}: {}", target, e))?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(path) = entry.enclosed_name().map(|p| target_dir.join(p)) else {
            continue;
        };

        if entry.is_dir() {
            std::fs::create_dir_all(&path)?;
            continue;
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut outfile = std::fs::File::create(&path)?;
        io::copy(&mut entry, &mut outfile)?;

        // 保留可执行权限
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.unix_mode() {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
            }
        }
    }

    println!("Extracted {} entries to {}", zip.len(), target);
    Ok(())
}

#[derive(Parser, Debug)]
#[command(name = "simadmin")]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,

    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(Subcommand, Debug)]
enum CliCommand {
    /// 启动 Web 管理服务
    Serve(ServeArgs),
    /// 管理 Web 登录认证（用于 SSH 本机恢复）
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// 解压 ZIP 文件到指定目录（供安装脚本调用）
    ExtractZip {
        /// ZIP 文件路径
        archive: String,
        /// 解压目标目录
        target: String,
    },
    /// Read-only JSON inventory of every ModemManager modem/SIM line.
    InspectModems,
    /// Prepare the per-baseband secondary QMI endpoints that carry IMS/VoLTE.
    ///
    /// Runs before ModemManager (see the shipped systemd unit) so each baseband's
    /// spare QMI channel is bound and hidden from ModemManager via udev. The IMS
    /// bearer then lives on its own endpoint instead of contending with the
    /// primary port that ModemManager uses for normal mobile data.
    SecondaryQmiInit {
        /// Write the udev ignore rule and reload udev (default: yes).
        #[arg(long, default_value_t = true)]
        write_udev_rule: bool,
        /// Report what would happen without binding anything.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
enum AuthCommand {
    /// 交互式重置管理员密码，并清空所有 Web 会话
    ResetPassword,
    /// 清除管理员密码，让 Web UI 下次进入首次设置
    Clear,
}

#[derive(ClapArgs, Debug, Clone)]
struct ServeArgs {
    /// 监听端口 (默认: 3000)
    #[arg(short, long, default_value = "3000", env = "PORT")]
    port: u16,

    /// 监听地址 (默认: ::，双栈监听 IPv4/IPv6)
    #[arg(short = 'H', long, default_value = "::", env = "HOST")]
    host: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化 tracing 日志框架
    // 通过 RUST_LOG 环境变量控制日志级别，默认为 info
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    // 解析命令行参数
    let cli = Cli::parse();

    // 处理非服务子命令
    if let Some(CliCommand::ExtractZip { archive, target }) = &cli.command {
        return run_extract_zip(archive, target);
    }
    if let Some(CliCommand::Auth { command }) = &cli.command {
        let db = Database::new(get_data_db_path())?;
        let config_manager = ConfigManager::new(get_default_config_path());
        let security = config_manager.get_security();
        return match command {
            AuthCommand::ResetPassword => {
                api::auth::reset_admin_password_interactive(&db, &security)
            }
            AuthCommand::Clear => api::auth::clear_admin_auth(&db),
        };
    }
    if matches!(&cli.command, Some(CliCommand::InspectModems)) {
        let conn = Connection::system().await?;
        let mut bindings = hardware::cellular::modem_manager::discover_modem_bindings(&conn).await?;
        for binding in &mut bindings {
            binding.sim_iccid = services::system::system_event::mask_identifier(&binding.sim_iccid);
        }
        println!("{}", serde_json::to_string_pretty(&bindings)?);
        return Ok(());
    }
    if let Some(CliCommand::SecondaryQmiInit {
        write_udev_rule,
        dry_run,
    }) = &cli.command
    {
        return run_secondary_qmi_init(*write_udev_rule, *dry_run).await;
    }

    let args = match cli.command {
        Some(CliCommand::Serve(args)) => args,
        None => cli.serve,
        _ => unreachable!(),
    };
    let bind_addr = display_bind_addr(&args.host, args.port);

    // 确保 ModemManager 已提权以支持 AT 指令读取短信中心
    ensure_modemmanager_debug_override();

    // Connect to system D-Bus
    let dbus_conn = Arc::new(Connection::system().await?);

    // 创建 SMS 数据库（存储在可执行文件同级目录）
    let db_path = get_data_db_path();
    let app_db = Arc::new(Database::new(db_path)?);

    // 初始化配置管理器
    let config_path = get_default_config_path();
    info!(path = ?config_path, "Loading config");
    let config_manager = Arc::new(ConfigManager::new(config_path));
    let data_user_disabled = Arc::new(AtomicBool::new(!config_manager.get_data_enabled()));
    let cell_monitoring_active = Arc::new(AtomicBool::new(false));
    let vowifi_runtime = Arc::new(connectivity::modems::softstack::vowifi::runtime::VowifiRuntime::new());
    let volte_runtime = Arc::new(connectivity::modems::softstack::volte::runtime::VolteRuntime::new());
    let line_registry = Arc::new(services::line_registry::LineRuntimeRegistry::with_config(
        Arc::clone(&volte_runtime),
        Arc::clone(&config_manager),
        Arc::clone(&app_db),
    ));
    match line_registry.refresh(dbus_conn.as_ref()).await {
        Ok(count) => info!(count, "Discovered modem/SIM lines"),
        Err(error) => warn!(error = %error, "Initial modem/SIM line discovery failed"),
    }
    line_registry
        .sync_trunk_profiles(config_manager.as_ref())
        .await;
    if let Some(primary) = line_registry.primary().await {
        let primary_profile = config_manager.get_line_profile(&primary.binding().line_id);
        data_user_disabled.store(
            !primary_profile.data_connection_enabled,
            std::sync::atomic::Ordering::SeqCst,
        );
        if config_manager.get_data_enabled() != primary_profile.data_connection_enabled {
            let _ = config_manager.set_data_enabled(primary_profile.data_connection_enabled);
        }
    }
    // Copy the compiled-in VoWiFi carrier profiles into the database so they can
    // be edited without a rebuild. Existing rows are left alone.
    {
        let profile_store = connectivity::modems::softstack::vowifi::profile_store::ProfileStore::new(Arc::clone(&app_db));
        match profile_store.seed_builtins() {
            Ok(0) => {}
            Ok(inserted) => info!(inserted, "Seeded built-in VoWiFi carrier profiles"),
            Err(error) => warn!(error = %error, "Failed to seed VoWiFi carrier profiles"),
        }
        // Fold any pre-existing `vowifi-profiles.conf` into the database, then
        // archive the file. Custom profiles live in one place from now on.
        let legacy_path = config_manager.legacy_vowifi_profiles_path();
        match profile_store.migrate_legacy_profiles_file(&legacy_path) {
            Ok(0) => {}
            Ok(migrated) => info!(
                migrated,
                "Migrated vowifi-profiles.conf into the carrier profile database"
            ),
            Err(error) => warn!(error = %error, "Failed to migrate legacy VoWiFi profiles"),
        }
        // Make the stored rows visible to the live matcher; without this an
        // edited profile would only show up in the API, not at connect time.
        profile_store.publish();
    }

    let esim_supervisor = Arc::new(EsimSupervisor::new(Arc::clone(&config_manager)));

    let nm_result = ensure_nm_modem_profile().await;
    tracing::info!(result = %nm_result, "NetworkManager modem profile setup completed");

    // 初始化通知发送器
    let notification_sender = Arc::new(NotificationSender::new(
        Arc::clone(&config_manager),
        Arc::clone(&dbus_conn),
        Arc::clone(&app_db),
    ));
    let system_event_emitter = Arc::new(SystemEventEmitter::new(Arc::clone(&notification_sender)));
    let (sms_resync, sms_resync_rx) = services::messaging::sms_listener::sms_resync_channel();
    let ddns_manager = Arc::new(DdnsManager::new());
    {
        let notification_queue_worker = Arc::clone(&notification_sender);
        tokio::spawn(async move {
            notification_queue_worker.run_queue_worker().await;
        });
    }
    services::system::system_event_monitor::spawn_system_event_monitor(
        Arc::clone(&system_event_emitter),
        Arc::clone(&dbus_conn),
    );
    services::system::device_status::spawn_device_status_scheduler(
        Arc::clone(&config_manager),
        Arc::clone(&notification_sender),
        Arc::clone(&app_db),
        Arc::clone(&dbus_conn),
        Arc::clone(&ddns_manager),
    );

    {
        let ddns_manager_clone = Arc::clone(&ddns_manager);
        let config_clone = Arc::clone(&config_manager);
        let notification_clone = Arc::clone(&notification_sender);
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
            loop {
                let config = config_clone.get_ddns_config();
                let interval = config.interval_seconds.max(60);
                if config.enabled {
                    if let Err(err) = ddns_manager_clone
                        .sync_now(Arc::clone(&config_clone), Arc::clone(&notification_clone))
                        .await
                    {
                        tracing::warn!(error = %err, "DDNS background sync failed");
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;
            }
        });
    }

    {
        let config_clone = Arc::clone(&config_manager);
        let notification_clone = Arc::clone(&notification_sender);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(crate::services::system::ota::duration_until_next_update_check()).await;
                let config = config_clone.get_version_update_notifications();
                if config.enabled {
                    if let Err(err) = crate::services::system::ota::check_and_notify_version_update(
                        Arc::clone(&config_clone),
                        Arc::clone(&notification_clone),
                    )
                    .await
                    {
                        tracing::warn!(error = %err, "Version update notification check failed");
                    }
                }
            }
        });
    }

    // 启动 SMS 监听线程
    {
        let conn_clone = Connection::system().await?;
        let db_clone = Arc::clone(&app_db);
        let notification_clone = Arc::clone(&notification_sender);
        let sms_config_clone = Arc::clone(&config_manager);
        let sms_line_registry = Arc::clone(&line_registry);
        let resync_rx = sms_resync_rx;
        tokio::spawn(async move {
            let _ = services::messaging::sms_listener::start_sms_listener(
                conn_clone,
                db_clone,
                notification_clone,
                sms_config_clone,
                sms_line_registry,
                resync_rx,
            )
            .await;
        });
    }

    // 电话监听暂不启用

    // Boot-time cellular data is brought up per-line and proxy-isolated by the
    // per-line data supervisor spawned after AppState is built (see below).
    // The legacy global `init_data_connection` auto-connect is intentionally not
    // started here: it activated the first modem's bearer as a system-default
    // route, which contradicts the per-line proxy-only egress model. Data now
    // only comes up for lines whose profile has `data_connection_enabled`, and
    // only proxied traffic (SO_BINDTODEVICE) uses that SIM.

    // 启动数据连接 Watchdog（每 15 秒检查一次）
    {
        let conn_clone = Arc::clone(&dbus_conn);
        let user_off = Arc::clone(&data_user_disabled);
        let cfg = Arc::clone(&config_manager);
        let system_events = Arc::clone(&system_event_emitter);
        tokio::spawn(async move {
            // 初始延迟 5 秒，等待系统稳定
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            tracing::info!(interval = 15, "Watchdog started");
            hardware::cellular::modem_manager::data_connection_watchdog(
                conn_clone,
                15,
                user_off,
                cfg,
                system_events,
            )
            .await;
        });
    }

    system_event_emitter
        .emit_code(
            system_event_codes::SYSTEM_SERVICE_STARTED,
            system_event_severity::INFO,
            system_event_status::SUCCEEDED,
            "simadmin",
            "SimAdmin 服务启动完成",
        )
        .await;

    // CORS 配置：允许前端开发服务器跨域访问
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // 创建统一的应用状态
    let app_state = AppState::new(AppStateDependencies {
        dbus_conn,
        database: app_db,
        config_manager,
        notification_sender,
        system_event_emitter,
        ddns_manager,
        esim_supervisor: Arc::clone(&esim_supervisor),
        sms_resync,
        data_user_disabled,
        vowifi_runtime,
        volte_runtime,
        line_registry,
        cell_monitoring_active,
    });

    // Restore only explicitly enabled per-line data/airplane intents. The
    // legacy global data switch is kept in sync for compatibility, but it no
    // longer causes an unselected modem to become an implicit proxy route.
    {
        let restore_app = app_state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;
            for line in restore_app.line_registry.all().await {
                let binding = line.binding();
                if !binding.present {
                    continue;
                }
                let profile = restore_app
                    .config_manager
                    .get_line_profile(&binding.line_id);
                if profile.airplane_mode_enabled {
                    api::handlers::stop_line_data_runtime(&restore_app, &line).await;
                    let _ = set_airplane_mode_for_modem(
                        restore_app.dbus_conn.as_ref(),
                        &binding.modem_path,
                        true,
                    )
                    .await;
                    continue;
                }
                if !profile.data_connection_enabled {
                    continue;
                }
                if let Err(error) =
                    api::handlers::start_line_data_runtime(&restore_app, &line, &profile).await
                {
                    let _ = line.data_proxy.record_error(error).await;
                }
            }
        });
    }

    // Keep the line inventory synchronized with ModemManager hotplug and SIM
    // replacement events. Refresh preserves existing per-line runtime state.
    {
        let refresh_app = app_state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(10));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                if let Err(error) = refresh_app
                    .line_registry
                    .refresh(refresh_app.dbus_conn.as_ref())
                    .await
                {
                    tracing::warn!(error = %error, "Modem/SIM line inventory refresh failed");
                } else {
                    refresh_app
                        .line_registry
                        .sync_trunk_profiles(refresh_app.config_manager.as_ref())
                        .await;
                }
            }
        });
    }

    // 启动自动化中心后台调度引擎
    services::automation::spawn_automation_scheduler(app_state.clone());

    // Flush each line's proxied-traffic counters to disk periodically. Counting
    // is in memory for speed; this is what makes the totals survive a restart.
    {
        let traffic_registry = Arc::clone(&app_state.line_registry);
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(tokio::time::Duration::from_secs(LINE_TRAFFIC_FLUSH_SECS));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // Skip the immediate first tick; nothing has been counted yet.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                traffic_registry.flush_data_traffic().await;
            }
        });
    }

    // Phase C: prune both dedup fingerprints and user-visible SMS history each
    // day so long-running installs remain bounded. Both limits are
    // user-configurable via the SMS path policy.
    {
        let cleanup_app = app_state.clone();
        tokio::spawn(async move {
            let db = cleanup_app.database.clone();
            let config_manager = cleanup_app.config_manager.clone();
            // Small startup delay so the first sweep doesn't race with boot.
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            loop {
                let policy = config_manager.get_sms_path_policy();
                let retention_days = policy.dedup_retention_days;
                match db.cleanup_sms_dedup(retention_days) {
                    Ok(deleted) if deleted > 0 => {
                        tracing::info!(
                            deleted,
                            retention_days,
                            "Pruned expired SMS dedup fingerprints"
                        );
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!(error = %err, "Failed to prune SMS dedup fingerprints");
                    }
                }
                match db.prune_sms_messages(policy.message_retention_limit) {
                    Ok(deleted) if deleted > 0 => {
                        tracing::info!(
                            deleted,
                            message_retention_limit = policy.message_retention_limit,
                            "Pruned oldest SMS history rows"
                        );
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!(error = %err, "Failed to prune SMS history rows");
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(24 * 60 * 60)).await;
            }
        });
    }

    // Build protected routes - 使用统一的 AppState
    spawn_vowifi_auto_restore(app_state.clone());
    spawn_volte_auto_restore(app_state.clone());

    let protected_routes = Router::new()
        // ========== 设备信息接口 ==========
        .route("/api/device", get(get_device_info).options(options_handler))
        .route(
            "/api/modems",
            get(get_modem_lines_handler)
                .post(get_modem_lines_handler)
                .options(options_handler),
        )
        // ========== SIM 卡接口 ==========
        .route("/api/sim", get(get_sim_info).options(options_handler))
        .route(
            "/api/sim/cache",
            post(update_sim_cache_handler).options(options_handler),
        )
        // ========== 网络接口 ==========
        .route(
            "/api/network",
            get(get_network_info).options(options_handler),
        )
        .route("/api/cells", get(get_cells).options(options_handler))
        .route(
            "/api/cell-monitor/start",
            post(start_cell_monitor_handler).options(options_handler),
        )
        .route(
            "/api/cell-monitor/stop",
            post(stop_cell_monitor_handler).options(options_handler),
        )
        .route(
            "/api/radio-mode",
            get(get_radio_mode_handler)
                .post(set_radio_mode_handler)
                .options(options_handler),
        )
        .route(
            "/api/band-lock",
            get(get_band_lock_handler)
                .post(set_band_lock_handler)
                .options(options_handler),
        )
        .route(
            "/api/network/interfaces",
            get(get_network_interfaces_info).options(options_handler),
        )
        .route(
            "/api/network/connection-addresses",
            get(get_network_connection_addresses).options(options_handler),
        )
        .route(
            "/api/device-network/ddns/config",
            get(get_device_ddns_config_handler)
                .post(set_device_ddns_config_handler)
                .options(options_handler),
        )
        .route(
            "/api/device-network/ddns/status",
            get(get_device_ddns_status_handler).options(options_handler),
        )
        .route(
            "/api/device-network/ddns/sync",
            post(sync_device_ddns_handler).options(options_handler),
        )
        .route(
            "/api/device-network/ddns/logs",
            get(get_device_ddns_logs_handler).options(options_handler),
        )
        .route(
            "/api/device-network/ddns/logs/clear",
            post(clear_device_ddns_logs_handler).options(options_handler),
        )
        .route(
            "/api/device-network/wlan/status",
            get(get_device_wlan_status_handler).options(options_handler),
        )
        .route(
            "/api/device-network/wlan/enabled",
            post(set_device_wlan_enabled_handler).options(options_handler),
        )
        .route(
            "/api/device-network/wlan/scan",
            post(scan_device_wlan_handler).options(options_handler),
        )
        .route(
            "/api/device-network/wlan/profiles",
            get(get_device_wlan_profiles_handler).options(options_handler),
        )
        .route(
            "/api/device-network/wlan/forget",
            post(forget_device_wlan_handler).options(options_handler),
        )
        .route(
            "/api/device-network/wlan/connect",
            post(connect_device_wlan_handler).options(options_handler),
        )
        .route(
            "/api/device-network/wlan/disconnect",
            post(disconnect_device_wlan_handler).options(options_handler),
        )
        .route(
            "/api/device-network/wlan/profile",
            post(save_device_wlan_profile_handler).options(options_handler),
        )
        .route(
            "/api/network/signal-strength",
            get(get_signal_strength_handler).options(options_handler),
        )
        .route(
            "/api/location/cell-info",
            get(get_cell_location_handler).options(options_handler),
        )
        .route(
            "/api/network/operators",
            get(get_network_operators).options(options_handler),
        )
        .route(
            "/api/network/operators/scan",
            get(scan_network_operators).options(options_handler),
        )
        .route(
            "/api/network/register-manual",
            post(register_network_manual).options(options_handler),
        )
        .route(
            "/api/network/register-auto",
            post(register_network_auto).options(options_handler),
        )
        .route(
            "/api/apn",
            get(get_apn_list_handler)
                .post(set_apn_handler)
                .options(options_handler),
        )
        .route(
            "/api/cell-lock",
            get(get_cell_lock_status_handler)
                .post(set_cell_lock_handler)
                .options(options_handler),
        )
        .route(
            "/api/cell-lock/unlock-all",
            post(unlock_all_cells_handler).options(options_handler),
        )
        // ========== 数据连接接口 ==========
        .route(
            "/api/data",
            get(get_data_status)
                .post(set_data_status)
                .options(options_handler),
        )
        // Roaming and airplane mode are per-line only. The old global
        // `/api/roaming` and `/api/airplane-mode` endpoints acted on whichever
        // modem came first while pretending to be device-wide, so they are gone;
        // use `/api/modem/lines/{line_id}/roaming` and
        // `/api/modem/lines/{line_id}/airplane-mode` instead, and read state
        // from `/api/modem/line-controls`.
        .route(
            "/api/modem/line-controls",
            get(get_line_network_controls_handler).options(options_handler),
        )
        .route(
            "/api/modem/lines/{line_id}/data",
            post(set_line_data_connection_handler).options(options_handler),
        )
        .route(
            "/api/modem/lines/{line_id}/data/config",
            post(set_line_data_proxy_config_handler).options(options_handler),
        )
        .route(
            "/api/modem/lines/{line_id}/data/traffic/reset",
            post(reset_line_data_traffic_handler).options(options_handler),
        )
        .route(
            "/api/modem/lines/{line_id}/roaming",
            post(set_line_roaming_handler).options(options_handler),
        )
        .route(
            "/api/modem/lines/{line_id}/airplane-mode",
            post(set_line_airplane_mode_handler).options(options_handler),
        )
        .route(
            "/api/baseband/restart",
            post(restart_baseband_handler).options(options_handler),
        )
        .route(
            "/api/baseband/restart/status",
            get(get_baseband_restart_status_handler).options(options_handler),
        )
        // ========== eSIM 管理 ==========
        .route(
            "/api/modem/lines/{line_id}/esim-control",
            get(get_line_esim_control_handler)
                .post(set_line_esim_control_handler)
                .options(options_handler),
        )
        .route(
            "/api/esim/config",
            get(get_esim_config_handler)
                .post(set_esim_config_handler)
                .options(options_handler),
        )
        .route(
            "/api/esim/lpac/status",
            get(get_esim_lpac_status_handler).options(options_handler),
        )
        .route(
            "/api/esim/lpac/repair",
            post(repair_esim_lpac_handler).options(options_handler),
        )
        .route(
            "/api/esim/euicc",
            get(get_esim_euicc_handler).options(options_handler),
        )
        .route(
            "/api/esim/profiles",
            get(get_esim_profiles_handler)
                .post(download_esim_profile_handler)
                .options(options_handler),
        )
        .route(
            "/api/esim/profiles/{iccid}/enable",
            post(enable_esim_profile_handler).options(options_handler),
        )
        .route(
            "/api/esim/profiles/{iccid}/rename",
            post(rename_esim_profile_handler).options(options_handler),
        )
        .route(
            "/api/esim/profiles/{iccid}",
            delete(delete_esim_profile_handler).options(options_handler),
        )
        // ========== 电话功能接口 ==========
        .route(
            "/api/calls",
            get(get_calls_handler).options(options_handler),
        )
        .route(
            "/api/call/dial",
            post(dial_call_handler).options(options_handler),
        )
        .route(
            "/api/call/hangup",
            post(hangup_call_handler).options(options_handler),
        )
        .route(
            "/api/call/hangup-all",
            post(hangup_all_calls_handler).options(options_handler),
        )
        .route(
            "/api/call/answer",
            post(answer_call_handler).options(options_handler),
        )
        .route(
            "/api/call/volume",
            get(get_call_volume_handler)
                .post(set_call_volume_handler)
                .options(options_handler),
        )
        .route(
            "/api/call/forwarding",
            get(get_call_forwarding_handler)
                .post(set_call_forwarding_handler)
                .options(options_handler),
        )
        .route(
            "/api/call/settings",
            get(get_call_settings_handler)
                .post(set_call_settings_handler)
                .options(options_handler),
        )
        .route(
            "/api/call/history",
            get(get_call_history_handler).options(options_handler),
        )
        .route(
            "/api/call/history/{id}",
            axum::routing::delete(delete_call_history_handler).options(options_handler),
        )
        .route(
            "/api/call/history/clear",
            post(clear_call_history_handler).options(options_handler),
        )
        .route(
            "/api/ims/status",
            get(get_ims_status_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/status",
            get(get_vowifi_status_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/control",
            get(get_vowifi_control_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/feature",
            post(set_vowifi_feature_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/connection",
            post(set_vowifi_connection_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/connect",
            post(connect_vowifi_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/profile",
            get(get_vowifi_profile_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/diagnostics",
            get(get_vowifi_diagnostics_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/profiles",
            get(get_vowifi_profiles_handler).options(options_handler),
        )
        // Editable carrier profile database. Replaces the compiled-in constants
        // as the source of truth; unknown carriers still fall back to 3GPP
        // derivation, so a SIM with no entry here can still connect.
        .route(
            "/api/vowifi/carrier-profiles",
            get(list_vowifi_carrier_profiles_handler)
                .put(save_vowifi_carrier_profile_handler)
                .options(options_handler),
        )
        .route(
            "/api/vowifi/carrier-profiles/resolve",
            get(resolve_vowifi_carrier_profile_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/carrier-profiles/import",
            post(import_vowifi_carrier_profiles_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/carrier-profiles/{profile_id}",
            delete(delete_vowifi_carrier_profile_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/external-profiles",
            get(get_external_vowifi_profiles_handler)
                .post(set_external_vowifi_profile_handler)
                .options(options_handler),
        )
        .route(
            "/api/vowifi/lines",
            get(get_vowifi_lines_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/lines/{line_id}",
            post(set_vowifi_line_config_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/lines/{line_id}/connection",
            post(set_vowifi_line_connection_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/events",
            get(get_vowifi_events_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/soak",
            get(get_vowifi_soak_runs_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/sms/delivery",
            get(get_vowifi_sms_deliveries_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/sms/delivery/{message_id}",
            get(get_vowifi_sms_delivery_handler).options(options_handler),
        )
        .route(
            "/api/vowifi/esim-restore/status",
            get(get_vowifi_esim_restore_handler).options(options_handler),
        )
        .route(
            "/api/volte/control",
            get(get_volte_control_handler).options(options_handler),
        )
        .route(
            "/api/volte/feature",
            post(set_volte_feature_handler).options(options_handler),
        )
        .route(
            "/api/volte/connection",
            post(set_volte_connection_handler).options(options_handler),
        )
        .route(
            "/api/volte/lines",
            get(get_volte_lines_handler).options(options_handler),
        )
        .route(
            "/api/volte/lines/{line_id}",
            get(get_volte_line_handler).options(options_handler),
        )
        .route(
            "/api/volte/lines/{line_id}/connection",
            post(set_volte_line_connection_handler).options(options_handler),
        )
        .route(
            "/api/volte/lines/{line_id}/retry",
            post(retry_volte_line_handler).options(options_handler),
        )
        .route(
            "/api/volte/lines/{line_id}/ip-families",
            post(set_volte_line_ip_families_handler).options(options_handler),
        )
        .route(
            "/api/sim/slots",
            get(get_standalone_sim_slots_handler)
                .post(set_standalone_sim_slots_handler)
                .options(options_handler),
        )
        .route(
            "/api/trunk/lines",
            get(get_trunk_lines_handler).options(options_handler),
        )
        .route(
            "/api/trunk/lines/{line_id}",
            get(get_line_trunk_handler)
                .post(set_line_trunk_handler)
                .options(options_handler),
        )
        .route(
            "/api/trunk/lines/{line_id}/enabled",
            post(set_line_trunk_enabled_handler).options(options_handler),
        )
        .route(
            "/api/volte/call/status",
            get(get_volte_call_status_handler).options(options_handler),
        )
        .route(
            "/api/volte/voice",
            post(set_volte_voice_handler).options(options_handler),
        )
        .route(
            "/api/voice/path-policy",
            get(get_voice_path_policy_handler)
                .post(set_voice_path_policy_handler)
                .options(options_handler),
        )
        .route(
            "/api/web-call/capabilities",
            get(get_web_call_capabilities_handler).options(options_handler),
        )
        .route(
            "/api/sms/path-policy",
            get(get_sms_path_policy_handler)
                .post(set_sms_path_policy_handler)
                .options(options_handler),
        )
        .route(
            "/api/vilte/control",
            get(get_vilte_control_handler)
                .post(set_vilte_feature_handler)
                .options(options_handler),
        )
        .route(
            "/api/vilte/config",
            post(set_vilte_config_handler).options(options_handler),
        )
        // ========== 短信功能接口 ==========
        .route(
            "/api/sms/send",
            post(send_sms_handler).options(options_handler),
        )
        .route(
            "/api/sms/channels",
            get(get_sms_channels_handler).options(options_handler),
        )
        .route(
            "/api/sms/list",
            get(get_sms_list_handler).options(options_handler),
        )
        .route(
            "/api/sms/conversation",
            get(get_sms_conversation_handler).options(options_handler),
        )
        .route(
            "/api/sms/stats",
            get(get_sms_stats_handler).options(options_handler),
        )
        .route(
            "/api/sms/batch-delete",
            post(delete_sms_batch_handler).options(options_handler),
        )
        .route(
            "/api/sms/conversation/{phone_number}",
            axum::routing::delete(delete_sms_conversation_handler).options(options_handler),
        )
        .route(
            "/api/sms/message/{id}",
            axum::routing::delete(delete_sms_message_handler).options(options_handler),
        )
        .route(
            "/api/sms/clear",
            post(clear_sms_handler).options(options_handler),
        )
        // ========== 语音通话接口 (VoWiFi) ==========
        .route(
            "/api/voice/call",
            post(place_call_handler).options(options_handler),
        )
        // ========== 系统接口 ==========
        .route("/api/stats", get(get_system_stats).options(options_handler))
        .route("/api/stats/cpu", get(get_cpu_info).options(options_handler))
        .route(
            "/api/connectivity",
            get(get_connectivity_check).options(options_handler),
        )
        .route(
            "/api/system/reboot",
            post(system_reboot).options(options_handler),
        )
        .route(
            "/api/service/restart",
            post(restart_service_handler).options(options_handler),
        )
        // ========== 通知配置接口 ==========
        .route(
            "/api/notifications/config",
            get(get_notification_config_handler)
                .post(set_notification_config_handler)
                .options(options_handler),
        )
        .route(
            "/api/notifications/test/{channel}",
            post(test_notification_channel_handler).options(options_handler),
        )
        // ========== OTA 更新接口 ==========
        .route(
            "/api/notifications/logs",
            get(get_notification_logs_handler).options(options_handler),
        )
        .route(
            "/api/notifications/logs/clear",
            post(clear_notification_logs_handler).options(options_handler),
        )
        .route(
            "/api/notifications/queue",
            get(get_notification_queue_handler).options(options_handler),
        )
        .route(
            "/api/notifications/queue/retry-all",
            post(retry_all_notification_queue_handler).options(options_handler),
        )
        .route(
            "/api/notifications/queue/clear",
            post(clear_notification_queue_handler).options(options_handler),
        )
        .route(
            "/api/notifications/queue/{id}",
            delete(delete_notification_queue_item_handler).options(options_handler),
        )
        .route(
            "/api/notifications/queue/{id}/retry",
            post(retry_notification_queue_item_handler).options(options_handler),
        )
        // ========== 自动化中心接口 ==========
        .route(
            "/api/automation/config",
            get(get_automation_config_handler)
                .post(set_automation_config_handler)
                .options(options_handler),
        )
        .route(
            "/api/automation/logs",
            get(get_automation_logs_handler).options(options_handler),
        )
        .route(
            "/api/automation/logs/clear",
            post(clear_automation_logs_handler).options(options_handler),
        )
        .route(
            "/api/automation/test/{task_id}",
            post(test_automation_task_handler).options(options_handler),
        )
        .route(
            "/api/ota/status",
            get(get_ota_status_handler).options(options_handler),
        )
        .route(
            "/api/ota/upload",
            post(upload_ota_handler)
                .options(options_handler)
                .layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
        )
        .route(
            "/api/ota/latest-release",
            post(get_latest_ota_release_handler).options(options_handler),
        )
        .route(
            "/api/ota/online-prepare",
            post(prepare_online_ota_handler).options(options_handler),
        )
        .route(
            "/api/ota/apply",
            post(apply_ota_handler).options(options_handler),
        )
        .route(
            "/api/ota/cancel",
            post(cancel_ota_handler).options(options_handler),
        )
        .route(
            "/api/auth/password",
            post(api::auth::change_password).options(options_handler),
        )
        .route(
            "/api/auth/settings",
            get(api::auth::get_settings)
                .post(api::auth::set_settings)
                .options(options_handler),
        )
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            api::auth::auth_middleware,
        ));

    let app = Router::new()
        .route("/api/health", get(health_check).options(options_handler))
        .route(
            "/api/auth/status",
            get(api::auth::status).options(options_handler),
        )
        .route(
            "/api/auth/setup",
            post(api::auth::setup).options(options_handler),
        )
        .route(
            "/api/auth/login",
            post(api::auth::login).options(options_handler),
        )
        .route(
            "/api/auth/logout",
            post(api::auth::logout).options(options_handler),
        )
        .merge(protected_routes)
        .with_state(app_state)
        .layer(cors)
        .fallback(spa_fallback);

    // Start server - 显示版权信息
    info!(
        version = env!("APP_VERSION"),
        branch = env!("GIT_BRANCH"),
        commit = env!("GIT_COMMIT"),
        "SimAdmin - Debian SIM Management Service"
    );
    info!("Copyright © 2026 GitHub 3899 - SimAdmin");

    // 绑定端口，如果被占用则轮询等待（最多 30 秒）
    let listener = bind_with_retry(&args.host, args.port, 30).await?;
    info!(addr = %bind_addr, "Server listening");
    // 使用优雅关闭
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

/// 绑定端口，如果被占用则轮询等待
fn display_bind_addr(host: &str, port: u16) -> String {
    let host = host.trim_matches(|c| c == '[' || c == ']');
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

async fn bind_listener(host: &str, port: u16) -> std::io::Result<tokio::net::TcpListener> {
    let normalized_host = host.trim_matches(|c| c == '[' || c == ']');
    if normalized_host == "::" {
        let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
        socket.set_reuse_address(true)?;
        socket.set_only_v6(false)?;
        socket.bind(&SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port).into())?;
        socket.listen(1024)?;
        socket.set_nonblocking(true)?;
        let listener: std::net::TcpListener = socket.into();
        return tokio::net::TcpListener::from_std(listener);
    }

    tokio::net::TcpListener::bind((normalized_host, port)).await
}

async fn bind_with_retry(
    host: &str,
    port: u16,
    max_retries: u32,
) -> Result<tokio::net::TcpListener> {
    use std::time::Duration;
    let addr = display_bind_addr(host, port);

    for i in 0..max_retries {
        match bind_listener(host, port).await {
            Ok(listener) => return Ok(listener),
            Err(e) => {
                if i == 0 {
                    warn!(addr = %addr, "Port busy, waiting for release...");
                }
                if i + 1 < max_retries {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                } else {
                    return Err(anyhow::anyhow!("Failed to bind to {}: {}", addr, e));
                }
            }
        }
    }
    unreachable!()
}

/// 监听 Ctrl+C 和 SIGTERM 信号，用于优雅关闭
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C signal handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    warn!("Shutdown signal received; starting graceful shutdown");
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(8));
        eprintln!("SimAdmin graceful shutdown exceeded 8s; forcing process exit");
        std::process::exit(0);
    });
}
