//! Explicit Actions-only factory using the actual stored profile and normal
//! connector, with a policy-specific scheduling hold inside its old deadline.
use super::{account_config::AccountConfig,account_runtime::AccountBackendFactory,
    connect_failure_detail::ConnectFailureStep,router_connect_detail_test_gate::RouterConnectTestGate,
    runtime_backend::{BackendEvent,NativeHostingBackend},runtime_connector::TcpRuntimeConnector};
use std::{sync::{Arc,Mutex,atomic::{AtomicUsize,Ordering}},time::Duration};
use tokio::sync::mpsc;

pub(crate) struct ConnectDetailFactory {
    step:ConnectFailureStep,
    gate:Mutex<Option<Arc<RouterConnectTestGate>>>,builds:AtomicUsize,
}
impl ConnectDetailFactory {
    pub(crate) fn for_step(step:ConnectFailureStep)->Arc<Self>{Arc::new(Self{step,gate:Mutex::new(None),builds:AtomicUsize::new(0)})}
    pub(crate) fn gate(&self)->Arc<RouterConnectTestGate>{self.gate.lock().unwrap().as_ref().unwrap().clone()}
    pub(crate) fn builds(&self)->usize{self.builds.load(Ordering::SeqCst)}
}
impl AccountBackendFactory for ConnectDetailFactory {
    fn build(&self,config:&AccountConfig)->Result<(Arc<NativeHostingBackend>,mpsc::Receiver<BackendEvent>),&'static str>{
        let mut profile=config.runtime_profile()?;
        let gate=profile.connect_detail_test_gate(self.step).map_err(|_|"app_account_configuration_invalid")?;
        let connector=TcpRuntimeConnector::new(Duration::from_millis(config.profile.connect_timeout_ms)).map_err(|_|"app_account_configuration_invalid")?;
        assert!(self.gate.lock().unwrap().is_none(),"one actual fixture backend only");
        *self.gate.lock().unwrap()=Some(gate);self.builds.fetch_add(1,Ordering::SeqCst);
        Ok(NativeHostingBackend::new(profile,Arc::new(connector)))
    }
}
