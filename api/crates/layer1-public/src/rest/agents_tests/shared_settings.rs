use super::*;
use crate::rest::resource_access_tests::context;
use crate::rest::shared_agent_tests::{Authority, Reply};
use crate::rest::{
    agent_models_router, AgentModelCatalog, AgentModelCatalogError, AgentModelRuntimeSupport,
    AgentModelView,
};
use layer2_core::resource_access::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const ID: &str = "agt_shared_settings";
const RECIPIENT: &str = "settings-recipient";

struct Registry {
    input: layer2_core::sandbox::AgentRuntimeInputSender,
    revocation: Mutex<Option<(Arc<ResourceAccessService>, ResourceShare, usize)>>,
    resolves: AtomicUsize,
}

#[async_trait]
impl AgentRuntimeRegistry for Registry {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let count = self.resolves.fetch_add(1, Ordering::SeqCst) + 1;
        let revoke = {
            let mut guard = self.revocation.lock().unwrap();
            if guard.as_ref().is_some_and(|(_, _, at)| *at == count) {
                guard.take()
            } else {
                None
            }
        };
        if let Some((access, share, _)) = revoke {
            revoke_share(&access, &share).await;
        }
        Ok(AgentRuntimeConnection {
            capabilities: agent_capabilities(),
            input: self.input.clone(),
        })
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

async fn setup(
    mode: ShareMode,
) -> (
    Fixture,
    Arc<ResourceAccessService>,
    ResourceShare,
    Arc<Registry>,
    Arc<AtomicUsize>,
) {
    let writes = Arc::new(AtomicUsize::new(0));
    let count = writes.clone();
    let registry = Arc::new(Registry {
        input: layer2_core::sandbox::AgentRuntimeInputSender::spawn(4, move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        }),
        revocation: Mutex::new(None),
        resolves: AtomicUsize::new(0),
    });
    let org = Arc::new(Authority(Mutex::new(Reply::Member)));
    let fixture = fixture_with_ports(registry.clone(), None, Some(org.clone())).await;
    let agent = seeded_agent(ID, AgentStatus::Ready, 0, None);
    fixture
        .store
        .create_worktree(seeded_worktree(&agent))
        .await
        .unwrap();
    fixture.store.create_agent(agent).await.unwrap();
    let access = Arc::new(ResourceAccessService::new(fixture.store.clone(), org));
    let token = "b".repeat(64);
    let created = access
        .create_agent_share_link(
            &context(OWNER),
            None,
            CreateAgentShareLink {
                agent_id: AgentId(ID.into()),
                audience: AgentShareAudience::Public,
                mode,
                token,
                idempotency_key: "settings-share".into(),
            },
        )
        .await
        .unwrap();
    access
        .redeem_agent_share_link(
            &context(RECIPIENT),
            None,
            &created.share.id,
            &"b".repeat(64),
        )
        .await
        .unwrap();
    let share = access
        .read_share(
            &context(OWNER),
            None,
            &ResourceSelector::Agent {
                agent_id: AgentId(ID.into()),
            },
            &created.share.id,
        )
        .await
        .unwrap();
    (fixture, access, share, registry, writes)
}

async fn revoke_share(access: &ResourceAccessService, share: &ResourceShare) {
    access
        .change_share(
            &context(OWNER),
            None,
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: AgentId(ID.into()),
                },
                change: ShareChange::Revoke {
                    share_id: share.id.clone(),
                },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-settings-share".into(),
            },
        )
        .await
        .unwrap();
}

fn shared_request(
    method: Method,
    suffix: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = request(
        method,
        &format!("/api/v1/agents/{ID}/{suffix}"),
        RECIPIENT,
        key,
        body,
    );
    request.extensions_mut().insert(context(RECIPIENT));
    request
}

fn selection() -> Value {
    json!({"model":"gpt-6","reasoning_effort":"high","permission_mode":"autonomous"})
}

#[tokio::test]
async fn shared_settings_write_permission_is_live_on_reads_updates_and_cached_replay() {
    for mode in [ShareMode::View, ShareMode::ViewWrite] {
        let (fixture, access, share, _, writes) = setup(mode).await;
        let (status, _) = json_response(
            &fixture.router,
            shared_request(Method::GET, "settings", None, None),
        )
        .await;
        assert_eq!(status.is_success(), mode == ShareMode::ViewWrite);
        let (status, first) = json_response(
            &fixture.router,
            shared_request(
                Method::PATCH,
                "settings",
                Some("selection"),
                Some(selection()),
            ),
        )
        .await;
        assert_eq!(status.is_success(), mode == ShareMode::ViewWrite, "{first}");
        if mode == ShareMode::ViewWrite {
            assert_eq!(
                first,
                json!({"model":"gpt-6","reasoning_effort":"high","permission_mode":"autonomous","pending":false})
            );
            let (_, replay) = json_response(
                &fixture.router,
                shared_request(
                    Method::PATCH,
                    "settings",
                    Some("selection"),
                    Some(selection()),
                ),
            )
            .await;
            assert_eq!(replay, first);
            assert_eq!(writes.load(Ordering::SeqCst), 1);
            let stored = fixture
                .store
                .get_agent(&AgentId(ID.into()))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stored.owner.as_str(), OWNER);
        } else {
            assert_eq!(writes.load(Ordering::SeqCst), 0);
        }
        revoke_share(&access, &share).await;
        for method in [Method::GET, Method::PATCH] {
            let body = (method == Method::PATCH).then(selection);
            let (status, _) = json_response(
                &fixture.router,
                shared_request(method, "settings", Some("selection"), body),
            )
            .await;
            assert!(!status.is_success());
        }
        let (status, _) = json_response(
            &fixture.router,
            request(
                Method::GET,
                &format!("/api/v1/agents/{ID}/settings"),
                OWNER,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
}

#[tokio::test]
async fn shared_settings_revocation_before_transaction_or_ordered_write_prevents_effect() {
    for at in [1, 2] {
        let (fixture, access, share, registry, writes) = setup(ShareMode::ViewWrite).await;
        let before = fixture
            .store
            .get_agent(&AgentId(ID.into()))
            .await
            .unwrap()
            .unwrap()
            .settings();
        *registry.revocation.lock().unwrap() = Some((access, share, at));
        let (status, _) = json_response(
            &fixture.router,
            shared_request(
                Method::PATCH,
                "settings",
                Some("selection"),
                Some(selection()),
            ),
        )
        .await;
        assert!(!status.is_success());
        assert_eq!(writes.load(Ordering::SeqCst), 0, "revoked at resolve {at}");
        let stored = fixture
            .store
            .get_agent(&AgentId(ID.into()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.settings(), before);
        assert!(!stored.settings_pending());
    }
}

struct Catalog {
    calls: AtomicUsize,
    revoke: Option<(Arc<ResourceAccessService>, ResourceShare)>,
}
#[async_trait]
impl AgentModelCatalog for Catalog {
    async fn models(&self, _: &Sandbox) -> Result<Vec<AgentModelView>, AgentModelCatalogError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some((access, share)) = &self.revoke {
            revoke_share(access, share).await;
        }
        Ok(vec![AgentModelView {
            id: "gpt-6".into(),
            label: "GPT-6".into(),
            available: true,
        }])
    }
}
struct Support;
#[async_trait]
impl AgentModelRuntimeSupport for Support {
    async fn supported(&self, _: &Sandbox) -> bool {
        true
    }
}

#[tokio::test]
async fn shared_models_require_write_access_and_fence_buffered_results_after_provider_io() {
    for (mode, revoke) in [
        (ShareMode::View, false),
        (ShareMode::ViewWrite, false),
        (ShareMode::ViewWrite, true),
    ] {
        let (fixture, access, share, _, _) = setup(mode).await;
        let catalog = Arc::new(Catalog {
            calls: AtomicUsize::new(0),
            revoke: revoke.then_some((access, share)),
        });
        let router = agent_models_router(
            fixture.state.reads.clone(),
            catalog.clone(),
            Arc::new(Support),
        );
        let (status, body) =
            json_response(&router, shared_request(Method::GET, "models", None, None)).await;
        assert_eq!(
            status.is_success(),
            mode == ShareMode::ViewWrite && !revoke,
            "{body}"
        );
        assert_eq!(
            catalog.calls.load(Ordering::SeqCst),
            usize::from(mode == ShareMode::ViewWrite)
        );
    }
}
