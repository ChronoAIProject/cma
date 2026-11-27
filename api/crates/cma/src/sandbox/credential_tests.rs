use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use layer2_core::profiles::{
    AgentProvider, ModelAccess, SandboxProfileContent, SandboxProfileSnapshot,
};
use layer2_core::sandbox::{
    SandboxCredentialAuthority, SandboxCredentialState, SandboxLimits, SandboxOrigin,
    SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep,
};
use layer2_core::{SandboxId, SandboxProfileId, ServiceId, SubjectId};

use super::*;

#[path = "credential/legacy_admission_tests.rs"]
mod legacy_admission_tests;
#[path = "credential/protected_fixture.rs"]
mod protected_fixture;
use protected_fixture::protected_issue;

#[path = "credential/model_catalog_tests.rs"]
mod model_catalog_tests;
#[path = "credential/node_access_tests.rs"]
mod node_access_tests;
#[path = "credential/provided_tests.rs"]
mod provided_tests;
#[path = "credential/retirement_reauthorization_tests.rs"]
mod retirement_reauthorization_tests;

#[derive(Default)]
struct Tokens {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl OwnerTokenSource for Tokens {
    async fn token_for(&self, _owner: &SubjectId) -> Result<Option<SecretValue>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Some(SecretValue::new("owner-token".into())))
    }
}

#[derive(Default)]
struct MemoryCustody {
    values: Mutex<BTreeMap<(SandboxId, u64), CredentialMaterial>>,
    revocations: Mutex<BTreeMap<(SandboxId, String), CredentialRevocation>>,
    corrupt: AtomicBool,
    intent: Mutex<Option<CredentialIssuance>>,
}

#[async_trait::async_trait]
impl SandboxCredentialCustody for MemoryCustody {
    async fn insert(&self, material: CredentialMaterial) -> Result<(), CredentialCustodyError> {
        if material.source.is_managed() {
            return Err(CredentialCustodyError::Unavailable(
                "protected issuance required".into(),
            ));
        }
        self.seed_retained(material)
    }

    async fn issuance(
        &self,
        _: &Sandbox,
    ) -> Result<Option<CredentialIssuance>, CredentialCustodyError> {
        Ok(self.intent.lock().unwrap().clone())
    }
    async fn begin_issuance(
        &self,
        _: &Sandbox,
        intent: &CredentialIssuance,
    ) -> Result<bool, CredentialCustodyError> {
        *self.intent.lock().unwrap() = Some(intent.clone());
        Ok(true)
    }
    async fn accept_issuance(
        &self,
        _: &Sandbox,
        intent: &CredentialIssuance,
        material: &CredentialMaterial,
    ) -> Result<(), CredentialCustodyError> {
        self.seed_retained(material.clone())?;
        let mut accepted = intent.clone();
        accepted.state = IssuanceState::Accepted;
        *self.intent.lock().unwrap() = Some(accepted);
        Ok(())
    }

    async fn get(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialCustodyError> {
        if self.corrupt.load(Ordering::SeqCst) {
            return Err(CredentialCustodyError::Corrupt(
                "ciphertext integrity validation failed".into(),
            ));
        }
        Ok(self
            .values
            .lock()
            .expect("custody lock")
            .get(&(sandbox_id.clone(), generation))
            .cloned())
    }

    async fn list(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialCustodyError> {
        Ok(self
            .values
            .lock()
            .expect("custody lock")
            .iter()
            .filter(|((id, _), _)| id == sandbox_id)
            .map(|(_, material)| material.clone())
            .collect())
    }

    async fn delete(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<(), CredentialCustodyError> {
        self.values
            .lock()
            .expect("custody lock")
            .remove(&(sandbox_id.clone(), generation));
        Ok(())
    }

    async fn queue_revocation(
        &self,
        revocation: CredentialRevocation,
    ) -> Result<(), CredentialCustodyError> {
        self.revocations.lock().expect("revocation lock").insert(
            (
                revocation.sandbox_id.clone(),
                revocation.provider_key_id.clone(),
            ),
            revocation,
        );
        Ok(())
    }

    async fn revocations(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialRevocation>, CredentialCustodyError> {
        Ok(self
            .revocations
            .lock()
            .expect("revocation lock")
            .iter()
            .filter(|((id, _), _)| id == sandbox_id)
            .map(|(_, revocation)| revocation.clone())
            .collect())
    }

    async fn delete_revocation(
        &self,
        sandbox_id: &SandboxId,
        provider_key_id: &str,
    ) -> Result<(), CredentialCustodyError> {
        self.revocations
            .lock()
            .expect("revocation lock")
            .remove(&(sandbox_id.clone(), provider_key_id.to_string()));
        Ok(())
    }
}

#[derive(Default)]
struct ProviderState {
    mints: AtomicUsize,
    revoke_failures: AtomicUsize,
    revoked: Mutex<Vec<String>>,
    mint_requests: Mutex<Vec<serde_json::Value>>,
    rebound: AtomicBool,
}

async fn services() -> Json<serde_json::Value> {
    Json(serde_json::json!([{
        "id": "provider-service-id",
        "slug": "service-a",
        "name": "Service A",
        "is_active": true
    }, {
        "id": "provider-github-id",
        "slug": "api-github",
        "name": "GitHub",
        "is_active": true
    }]))
}

async fn mint(
    State(state): State<Arc<ProviderState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    state.mint_requests.lock().unwrap().push(body.clone());
    let number = state.mints.fetch_add(1, Ordering::SeqCst) + 1;
    Json(serde_json::json!({
        "id": format!("provider-key-{number}"),
        "full_key": format!("plaintext-key-{number}"),
        "scopes": "proxy read",
        "state_version": number,
        "allowed_service_ids": body["allowed_service_ids"],
        "allowed_node_ids": body["allowed_node_ids"],
        "allow_all_services": false,
        "allow_all_nodes": false
    }))
}

async fn revoke(State(state): State<Arc<ProviderState>>, Path(id): Path<String>) -> StatusCode {
    state.revoked.lock().expect("revoked lock").push(id);
    if state
        .revoke_failures
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
            remaining.checked_sub(1)
        })
        .is_ok()
    {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::NO_CONTENT
    }
}

async fn provider() -> (String, Arc<ProviderState>, tokio::task::JoinHandle<()>) {
    let state = Arc::new(ProviderState::default());
    let app = Router::new()
        .route("/api/v1/user-services", get(services))
        .route("/api/v1/api-keys/scope-plan", post(node_access_tests::plan))
        .route("/proxy", get(node_access_tests::proxy))
        .route("/api/v1/api-keys", post(mint))
        .route("/api/v1/api-keys/{id}", delete(revoke))
        .with_state(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("provider fixture binds");
    let base_url = format!(
        "http://{}",
        listener.local_addr().expect("provider address")
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("provider fixture serves");
    });
    (base_url, state, server)
}

fn sandbox() -> layer2_core::sandbox::Sandbox {
    let now = "2026-09-05T00:00:00Z".to_string();
    layer2_core::sandbox::Sandbox {
        id: SandboxId("sbx_credential_test".into()),
        owner: SubjectId::new("owner"),
        name: "credential-test".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_credential_test".into()),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 1_024,
                nyxid_services: vec![ServiceId::new("service-a")],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 1,
                max_agents_per_workspace: 1,
                workspace_profiles: Vec::new(),
                software_install_commands: Vec::new(),
                environment_variables: Vec::new(),
            },
        },
        status: SandboxStatus::Installing,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![ServiceId::new("service-a")],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: Vec::new(),
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::MintInitialCredential, now.clone()),
        limits: SandboxLimits {
            max_workspaces: 1,
            max_agents_per_workspace: 1,
        },
        lifecycle: Default::default(),
        last_activity_at: now.clone(),
        created_at: now.clone(),
        updated_at: now,
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

fn material(owner: SubjectId, provider_key_id: &str) -> CredentialMaterial {
    CredentialMaterial {
        source: Default::default(),
        github_route: None,
        sandbox_id: SandboxId("sbx_credential_test".into()),
        owner,
        authority: SandboxCredentialAuthority {
            generation: 1,
            provider_key_id: provider_key_id.into(),
            provider_state_version: 1,
            issued_at: "2026-09-05T00:00:00Z".into(),
            expires_at: "2026-10-05T00:00:00Z".into(),
            grant_fingerprint: "a".repeat(64),
        },
        value: SecretValue::new("winner-plaintext".into()),
    }
}

#[tokio::test]
async fn service_access_never_reuses_pregrant_or_stale_worker_custody() {
    let (base_url, provider, server) = provider().await;
    let custody = Arc::new(MemoryCustody::default());
    let controller = ManagedSandboxCredentials::new(
        Arc::new(KeyIssuer::new(reqwest::Client::new(), base_url, "test")),
        Arc::new(Tokens::default()),
        custody,
    );
    let mut before = sandbox();
    let initial = protected_issue(&controller, &before, 1).await.unwrap();
    before.credential.key_generation = 1;
    before.credential.current = Some(initial.authority.clone());
    before.credential.delivered = true;
    before.status = SandboxStatus::Paused;
    before.work.step = SandboxWorkStep::None;
    before.lifecycle.checkpoints.latest = Some(
        layer2_core::sandbox::SandboxCheckpointRef::seal(
            &layer3_adapter::CheckpointReceipt {
                epoch: 1,
                object_reference: "checkpoints/service-access".into(),
                sha256: "a".repeat(64),
                writers: layer3_adapter::WriterState::Resumed,
            },
            &[],
            before.updated_at.clone(),
        )
        .unwrap(),
    );
    let mut granted = before.clone();
    granted
        .grant_service_access(
            &before.owner,
            &[ServiceId::new("api-github")],
            before.updated_at.clone(),
        )
        .unwrap();
    let stale = protected_issue(&controller, &before, 2).await.unwrap();
    for generation in [1, 2] {
        assert!(matches!(
            controller
                .issue(&granted, generation, SystemTime::UNIX_EPOCH)
                .await,
            Err(CredentialError::Security(_))
        ));
    }
    let fresh = protected_issue(&controller, &granted, 3).await.unwrap();
    assert_ne!(
        fresh.authority.provider_key_id,
        stale.authority.provider_key_id
    );
    assert_ne!(
        fresh.authority.grant_fingerprint,
        stale.authority.grant_fingerprint
    );
    assert_eq!(provider.mints.load(Ordering::SeqCst), 3);
    assert_eq!(
        provider.mint_requests.lock().unwrap()[2]["allowed_service_ids"],
        serde_json::json!(["provider-github-id", "provider-service-id"])
    );
    let replay = protected_issue(&controller, &granted, 3).await.unwrap();
    assert_eq!(replay.authority, fresh.authority);
    assert_eq!(provider.mints.load(Ordering::SeqCst), 3);
    server.abort();
}

#[tokio::test]
async fn corrupt_custody_is_a_security_failure_without_minting() {
    let (base_url, provider, server) = provider().await;
    let custody = Arc::new(MemoryCustody::default());
    custody.corrupt.store(true, Ordering::SeqCst);
    let controller = ManagedSandboxCredentials::new(
        Arc::new(KeyIssuer::new(reqwest::Client::new(), base_url, "test")),
        Arc::new(Tokens::default()),
        custody,
    );

    assert!(matches!(
        controller
            .issue(&sandbox(), 1, SystemTime::UNIX_EPOCH)
            .await,
        Err(CredentialError::Security(_))
    ));
    assert_eq!(provider.mints.load(Ordering::SeqCst), 0);
    server.abort();
}

#[test]
fn credential_diagnostics_never_reveal_plaintext() {
    let material = material(SubjectId::new("owner"), "provider-key");
    assert!(!format!("{material:?}").contains("winner-plaintext"));
}
