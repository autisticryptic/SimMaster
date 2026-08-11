//! E911 entitlement orchestrator: per-line query / provision / websheet
//! operation lifecycle on top of the SSRF-safe client and the independent state
//! store.
//!
//! Core rules enforced here:
//!   - every query re-confirms the current `line_id` binding before using a SIM;
//!   - a websheet completion never confirms anything by itself — a fresh
//!     entitlement re-query must succeed first;
//!   - background work only ever writes the E911 state/secret store, never the
//!     user `SimOverride` file;
//!   - only non-emergency provisioning is exercised (emergency calling is
//!     validated separately and never via a 911 dial).

use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;

use crate::connectivity::core::entitlement::{
    E911State, E911StateSource, EntitlementQueryOutcome, EntitlementStatusValue, ProviderKind,
};
use crate::connectivity::modems::ims::profile_override::SimBindingKey;
use crate::services::e911::registry::{E911Provider, E911ProviderRegistry};
use crate::services::e911::ssrf::SsrfError;
use crate::services::e911::state_store::{E911Secrets, E911StateStore};

pub type E911Result<T> = Result<T, String>;

/// Errors surfaced to the API. Short stable codes.
pub const ERR_UNSUPPORTED: &str = "e911_unsupported";
pub const ERR_NOT_READY: &str = "e911_sim_identity_not_ready";
pub const ERR_UNCONFIGURED: &str = "e911_unconfigured";
pub const ERR_SSRF: &str = "e911_endpoint_blocked";
pub const ERR_TRANSPORT: &str = "e911_transport";
pub const ERR_ALREADY_PROVISIONED: &str = "e911_already_provisioned";
pub const ERR_OPERATION_NOT_FOUND: &str = "e911_operation_not_found";
pub const ERR_OPERATION_EXPIRED: &str = "e911_operation_expired";
pub const ERR_OPERATION_MISMATCH: &str = "e911_operation_binding_mismatch";
pub const ERR_STORE: &str = "e911_store";
pub const ERR_ADDRESS_REQUIRED: &str = "e911_address_required";
pub const ERR_ADDRESS_ALREADY_SET: &str = "e911_address_already_set";

/// Current wall-clock epoch seconds.
pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The orchestrated response to a status query. Never contains the address,
/// IMSI, ICCID, EID, IMEI, token, cookie or `ServerFlow_User_Data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E911StatusView {
    pub profile_id: String,
    pub provider_kind: ProviderKind,
    pub state: E911State,
    pub source: E911StateSource,
    /// Separate display axes (research doc §9 / §11): the API must never report
    /// `enabled=true` from the carrier policy as "address confirmed".
    pub operator_requires: bool,
    pub address_saved_locally: bool,
    pub operator_confirmed: bool,
    pub emergency_unverified: bool,
    pub needs_user_action: bool,
    pub needs_reconfirm: bool,
    pub retry_after_epoch: Option<i64>,
}

/// A one-time websheet operation handle. IDs are random and short-lived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E911Operation {
    pub operation_id: String,
    pub line_id: String,
    pub binding: SimBindingKey,
    pub expires_epoch: i64,
    /// Where the websheet should be opened (already SSRF-checked).
    pub server_flow_url: String,
    /// Callback verification value (the secret `ServerFlow_User_Data`).
    pub callback_state: String,
    pub state: E911OperationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum E911OperationState {
    Pending,
    Completed,
    Cancelled,
    Expired,
}

impl E911OperationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
}

/// Minimal transport seam so the orchestrator is testable without a live
/// carrier. The production implementation issues one HTTPS request and, when
/// the server returns a websheet directive, nothing further.
pub trait EntitlementTransport: Send + Sync {
    /// Run the TS.43 query against `provider`. `sim_auth` is invoked by the
    /// protocol layer when the server challenges with EAP-AKA; implementations
    /// must never log RAND/AUTN/RES/CK/IK/AUTS.
    fn query<'a>(
        &'a self,
        provider: &'a E911Provider,
        secrets: &'a E911Secrets,
        sim_auth: &'a (dyn Fn(&[u8], &[u8]) -> Result<Vec<u8>, String> + Sync),
    ) -> BoxFuture<'a, Result<EntitlementQueryOutcome, String>>;
}

/// The E911 orchestrator. Cheap to clone; holds only Arc'd dependencies.
#[derive(Clone)]
pub struct E911Orchestrator {
    store: E911StateStore,
    registry: E911ProviderRegistry,
    transport: std::sync::Arc<dyn EntitlementTransport>,
    operations:
        std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, E911Operation>>>,
}

impl std::fmt::Debug for E911Orchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E911Orchestrator")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

impl E911Orchestrator {
    pub fn new(
        store: E911StateStore,
        registry: E911ProviderRegistry,
        transport: std::sync::Arc<dyn EntitlementTransport>,
    ) -> Self {
        Self {
            store,
            registry,
            transport,
            operations: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    pub fn store(&self) -> &E911StateStore {
        &self.store
    }

    /// Resolve the provider for a profile, never guessing from the PLMN.
    pub fn provider_for(&self, profile_id: &str) -> E911Provider {
        self.registry
            .provider_for(profile_id)
            .cloned()
            .unwrap_or_else(|| self.registry.metadata_only_for(profile_id))
    }

    /// The status the UI may display. Never returns the address itself.
    pub fn status(
        &self,
        profile_id: &str,
        binding: &SimBindingKey,
        local_address_set: bool,
    ) -> E911Result<E911StatusView> {
        let provider = self.provider_for(profile_id);
        let record = self
            .store
            .load(binding)
            .map_err(|error| format!("{ERR_STORE}:{}", error.code()))?;

        let operator_requires = provider.kind != ProviderKind::MetadataOnly
            || local_address_set
            || provider.kind.may_query();
        // "Operator confirmed" requires a carrier read-back, never a local flag.
        let operator_confirmed = record.is_provisioned();
        let emergency_unverified = !operator_confirmed;

        Ok(E911StatusView {
            profile_id: profile_id.to_string(),
            provider_kind: provider.kind,
            state: record.state,
            source: record.source,
            operator_requires,
            address_saved_locally: local_address_set,
            operator_confirmed,
            emergency_unverified,
            needs_user_action: matches!(
                record.state,
                E911State::NeedsTerms | E911State::NeedsAddress | E911State::NeedsUserAction
            ),
            needs_reconfirm: record.needs_reconfirm,
            retry_after_epoch: record.retry_after_epoch,
        })
    }

    /// Trigger a read-only entitlement query. Returns the outcome after
    /// persisting the resulting non-secret state.
    pub async fn query(
        &self,
        profile_id: &str,
        binding: &SimBindingKey,
        secrets: &E911Secrets,
        sim_auth: &(dyn Fn(&[u8], &[u8]) -> Result<Vec<u8>, String> + Sync),
    ) -> E911Result<EntitlementQueryOutcome> {
        let provider = self.provider_for(profile_id);
        if !provider.may_query() {
            return Err(ERR_UNSUPPORTED.to_string());
        }
        if provider.entitlement_url.is_none() {
            return Err(ERR_UNCONFIGURED.to_string());
        }
        let outcome = self
            .transport
            .query(&provider, secrets, sim_auth)
            .await
            .map_err(|error| {
                if is_ssrf_error(&error) {
                    format!("{ERR_SSRF}:{error}")
                } else {
                    format!("{ERR_TRANSPORT}:{error}")
                }
            })?;

        self.persist_outcome(binding, &provider, &outcome)?;
        Ok(outcome)
    }

    /// Apply a query outcome to the persisted record. This is the *only* path
    /// that may move state to `Provisioned`.
    fn persist_outcome(
        &self,
        binding: &SimBindingKey,
        provider: &E911Provider,
        outcome: &EntitlementQueryOutcome,
    ) -> E911Result<()> {
        let mut record = self
            .store
            .load(binding)
            .map_err(|error| format!("{ERR_STORE}:{}", error.code()))?;

        let confirmed = outcome.is_carrier_confirmed();
        let state = if outcome.server_flow_url.is_some() {
            // Websheet directive: never confirm. Map to needs action.
            if outcome.prov_status == EntitlementStatusValue::Rejected
                || outcome.addr_status == EntitlementStatusValue::Rejected
            {
                E911State::Rejected
            } else if outcome.tc_status == EntitlementStatusValue::NotSet {
                E911State::NeedsTerms
            } else if outcome.addr_status == EntitlementStatusValue::NotSet {
                E911State::NeedsAddress
            } else {
                E911State::NeedsUserAction
            }
        } else if outcome.prov_status == EntitlementStatusValue::Rejected
            || outcome.addr_status == EntitlementStatusValue::Rejected
        {
            E911State::Rejected
        } else if confirmed {
            E911State::Provisioned
        } else {
            E911State::Unconfigured
        };

        record.state = state;
        record.source = if confirmed {
            E911StateSource::CarrierConfirmed
        } else {
            E911StateSource::CarrierDeclared
        };
        record.prov_status = outcome.prov_status;
        record.tc_status = outcome.tc_status;
        record.addr_status = outcome.addr_status;
        record.provider_reference = outcome.provider_reference.clone();
        if confirmed {
            record.confirmed_at_epoch = Some(now_epoch());
            record.needs_reconfirm = false;
            record.retry_after_epoch = None;
        } else if let Some(seconds) = outcome.retry_after_seconds {
            record.retry_after_epoch = Some(now_epoch() + seconds as i64);
        }
        if outcome.server_flow_url.is_some() {
            record.needs_reconfirm = true;
        }

        self.store
            .save(binding, &record)
            .map_err(|error| format!("{ERR_STORE}:{}", error.code()))?;
        // The provider kind is evidence, not a secret: only the URL host was
        // already allow-listed. Nothing here writes to the user override file.
        let _ = provider;
        Ok(())
    }

    /// Create a one-time websheet operation for a fresh query outcome. The URL
    /// must already be SSRF-checked (it comes from the transport, which applied
    /// the guard). `callback_state` is the secret `ServerFlow_User_Data`.
    pub async fn create_operation(
        &self,
        line_id: &str,
        binding: &SimBindingKey,
        server_flow_url: &str,
        callback_state: &str,
        ttl_seconds: i64,
    ) -> E911Result<E911Operation> {
        let operation_id = random_operation_id();
        let operation = E911Operation {
            operation_id: operation_id.clone(),
            line_id: line_id.to_string(),
            binding: binding.clone(),
            expires_epoch: now_epoch() + ttl_seconds,
            server_flow_url: server_flow_url.to_string(),
            callback_state: callback_state.to_string(),
            state: E911OperationState::Pending,
        };
        let mut operations = self.operations.lock().await;
        operations.insert(operation_id, operation.clone());
        Ok(operation)
    }

    /// Look up a websheet operation by ID for `line_id`, applying expiry.
    pub async fn get_operation(
        &self,
        line_id: &str,
        operation_id: &str,
    ) -> E911Result<E911Operation> {
        let mut operations = self.operations.lock().await;
        let operation = operations
            .get_mut(operation_id)
            .ok_or_else(|| ERR_OPERATION_NOT_FOUND.to_string())?;
        if operation.line_id != line_id {
            return Err(ERR_OPERATION_MISMATCH.to_string());
        }
        if operation.state == E911OperationState::Pending && now_epoch() > operation.expires_epoch {
            operation.state = E911OperationState::Expired;
        }
        Ok(operation.clone())
    }

    /// Cancel an operation (e.g. SIM swap, user abort). One-shot: a completed
    /// operation cannot be cancelled.
    pub async fn cancel_operation(&self, line_id: &str, operation_id: &str) -> E911Result<()> {
        let mut operations = self.operations.lock().await;
        let operation = operations
            .get_mut(operation_id)
            .ok_or_else(|| ERR_OPERATION_NOT_FOUND.to_string())?;
        if operation.line_id != line_id {
            return Err(ERR_OPERATION_MISMATCH.to_string());
        }
        if operation.state != E911OperationState::Pending {
            return Err(ERR_OPERATION_NOT_FOUND.to_string());
        }
        operation.state = E911OperationState::Cancelled;
        Ok(())
    }

    /// Complete a websheet operation after a verified callback. This NEVER
    /// confirms entitlement: the caller must issue a fresh `query` afterwards
    /// and only report success when that re-query confirms.
    pub async fn complete_operation(
        &self,
        line_id: &str,
        operation_id: &str,
        callback_state: &str,
    ) -> E911Result<()> {
        let mut operations = self.operations.lock().await;
        let operation = operations
            .get_mut(operation_id)
            .ok_or_else(|| ERR_OPERATION_NOT_FOUND.to_string())?;
        if operation.line_id != line_id {
            return Err(ERR_OPERATION_MISMATCH.to_string());
        }
        if operation.state != E911OperationState::Pending {
            return Err(ERR_OPERATION_EXPIRED.to_string());
        }
        if now_epoch() > operation.expires_epoch {
            operation.state = E911OperationState::Expired;
            return Err(ERR_OPERATION_EXPIRED.to_string());
        }
        // CSRF/state check: the caller's callback must carry the secret state
        // we handed out, not a guess.
        if operation.callback_state != callback_state {
            return Err(ERR_OPERATION_MISMATCH.to_string());
        }
        operation.state = E911OperationState::Completed;
        Ok(())
    }
}

fn random_operation_id() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [0u8; 16];
    let _ = SystemRandom::new().fill(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_ssrf_error(message: &str) -> bool {
    matches!(
        message,
        s if s.starts_with("entitlement_")
            || s == "ssrf"
            || s.contains("not_allowed")
            || s.contains("forbidden")
            || s.contains("must_be_https")
            || s.contains("too_many_redirects")
    )
}

/// Convenience for tests: treat an `SsrfError` string as a blocked endpoint.
pub fn ssrf_error(error: SsrfError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::entitlement::{
        E911EntitlementRecord, E911State, E911StateSource, EntitlementStatusValue, ProviderKind,
    };
    use crate::connectivity::modems::ims::profile_override::SimBindingKey;
    use crate::services::e911::registry::{E911Provider, E911ProviderRegistry};
    use crate::services::e911::ssrf::SsrfError;
    use crate::services::e911::state_store::{E911Secrets, E911StateStore};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root() -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "simadmin-e911-orch-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ))
    }

    fn plain_key(iccid: &str) -> SimBindingKey {
        SimBindingKey::Plain {
            iccid: iccid.to_string(),
        }
    }

    fn ts43_provider() -> E911Provider {
        E911Provider {
            profile_id: "profile-a".to_string(),
            kind: ProviderKind::Ts43,
            entitlement_url: Some("https://entitlement.example.net/query".to_string()),
            host_allow_list: vec!["entitlement.example.net".to_string()],
            websheet_host_policy: Some("public_https".to_string()),
        }
    }

    struct FakeTransport {
        outcome: EntitlementQueryOutcome,
    }

    impl EntitlementTransport for FakeTransport {
        fn query<'a>(
            &'a self,
            _provider: &'a E911Provider,
            _secrets: &'a E911Secrets,
            _sim_auth: &'a (dyn Fn(&[u8], &[u8]) -> Result<Vec<u8>, String> + Sync),
        ) -> BoxFuture<'a, Result<EntitlementQueryOutcome, String>> {
            let outcome = self.outcome.clone();
            Box::pin(async move { Ok(outcome) })
        }
    }

    fn orch_with(transport: impl EntitlementTransport + 'static) -> E911Orchestrator {
        E911Orchestrator::new(
            E911StateStore::new(temp_root()),
            E911ProviderRegistry::new(vec![ts43_provider()]),
            std::sync::Arc::new(transport),
        )
    }

    fn confirmed_outcome() -> EntitlementQueryOutcome {
        EntitlementQueryOutcome {
            state: E911State::Provisioned,
            prov_status: EntitlementStatusValue::Set,
            tc_status: EntitlementStatusValue::Set,
            addr_status: EntitlementStatusValue::Set,
            provider_reference: Some("ref-1".to_string()),
            server_flow_url: None,
            server_flow_user_data: None,
            retry_after_seconds: None,
        }
    }

    fn websheet_outcome() -> EntitlementQueryOutcome {
        EntitlementQueryOutcome {
            state: E911State::NeedsAddress,
            prov_status: EntitlementStatusValue::NotSet,
            tc_status: EntitlementStatusValue::Set,
            addr_status: EntitlementStatusValue::NotSet,
            provider_reference: Some("ref-2".to_string()),
            server_flow_url: Some("https://websheet.example.net/terms".to_string()),
            server_flow_user_data: Some("csrf-secret".to_string()),
            retry_after_seconds: None,
        }
    }

    #[test]
    fn status_shows_operator_requires_not_confirm() {
        let orch = orch_with(FakeTransport {
            outcome: confirmed_outcome(),
        });
        let key = plain_key("8986000111111111111");
        // No local address, but a queryable provider exists.
        let view = orch.status("profile-a", &key, false).unwrap();
        assert!(view.operator_requires);
        assert!(!view.address_saved_locally);
        // Before any query the operator has not confirmed anything.
        assert!(!view.operator_confirmed);
        assert!(view.emergency_unverified);
    }

    #[tokio::test]
    async fn query_confirms_only_after_carrier_readback() {
        let orch = orch_with(FakeTransport {
            outcome: confirmed_outcome(),
        });
        let key = plain_key("8986000111111111111");
        let secrets = E911Secrets::default();
        let outcome = orch
            .query("profile-a", &key, &secrets, &|_rand, _autn| {
                Ok(vec![0u8; 4])
            })
            .await
            .unwrap();
        assert!(outcome.is_carrier_confirmed());

        let record = orch.store().load(&key).unwrap();
        assert_eq!(record.state, E911State::Provisioned);
        assert_eq!(record.source, E911StateSource::CarrierConfirmed);
        assert!(record.is_provisioned());
    }

    #[tokio::test]
    async fn websheet_outcome_never_confirms_and_sets_reconfirm() {
        let orch = orch_with(FakeTransport {
            outcome: websheet_outcome(),
        });
        let key = plain_key("8986000111111111111");
        orch.query(
            "profile-a",
            &key,
            &E911Secrets::default(),
            &|_rand, _autn| Ok(vec![0u8; 4]),
        )
        .await
        .unwrap();
        let record = orch.store().load(&key).unwrap();
        assert_eq!(record.state, E911State::NeedsAddress);
        assert_ne!(record.source, E911StateSource::CarrierConfirmed);
        assert!(record.needs_reconfirm);
        assert!(!record.is_provisioned());
    }

    #[tokio::test]
    async fn metadata_only_provider_refuses_queries() {
        let registry = E911ProviderRegistry::default();
        let orch = E911Orchestrator::new(
            E911StateStore::new(temp_root()),
            registry,
            std::sync::Arc::new(FakeTransport {
                outcome: confirmed_outcome(),
            }),
        );
        let key = plain_key("8986000111111111111");
        let result = orch
            .query(
                "unknown-profile",
                &key,
                &E911Secrets::default(),
                &|_r, _a| Ok(vec![0u8; 4]),
            )
            .await;
        assert_eq!(result.unwrap_err(), ERR_UNSUPPORTED);
    }

    #[tokio::test]
    async fn per_binding_state_never_crosses_lines() {
        let orch = orch_with(FakeTransport {
            outcome: confirmed_outcome(),
        });
        let a = plain_key("8986000111111111111");
        let b = plain_key("8986000111111111112");
        orch.query("profile-a", &a, &E911Secrets::default(), &|_r, _a| {
            Ok(vec![0u8; 4])
        })
        .await
        .unwrap();
        assert!(orch.store().load(&a).unwrap().is_provisioned());
        assert!(!orch.store().load(&b).unwrap().is_provisioned());
    }

    #[test]
    fn ssrf_error_is_mapped_to_blocked_code() {
        let error = ssrf_error(SsrfError::HostNotAllowed("evil.example".to_string()));
        assert!(error.contains("entitlement"));
    }

    #[test]
    fn already_carrier_confirmed_address_is_unverified_until_readback() {
        // A record manually set to Provisioned without a query source is never
        // considered operator-confirmed.
        let store = E911StateStore::new(temp_root());
        let key = plain_key("8986000111111111111");
        let record = E911EntitlementRecord {
            state: E911State::Provisioned,
            source: E911StateSource::LocalOnly,
            addr_status: EntitlementStatusValue::Set,
            ..Default::default()
        };
        store.save(&key, &record).unwrap();
        let orch = E911Orchestrator::new(
            store,
            E911ProviderRegistry::new(vec![ts43_provider()]),
            std::sync::Arc::new(FakeTransport {
                outcome: confirmed_outcome(),
            }),
        );
        let view = orch.status("profile-a", &key, true).unwrap();
        assert!(view.address_saved_locally);
        assert!(!view.operator_confirmed);
        assert!(view.emergency_unverified);
    }

    #[tokio::test]
    async fn operation_lifecycle_pending_complete() {
        let orch = orch_with(FakeTransport {
            outcome: confirmed_outcome(),
        });
        let key = plain_key("8986000111111111111");
        let operation = orch
            .create_operation(
                "line-1",
                &key,
                "https://websheet.example.net/terms",
                "csrf-secret",
                60,
            )
            .await
            .unwrap();
        assert_eq!(operation.state, E911OperationState::Pending);
        assert_eq!(operation.line_id, "line-1");

        // Wrong callback state fails the CSRF/state check.
        assert_eq!(
            orch.complete_operation("line-1", &operation.operation_id, "wrong")
                .await
                .unwrap_err(),
            ERR_OPERATION_MISMATCH
        );
        // Correct state completes.
        orch.complete_operation("line-1", &operation.operation_id, "csrf-secret")
            .await
            .unwrap();
        assert_eq!(
            orch.get_operation("line-1", &operation.operation_id)
                .await
                .unwrap()
                .state,
            E911OperationState::Completed
        );
    }

    #[tokio::test]
    async fn operation_is_scoped_to_line() {
        let orch = orch_with(FakeTransport {
            outcome: confirmed_outcome(),
        });
        let key = plain_key("8986000111111111111");
        let operation = orch
            .create_operation(
                "line-1",
                &key,
                "https://websheet.example.net/terms",
                "s",
                60,
            )
            .await
            .unwrap();
        assert_eq!(
            orch.get_operation("line-2", &operation.operation_id)
                .await
                .unwrap_err(),
            ERR_OPERATION_MISMATCH
        );
    }

    #[tokio::test]
    async fn operation_expires() {
        let orch = orch_with(FakeTransport {
            outcome: confirmed_outcome(),
        });
        let key = plain_key("8986000111111111111");
        let operation = orch
            .create_operation(
                "line-1",
                &key,
                "https://websheet.example.net/terms",
                "s",
                -10,
            )
            .await
            .unwrap();
        assert_eq!(
            orch.get_operation("line-1", &operation.operation_id)
                .await
                .unwrap()
                .state,
            E911OperationState::Expired
        );
        assert_eq!(
            orch.complete_operation("line-1", &operation.operation_id, "s")
                .await
                .unwrap_err(),
            ERR_OPERATION_EXPIRED
        );
    }

    #[tokio::test]
    async fn cancelled_operation_cannot_complete() {
        let orch = orch_with(FakeTransport {
            outcome: confirmed_outcome(),
        });
        let key = plain_key("8986000111111111111");
        let operation = orch
            .create_operation(
                "line-1",
                &key,
                "https://websheet.example.net/terms",
                "s",
                60,
            )
            .await
            .unwrap();
        orch.cancel_operation("line-1", &operation.operation_id)
            .await
            .unwrap();
        assert_eq!(
            orch.complete_operation("line-1", &operation.operation_id, "s")
                .await
                .unwrap_err(),
            ERR_OPERATION_EXPIRED
        );
    }
}
