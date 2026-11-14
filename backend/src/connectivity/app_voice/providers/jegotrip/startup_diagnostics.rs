//! Finite original-scope startup diagnostics, never credentials or a grant.
//! Capturing/reading these values does not open IO, retire a scope, retry work,
//! alter an error, or restore readiness. Only first failure of this trace stays.
use super::{common_runtime::CommonIoError,netstream_runtime::NetstreamError,
    native_router::RouterConnectError,router_runtime::RouterIoError,
    rpc_pending::RpcConnectionScope,
    session::AttemptOwner,transport_actor::RuntimeError};
use serde::Serialize;
use std::{fmt,sync::Mutex};

#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="snake_case")]
pub enum StartupPhase { Bootstrap, Account }
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="snake_case")]
pub enum StartupStage { Preparation, Attempt, RouterConnect, RouterTls, RouterControl, RouterIo, LogicalChannel, CommonFraming }

#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
pub struct StartupFailure {
    pub phase:StartupPhase,
    pub stage:StartupStage,
    pub code:&'static str,
}
#[derive(Clone,Copy,PartialEq,Eq)]
struct Failure {stage:StartupStage,code:&'static str}

pub(crate) struct StartupTrace {
    owner:AttemptOwner,scope:RpcConnectionScope,first:Mutex<Option<Failure>>,
}
impl fmt::Debug for StartupTrace {
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("StartupTrace(<redacted>)")}
}
impl StartupTrace {
    pub(super) fn original(owner:AttemptOwner,scope:RpcConnectionScope)->Self {
        Self {owner,scope,first:Mutex::new(None)}
    }
    fn record(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,failure:Failure) {
        // Errors often close their original scope before returning. History is
        // readable after that close, but never under a different owner/scope.
        if &self.owner!=owner || !self.scope.matches(scope) {return;}
        let mut first=self.first.lock().unwrap_or_else(|p|p.into_inner());
        if first.is_none() {*first=Some(failure);}
    }
    pub(super) fn snapshot(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,phase:StartupPhase)->Option<StartupFailure> {
        if &self.owner!=owner || !self.scope.matches(scope) {return None;}
        self.first.lock().unwrap_or_else(|p|p.into_inner()).map(|failure|StartupFailure {phase,stage:failure.stage,code:failure.code})
    }
    pub(super) fn runtime(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,error:RuntimeError) {
        let code=match error {
            RuntimeError::Scope=>"jegotrip_startup_scope_invalid",RuntimeError::InvalidConfiguration=>"jegotrip_startup_configuration_invalid",
            RuntimeError::Backpressure=>"jegotrip_startup_backpressure",RuntimeError::Closed=>"jegotrip_startup_closed",
            RuntimeError::Cancelled=>"jegotrip_startup_cancelled",RuntimeError::Timeout=>"jegotrip_startup_timeout",
            RuntimeError::Io=>"jegotrip_startup_io_failed",RuntimeError::Protocol=>"jegotrip_startup_protocol_failed",
            RuntimeError::SessionRevoked=>"jegotrip_startup_session_revoked",RuntimeError::CallNotReady=>"jegotrip_startup_call_not_ready",
            RuntimeError::CallRejected=>"jegotrip_startup_call_rejected",RuntimeError::UnregistrationRejected=>"jegotrip_startup_unregister_rejected",
            RuntimeError::LoggedOut=>"jegotrip_startup_logged_out",RuntimeError::Tls(value)=>value.code(),
        };
        // A whole-attempt timeout/close is not proof of a precise network
        // stage. A more specific captured error, if any, keeps its first slot.
        let stage=match error {RuntimeError::Scope|RuntimeError::InvalidConfiguration=>StartupStage::Preparation,_=>StartupStage::Attempt};
        self.record(owner,scope,Failure {stage,code});
    }
    pub(super) fn router(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,error:RouterIoError) {
        self.record(owner,scope,router_failure(error));
    }
    pub(super) fn netstream(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,error:NetstreamError) {
        let code=match error {
            NetstreamError::Scope=>"jegotrip_logical_channel_closed",NetstreamError::Configuration=>"jegotrip_logical_channel_configuration_invalid",
            NetstreamError::Timeout=>"jegotrip_logical_channel_timeout",NetstreamError::Entropy=>"jegotrip_logical_channel_entropy_failed",
            NetstreamError::Router(value)=>router_failure(value).code,NetstreamError::Session(_)=>"jegotrip_logical_channel_session_failed",
        };
        self.record(owner,scope,Failure {stage:StartupStage::LogicalChannel,code});
    }
    pub(super) fn common(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,error:CommonIoError) {
        let code=match error {
            CommonIoError::Scope=>"jegotrip_common_startup_closed",CommonIoError::Configuration=>"jegotrip_common_startup_configuration_invalid",
            CommonIoError::Timeout=>"jegotrip_common_startup_timeout",CommonIoError::Packet(_)=>"jegotrip_common_startup_packet_failed",
            CommonIoError::Control(_)=>"jegotrip_common_startup_control_failed",CommonIoError::Netstream(_)=>"jegotrip_common_startup_channel_failed",
        };
        self.record(owner,scope,Failure {stage:StartupStage::CommonFraming,code});
    }
}
fn router_failure(error:RouterIoError)->Failure {
    let (stage,code)=match error {
        RouterIoError::Connect(value)=>(StartupStage::RouterConnect,value.code()),
        RouterIoError::Tls(value)=>(StartupStage::RouterTls,value.code()),
        RouterIoError::Control(value)=>(StartupStage::RouterControl,match value {
            RouterConnectError::Configuration=>"jegotrip_router_control_configuration_invalid",
            RouterConnectError::Pending=>"jegotrip_router_control_pending",RouterConnectError::Timeout=>"jegotrip_router_control_timeout",
            RouterConnectError::ClockRollback=>"jegotrip_router_control_clock_rollback",RouterConnectError::Rejected=>"jegotrip_router_control_rejected",
            RouterConnectError::Protocol=>"jegotrip_router_control_protocol_failed",
        }),
        RouterIoError::Scope=>(StartupStage::RouterIo,"jegotrip_router_scope_closed"),RouterIoError::Io=>(StartupStage::RouterIo,"jegotrip_router_io_failed"),
        RouterIoError::Timeout=>(StartupStage::RouterIo,"jegotrip_router_timeout"),RouterIoError::Entropy=>(StartupStage::RouterIo,"jegotrip_router_entropy_failed"),
        RouterIoError::Configuration=>(StartupStage::RouterIo,"jegotrip_router_configuration_invalid"),
    };
    Failure {stage,code}
}
