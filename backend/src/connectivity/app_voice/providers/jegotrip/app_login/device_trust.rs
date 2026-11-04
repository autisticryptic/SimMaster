//! 设备授信查询的历史观察，绝不充当当前授权、SDK 凭据或 ready 证明。
use super::{Error, device_identity::Snapshot, http::{CallStatus,TrustDecision}, device_sync::DeviceSyncObservation, stored_code, valid_attempt_id};
use serde::{Deserialize, Serialize};

pub(super) const MAX_OBSERVATION_BYTES: usize = 2048;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrustQueryState { NotConfigured, NotRequested, Observed, Failed }

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrustQueryObservation {
    pub state: TrustQueryState,
    pub authentication_code: Option<i64>,
    pub error: Option<String>,
    pub server_code: Option<u32>,
}
impl TrustQueryObservation {
    pub(super) fn not_requested()->Self {Self {state:TrustQueryState::NotRequested,authentication_code:None,error:None,server_code:None}}
    /// 只保留有限状态码；token、设备值和服务端反射文本不能进入观察记录。
    pub(super) fn capture(result: Result<Option<TrustDecision>, Error>) -> Self {
        match result {
            Ok(Some(value)) => Self { state: TrustQueryState::Observed, authentication_code: value.authentication_code, error: None, server_code: None },
            Ok(None) => Self { state: TrustQueryState::NotConfigured, authentication_code: None, error: None, server_code: None },
            Err(error) => Self { state: TrustQueryState::Failed, authentication_code: None,
                error: Some(stored_code(error.code).unwrap_or("app_login_internal_failed").to_owned()), server_code: error.server_code },
        }
    }
    fn valid(&self) -> bool {
        if self.authentication_code.is_some_and(|n| !(0..=999999).contains(&n)) { return false; }
        match self.state {
            TrustQueryState::NotConfigured|TrustQueryState::NotRequested => self.authentication_code.is_none() && self.error.is_none() && self.server_code.is_none(),
            TrustQueryState::Observed => self.error.is_none() && self.server_code.is_none(),
            TrustQueryState::Failed => self.authentication_code.is_none() && self.error.as_deref().is_some_and(|e| stored_code(e).is_some()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceTrustObservation {
    /// 本地标识行/版本，不复制 UUID、账号或任何认证 token。
    pub device_identity_id: String,
    pub device_identity_revision: u64,
    pub observed_at: i64,
    pub security_verify: TrustQueryObservation,
    pub existing_device: TrustQueryObservation,
    pub hosting_enabled: Option<bool>,
    pub business_query_error: Option<String>,
    pub business_query_server_code: Option<u32>,
    /// New reuse observations are additive; old 2KiB records still decode.
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub call_status:Option<TrustQueryObservation>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub local_cs_forward:Option<i64>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub device_sync:Option<DeviceSyncObservation>,
}
impl DeviceTrustObservation {
    /// 与本次原始设备快照绑定；服务器读回与历史存储均不绕过业务开关。
    pub(super) fn capture(snapshot: &Snapshot, at: i64, security: Result<Option<TrustDecision>, Error>,
        existing: Result<Option<TrustDecision>, Error>, business: Result<bool, Error>) -> Self {
        let (hosting_enabled, business_query_error, business_query_server_code) = match business {
            Ok(enabled) => (Some(enabled), None, None),
            Err(error) => (None, Some(stored_code(error.code).unwrap_or("app_login_internal_failed").to_owned()), error.server_code),
        };
        Self { device_identity_id: snapshot.id.clone(), device_identity_revision: snapshot.revision, observed_at: at,
            security_verify: TrustQueryObservation::capture(security), existing_device: TrustQueryObservation::capture(existing),
            hosting_enabled, business_query_error, business_query_server_code,call_status:None,local_cs_forward:None,device_sync:None }
    }
    pub(super) fn capture_reuse(snapshot:&Snapshot,at:i64,status:Result<CallStatus,Error>,sync:DeviceSyncObservation)->Self {
        let mut observation=Self::capture(snapshot,at,Ok(None),Ok(None),status.map(|s|s.hosting_enabled));
        observation.security_verify=TrustQueryObservation::not_requested();
        observation.existing_device=TrustQueryObservation::not_requested();
        observation.call_status=Some(TrustQueryObservation::capture(status.map(|s|Some(TrustDecision {authentication_code:s.authentication_code}))));
        observation.local_cs_forward=status.ok().and_then(|s|s.local_cs_forward);
        observation.device_sync=Some(sync);observation
    }
    fn valid(&self) -> bool {
        let reuse_valid=match &self.call_status {
            Some(status)=>status.valid() && self.security_verify.state==TrustQueryState::NotRequested
                && self.existing_device.state==TrustQueryState::NotRequested
                && self.local_cs_forward.is_none_or(|n|(0..=999999).contains(&n))
                && self.device_sync.as_ref().is_some_and(DeviceSyncObservation::valid),
            None=>self.local_cs_forward.is_none() && self.device_sync.is_none(),
        };
        reuse_valid && valid_attempt_id(&self.device_identity_id) && self.device_identity_revision > 0 && self.observed_at >= 0
            && self.security_verify.valid() && self.existing_device.valid()
            && match (&self.hosting_enabled, &self.business_query_error) {
                (Some(_), None) => self.business_query_server_code.is_none(),
                (None, Some(error)) => stored_code(error).is_some(),
                _ => false,
            }
    }
    pub(super) fn encode(&self) -> Result<String, &'static str> {
        if !self.valid() { return Err("app_login_storage_failed"); }
        let value = serde_json::to_string(self).map_err(|_| "app_login_storage_failed")?;
        if value.len() > MAX_OBSERVATION_BYTES { return Err("app_login_storage_failed"); }
        Ok(value)
    }
    /// 数据库读取同样有界且严格；未知字段/反射值失败，不冒充历史成功。
    pub(super) fn decode(value: &str) -> Result<Self, &'static str> {
        if value.is_empty() || value.len() > MAX_OBSERVATION_BYTES { return Err("app_login_storage_failed"); }
        let observation: Self = serde_json::from_str(value).map_err(|_| "app_login_storage_failed")?;
        if !observation.valid() { return Err("app_login_storage_failed"); }
        Ok(observation)
    }
}
