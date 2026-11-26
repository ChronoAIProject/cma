use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    response::Response,
    Router,
};
use layer2_core::profiles::store::{AgentProfileStore, InMemoryProfileStore};
use layer2_core::profiles::{
    AgentProfile, AgentProfileContent, PermissionLevel, ProfileVisibility, SandboxProfileContent,
    SandboxProfileSnapshot, ThinkingEffort,
};
use layer2_core::sandbox::store::{InMemorySandboxStore, SandboxEventStore, SandboxStore};
use layer2_core::sandbox::{
    RuntimeBinding, Sandbox, SandboxCredentialState, SandboxEventKind, SandboxLimits,
    SandboxOrigin, SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep,
    StoredSandboxRetirement,
};
use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityQuery, ActivityRecord, ActivityService, ActivityStore,
    ActivityTarget, Authorization, Capability, Page, PlatformSettings, PlatformSettingsService,
    PlatformSettingsStore, ProviderInventory, RuntimeEntry, RuntimeInventory, RuntimeStatus,
    SandboxId, StoreError, StoredPlatformSettings, SubjectId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;
use crate::auth::AuthenticatedSubject;

#[path = "admin/admission_tests.rs"]
pub(crate) mod admission_control_tests;

#[path = "admin/ceilings/tests.rs"]
mod admission_ceilings_tests;

const NOW: &str = "2026-09-05T00:00:00Z";

#[derive(Default)]
pub(crate) struct MemoryActivity {
    records: Mutex<Vec<ActivityRecord>>,
}

#[async_trait]
impl ActivityStore for MemoryActivity {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }

    async fn list(&self, query: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        let mut records: Vec<_> = self
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| {
                query
                    .actor
                    .as_ref()
                    .is_none_or(|actor| record.actor.as_ref() == Some(actor))
                    && query.action.is_none_or(|action| record.action == action)
                    && !(query.exclude_successful_reads
                        && matches!(
                            record.action,
                            ActivityAction::Read
                                | ActivityAction::List
                                | ActivityAction::GatewayRecordRead
                        )
                        && record.outcome == ActivityOutcome::Succeeded)
            })
            .take(query.limit as usize)
            .cloned()
            .collect();
        if query.cursor.as_deref() == Some("after-first") && !records.is_empty() {
            records.remove(0);
        }
        Ok(Page::new(records, None))
    }
}

pub(crate) struct MemorySettings {
    stored: Mutex<StoredPlatformSettings>,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self {
            stored: Mutex::new(StoredPlatformSettings {
                revision: 0,
                settings: PlatformSettings::default(),
            }),
        }
    }
}

#[async_trait]
impl PlatformSettingsStore for MemorySettings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        Ok(self.stored.lock().unwrap().clone())
    }

    async fn replace(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        let mut stored = self.stored.lock().unwrap();
        if stored.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: stored.revision,
            });
        }
        *stored = StoredPlatformSettings {
            revision: expected_revision + 1,
            settings: settings.clone(),
        };
        Ok(stored.clone())
    }
}

struct FixedInventory;

#[async_trait]
impl RuntimeInventory for FixedInventory {
    async fn snapshot(&self) -> Result<ProviderInventory, StoreError> {
        Ok(ProviderInventory {
            observed_at: NOW.into(),
            runtimes: vec![RuntimeEntry {
                runtime_id: "runtime-a".into(),
                generation: 3,
                owner_reference: Some("sbx_a".into()),
                status: RuntimeStatus::Ready,
            }],
        })
    }
}

pub(crate) struct Fixture {
    pub(crate) app: Router,
    pub(crate) sandboxes: Arc<InMemorySandboxStore>,
    pub(crate) profiles: Arc<InMemoryProfileStore>,
    activity: Arc<MemoryActivity>,
    pub(crate) names: Arc<crate::rest::public_names::tests::Names>,
}

pub(crate) fn fixture(inventory: bool) -> Fixture {
    fixture_with_pools(inventory, None)
}

pub(crate) fn fixture_with_pools(
    inventory: bool,
    pools: Option<Arc<dyn warm_pools::WarmPoolAdministration>>,
) -> Fixture {
    fixture_with_configure(inventory, pools, |state, _| state)
}

fn fixture_with_options(
    inventory: bool,
    pools: Option<Arc<dyn warm_pools::WarmPoolAdministration>>,
    diagnostics: Option<Arc<layer2_core::binding_diagnostics::BindingDiagnosticsService>>,
) -> Fixture {
    fixture_with_configure(inventory, pools, |state, _| match diagnostics {
        Some(service) => state.with_binding_diagnostics(service),
        None => state,
    })
}

pub(crate) fn fixture_with_configure(
    inventory: bool,
    pools: Option<Arc<dyn warm_pools::WarmPoolAdministration>>,
    configure: impl FnOnce(AdminState, &Arc<InMemorySandboxStore>) -> AdminState,
) -> Fixture {
    let activity = Arc::new(MemoryActivity::default());
    let activity_service = ActivityService::new(activity.clone());
    let settings = Arc::new(PlatformSettingsService::new(
        Arc::new(MemorySettings::default()),
        Arc::clone(&activity_service),
    ));
    let sandboxes = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let retirement = Arc::new(StoredSandboxRetirement::new(sandboxes.clone()));
    let operators = Arc::new(
        layer2_core::sandbox::SandboxOperatorService::new(
            sandboxes.clone(),
            profiles.clone(),
            retirement,
            Arc::clone(&activity_service),
        )
        .with_maintenance(sandboxes.clone()),
    );
    let names = crate::rest::public_names::tests::Names::seeded();
    let mut state = AdminState::new(settings, operators, activity_service)
        .with_user_directory(layer2_core::UserDirectory::new(names.clone()))
        .with_maintenance(sandboxes.clone())
        .with_runtime_updates(Arc::new(
            layer2_core::sandbox::SandboxRuntimeUpdateService::new(
                sandboxes.clone(),
                "runtime-v2".into(),
            ),
        ));
    if inventory {
        state = state.with_runtime_inventory(Arc::new(FixedInventory));
    }
    if let Some(pools) = pools {
        state = state.with_warm_pools(pools);
    }
    Fixture {
        app: router(configure(state, &sandboxes)),
        sandboxes,
        profiles,
        activity,
        names,
    }
}

#[path = "admin/binding_holds_tests.rs"]
pub(in crate::rest) mod binding_holds_tests;

fn request(
    method: Method,
    path: &str,
    body: Option<Value>,
    capabilities: impl IntoIterator<Item = Capability>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    let body = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    authenticated(builder.body(body).unwrap(), capabilities)
}

fn raw_request(
    method: Method,
    path: &str,
    body: &'static str,
    capabilities: impl IntoIterator<Item = Capability>,
) -> Request<Body> {
    authenticated(
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap(),
        capabilities,
    )
}

fn authenticated(
    mut request: Request<Body>,
    capabilities: impl IntoIterator<Item = Capability>,
) -> Request<Body> {
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId::new("operator-a")));
    request
        .extensions_mut()
        .insert(Authorization::new(capabilities));
    request
}

async fn body_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(&body_bytes(response).await).unwrap()
}

pub(crate) fn sandbox(id: &str, owner: &str, version: &str) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId::new(owner),
        name: format!("Sandbox {id}"),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: layer2_core::SandboxProfileId("sbp_test".into()),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: layer2_core::profiles::AgentProvider::Codex,
                model_access: layer2_core::profiles::ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: Some(RuntimeBinding {
            boot_id: None,
            provision_generation: Some(1),
            runtime_id: format!("runtime-{id}"),
            runtime_version: version.into(),
            bound_at: NOW.into(),
        }),
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: Vec::new(),
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        lifecycle: layer2_core::sandbox::SandboxLifecycleState {
            runtime_generation_high_water: 1,
            ..Default::default()
        },
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

pub(crate) fn agent_profile(id: &str, owner: &str, visibility: ProfileVisibility) -> AgentProfile {
    AgentProfile {
        internal: false,
        id: layer2_core::AgentProfileId(id.into()),
        owner: SubjectId::new(owner),
        name: format!("Agent {id}"),
        description: String::new(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            isolated_agent: false,
            model: None,
            thinking_effort: ThinkingEffort::Medium,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: String::new(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

async fn send(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
    capabilities: impl IntoIterator<Item = Capability>,
) -> Response {
    app.clone()
        .oneshot(request(method, path, body, capabilities))
        .await
        .unwrap()
}

#[path = "admin/warm_pools_tests.rs"]
mod warm_pool_tests;

#[path = "admin/runtime_update_tests.rs"]
mod runtime_update_tests;

#[path = "admin/usage_reporting_tests.rs"]
mod usage_reporting_tests;

#[path = "admin/route_contract_1_tests.rs"]
mod route_contract_1;

#[path = "admin/route_contract_2_tests.rs"]
mod route_contract_2;
#[path = "admin/runtime_migration_tests.rs"]
mod runtime_migration_tests;
