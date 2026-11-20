use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use layer2_core::{
    profiles::InMemoryProfileStore,
    sandbox::{InMemorySandboxStore, SandboxOperatorService, StoredSandboxRetirement},
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Authorization, Capability, Page,
    PlatformSettings, PlatformSettingsService, PlatformSettingsStore, StoreError,
    StoredPlatformSettings, SubjectId,
};
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    hosted::{HostedAgentOpenSettings, HostedBackend},
    provider::ProviderSandboxRequest,
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeHandle,
    RuntimeLifecycle, RuntimeObservation,
};
use serde_json::Value;
use tower::ServiceExt;

use crate::config::FeatureFlags;

#[derive(Clone)]
pub(super) enum Reply {
    Complete(Vec<RuntimeObservation>),
    Unsupported,
    Failure,
    Incomplete,
    Pending,
}

pub(super) struct Provider {
    pub calls: AtomicUsize,
    pub cancelled: AtomicUsize,
    pub reply: Mutex<Reply>,
}
impl Provider {
    pub fn new(reply: Reply) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
            reply: Mutex::new(reply),
        })
    }
}

struct PendingCall<'a>(&'a AtomicUsize);
impl Drop for PendingCall<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl HostedBackend for Provider {
    async fn inventory(&self) -> layer3_adapter::Result<Vec<RuntimeObservation>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let reply = self.reply.lock().unwrap().clone();
        match reply {
            Reply::Complete(rows) => Ok(rows),
            Reply::Unsupported => Err(AdapterError::Unsupported("inventory unavailable".into())),
            Reply::Failure => Err(AdapterError::Unavailable("provider failed".into())),
            Reply::Incomplete => Err(AdapterError::Rejected(
                "inventory pagination incomplete".into(),
            )),
            Reply::Pending => {
                let _pending = PendingCall(&self.cancelled);
                std::future::pending().await
            }
        }
    }
    fn native_pools(&self) -> Option<&dyn layer3_adapter::agent_sandbox::NativePoolControl> {
        panic!("runtime inventory must not query warm pools")
    }
    async fn provision(&self, _: ProviderSandboxRequest) -> layer3_adapter::Result<RuntimeHandle> {
        panic!("inventory cannot provision")
    }
    async fn exchange(
        &self,
        _: &RuntimeHandle,
        _: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        panic!("inventory cannot contact a runtime")
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("inventory cannot open an Agent")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> layer3_adapter::Result<()> {
        panic!("inventory cannot delete a runtime")
    }
}

struct UnusedStores;
#[async_trait]
impl ActivityStore for UnusedStores {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        panic!("inventory cannot write activity")
    }
    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        panic!("inventory does not read activity")
    }
}
#[async_trait]
impl PlatformSettingsStore for UnusedStores {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        panic!("inventory does not read settings")
    }
    async fn replace(
        &self,
        _: u64,
        _: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        panic!("inventory cannot write settings")
    }
}

pub(super) fn flags(value: Option<&str>) -> Result<FeatureFlags, String> {
    FeatureFlags::from_lookup(&mut |key| {
        Ok(if key == "CHRONO_ADMIN_RUNTIME_INVENTORY_ENABLED" {
            value.map(str::to_owned)
        } else {
            None
        })
    })
}

pub(super) struct Manager {
    pub source: &'static str,
    app: Router,
}
impl Manager {
    pub fn new(source: &'static str, provider: Arc<Provider>, flags: FeatureFlags) -> Self {
        let activity = ActivityService::new(Arc::new(UnusedStores));
        let settings = Arc::new(PlatformSettingsService::new(
            Arc::new(UnusedStores),
            activity.clone(),
        ));
        let sandboxes = Arc::new(InMemorySandboxStore::default());
        let operators = Arc::new(SandboxOperatorService::new(
            sandboxes.clone(),
            Arc::new(InMemoryProfileStore::default()),
            Arc::new(StoredSandboxRetirement::new(sandboxes)),
            activity.clone(),
        ));
        let adapter = Arc::new(CodexAdapter::new(provider, CodexSandboxSpec::default()));
        let state = super::super::admin_state(flags, settings, operators, activity, adapter);
        Self {
            source,
            app: layer1_public::rest::admin::router(state),
        }
    }
    pub async fn get(&self, authorized: bool) -> (StatusCode, Value) {
        let capabilities = if authorized {
            vec![Capability::ReadRuntimeInventory]
        } else {
            vec![]
        };
        let request = Request::get("/api/v1/admin/runtime-inventory")
            .header("x-request-id", format!("{}-inventory", self.source))
            .extension(layer1_public::AuthenticatedSubject(SubjectId::new(
                "inventory-reader",
            )))
            .extension(Authorization::new(capabilities))
            .body(Body::empty())
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
}

pub(super) fn row(id: &str, generation: u64, state: &str) -> RuntimeObservation {
    RuntimeObservation {
        sandbox_id: format!("sbx_{id}"),
        generation,
        handle: RuntimeHandle(format!("runtime-{id}")),
        image_sha256: "a".repeat(43),
        spec_fingerprint: "b".repeat(43),
        runtime_contract: "3".into(),
        lifecycle: RuntimeLifecycle {
            state: state.into(),
            reason: Some("private provider diagnostic".into()),
        },
    }
}
