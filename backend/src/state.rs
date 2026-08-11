//! 应用状态模块
//! 统一管理应用的共享状态

use axum::extract::FromRef;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use zbus::Connection;

use crate::connectivity::modems::ims::profile_override::SimOverrideStore;
use crate::connectivity::modems::ims::vowifi::carrier_catalog::CarrierCatalog;
use crate::hardware::cellular::cell_lock_store::CellLockStore;
use crate::hardware::sim::esim::EsimSupervisor;
use crate::platform::config::ConfigManager;
use crate::platform::db::Database;
use crate::services::e911::orchestrator::E911Orchestrator;
use crate::services::line_registry::LineRuntimeRegistry;
use crate::services::messaging::sms_listener::SmsResyncHandle;
use crate::services::network::device_network::DdnsManager;
use crate::services::notify::notification::NotificationSender;
use crate::services::system::system_event::SystemEventEmitter;

#[derive(Clone)]
pub struct ActiveCallRecord {
    pub id: i64,
    pub line_id: String,
    pub direction: String,
    pub answered_at: Option<Instant>,
    pub answered: bool,
    pub missing_polls: u8,
}

/// 应用全局状态
///
/// 统一管理所有共享资源，避免在路由中多次调用 `.with_state()`
#[derive(Clone)]
pub struct AppState {
    /// D-Bus 连接（用于与 ofono 通信）
    pub dbus_conn: Arc<Connection>,
    /// 数据库连接（用于存储 SMS 和通话记录）
    pub database: Arc<Database>,
    /// 配置管理器（用于管理通知等配置）
    pub config_manager: Arc<ConfigManager>,
    /// 通知发送器（用于转发 SMS、通话和 DDNS 通知）
    pub notification_sender: Arc<NotificationSender>,
    pub system_event_emitter: Arc<SystemEventEmitter>,
    pub ddns_manager: Arc<DdnsManager>,
    pub esim_supervisor: Arc<EsimSupervisor>,
    pub sms_resync: SmsResyncHandle,
    pub sms_db_maintenance_pending: Arc<AtomicBool>,
    pub active_calls: Arc<Mutex<HashMap<String, ActiveCallRecord>>>,
    /// Running automation task and target scopes. A standard mutex is used
    /// because reservations are tiny synchronous critical sections and must
    /// be acquired before spawning background work.
    pub automation_running_scopes: Arc<std::sync::Mutex<HashSet<String>>>,
    /// 小区锁定 UI 状态（底层无锁网时仅内存态），按 line_id 分开保存，
    /// 否则一张卡的锁定会显示/覆盖到另一张卡上。
    pub cell_lock: Arc<Mutex<HashMap<String, CellLockStore>>>,
    /// Per physical-modem + active-SIM runtime registry.
    pub line_registry: Arc<LineRuntimeRegistry>,
    /// Lines whose ModemManager location/signal polling has been enabled.
    pub cell_monitoring_active: Arc<Mutex<HashSet<String>>>,
    /// Immutable carrier access/IMS/SIP configuration catalog.
    pub carrier_catalog: Arc<CarrierCatalog>,
    /// Per-SIM user overrides, keyed by `SimBindingKey`.
    pub sim_overrides: Arc<SimOverrideStore>,
    /// E911 entitlement orchestrator (query/status/websheet operations).
    pub e911: Arc<E911Orchestrator>,
}

/// Named startup dependencies prevent positional mix-ups as application state grows.
pub struct AppStateDependencies {
    pub dbus_conn: Arc<Connection>,
    pub database: Arc<Database>,
    pub config_manager: Arc<ConfigManager>,
    pub notification_sender: Arc<NotificationSender>,
    pub system_event_emitter: Arc<SystemEventEmitter>,
    pub ddns_manager: Arc<DdnsManager>,
    pub esim_supervisor: Arc<EsimSupervisor>,
    pub sms_resync: SmsResyncHandle,
    pub line_registry: Arc<LineRuntimeRegistry>,
    pub cell_monitoring_active: Arc<Mutex<HashSet<String>>>,
    pub carrier_catalog: Arc<CarrierCatalog>,
    pub sim_overrides: Arc<SimOverrideStore>,
    pub e911: Arc<E911Orchestrator>,
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(dependencies: AppStateDependencies) -> Self {
        let AppStateDependencies {
            dbus_conn,
            database,
            config_manager,
            notification_sender,
            system_event_emitter,
            ddns_manager,
            esim_supervisor,
            sms_resync,
            line_registry,
            cell_monitoring_active,
            carrier_catalog,
            sim_overrides,
            e911,
        } = dependencies;
        Self {
            dbus_conn,
            database,
            config_manager,
            notification_sender,
            system_event_emitter,
            ddns_manager,
            esim_supervisor,
            sms_resync,
            sms_db_maintenance_pending: Arc::new(AtomicBool::new(false)),
            active_calls: Arc::new(Mutex::new(HashMap::new())),
            automation_running_scopes: Arc::new(std::sync::Mutex::new(HashSet::new())),
            cell_lock: Arc::new(Mutex::new(HashMap::new())),
            line_registry,
            cell_monitoring_active,
            carrier_catalog,
            sim_overrides,
            e911,
        }
    }
}

// 实现 FromRef trait，允许从 AppState 中提取子状态
// 这样现有的 handler 可以继续使用 State<Arc<Connection>> 等类型

impl FromRef<AppState> for Arc<Connection> {
    fn from_ref(state: &AppState) -> Self {
        state.dbus_conn.clone()
    }
}

impl FromRef<AppState> for Arc<Database> {
    fn from_ref(state: &AppState) -> Self {
        state.database.clone()
    }
}

impl FromRef<AppState> for Arc<ConfigManager> {
    fn from_ref(state: &AppState) -> Self {
        state.config_manager.clone()
    }
}

impl FromRef<AppState> for Arc<NotificationSender> {
    fn from_ref(state: &AppState) -> Self {
        state.notification_sender.clone()
    }
}

impl FromRef<AppState> for Arc<SystemEventEmitter> {
    fn from_ref(state: &AppState) -> Self {
        state.system_event_emitter.clone()
    }
}

impl FromRef<AppState> for Arc<DdnsManager> {
    fn from_ref(state: &AppState) -> Self {
        state.ddns_manager.clone()
    }
}

impl FromRef<AppState> for Arc<EsimSupervisor> {
    fn from_ref(state: &AppState) -> Self {
        state.esim_supervisor.clone()
    }
}

impl FromRef<AppState> for Arc<Mutex<HashMap<String, CellLockStore>>> {
    fn from_ref(state: &AppState) -> Self {
        state.cell_lock.clone()
    }
}

// 支持 (Arc<Connection>, Arc<Database>) 元组类型
impl FromRef<AppState> for (Arc<Connection>, Arc<Database>) {
    fn from_ref(state: &AppState) -> Self {
        (state.dbus_conn.clone(), state.database.clone())
    }
}

impl FromRef<AppState> for Arc<E911Orchestrator> {
    fn from_ref(state: &AppState) -> Self {
        state.e911.clone()
    }
}
