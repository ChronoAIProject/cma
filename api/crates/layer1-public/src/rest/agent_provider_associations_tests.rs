use super::{
    agents_tests,
    resource_access_tests::context,
    shared_agent_tests::{fixture, Reply},
};
use axum::{
    http::{Method, StatusCode},
    Router,
};
use layer2_core::{
    profiles::{content_digest, AgentProvider, ProfileCapacityBounds},
    resource_access::*,
    sandbox::{
        binding::*,
        provider_associations::*,
        store::{AgentStore, InMemorySandboxStore, PageRequest, SandboxStoreError},
        Agent, AgentStatus,
    },
    AgentId, Authorization, Capability, SubjectId,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub(super) const NOW: &str = "2026-09-22T00:00:00Z";
pub(super) const ID: &str = "agt_shared_read";
const BASE: &str = "/api/v1/agents/agt_shared_read/provider-associations";

pub(super) fn binding(agent: &Agent, id: &str, borrowed: bool) -> ProviderBinding {
    let input = json!({"schema_version":1,
        "metadata":{"name":"association-test","owner":{"kind":"human","subject":agent.owner.as_str()},"provider":"registered"},
        "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,"api_type":"supported","model":"test","thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"},"context":{},
        "environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}});
    let digest = format!("sha256:{}", content_digest(&input).unwrap());
    let profile = map_profile(
        input,
        &digest,
        AgentProvider::default(),
        "supported",
        ProfileCapacityBounds {
            min_cpu_millis: 250,
            max_cpu_millis: 8000,
            min_memory_mib: 512,
            max_memory_mib: 16384,
        },
    )
    .unwrap();
    let mut value = ProviderBinding::new(
        id.into(),
        SubjectId::new("provider-manager"),
        profile,
        content_digest(&id).unwrap(),
        NOW.into(),
    )
    .unwrap();
    value.sandbox_id = agent.sandbox_id.clone();
    value.workspace_id = agent.workspace_id.clone();
    if borrowed {
        value.existing_agent = Some(ExistingBindingAgent {
            agent_id: agent.id.clone(),
            worktree_id: agent.worktree_id.clone(),
        });
        value.level = 1;
        value.state = BindingState::Ready;
        value.work = BindingWork::Idle;
    }
    value
}
pub(super) async fn attach(
    store: &InMemorySandboxStore,
    id: &str,
    agent_id: &str,
    borrowed: bool,
) -> BindingConversation {
    let agent = store
        .get_agent(&AgentId(agent_id.into()))
        .await
        .unwrap()
        .unwrap();
    let mut parent = binding(&agent, id, borrowed);
    store.save_binding(None, parent.clone()).await.unwrap();
    if !borrowed {
        parent.state = BindingState::Ready;
        parent.work = BindingWork::Idle;
        parent.advance_revision(NOW.into()).unwrap();
        store.save_binding(Some(1), parent.clone()).await.unwrap();
    }
    let mut conversation =
        BindingConversation::new(&parent, "event-0001".into(), NOW.into()).unwrap();
    conversation.agent_id = agent.id;
    conversation.worktree_id = agent.worktree_id;
    store
        .save_binding_conversation(parent.revision, None, conversation.clone())
        .await
        .unwrap();
    conversation
}
pub(super) async fn retire(store: &InMemorySandboxStore, mut conversation: BindingConversation) {
    let mut parent = store
        .binding(&conversation.binding_id)
        .await
        .unwrap()
        .unwrap();
    for state in [
        BindingConversationState::Retiring,
        BindingConversationState::Retired,
    ] {
        let expected = conversation.revision;
        conversation.state = state;
        conversation.revision += 1;
        store
            .save_binding_conversation(parent.revision, Some(expected), conversation.clone())
            .await
            .unwrap();
    }
    let expected = parent.revision;
    parent.state = BindingState::Retired;
    parent.work = BindingWork::Idle;
    parent.advance_revision(NOW.into()).unwrap();
    store.save_binding(Some(expected), parent).await.unwrap();
}
async fn response(router: &Router, subject: &str, suffix: &str) -> (StatusCode, Value) {
    let mut request =
        agents_tests::request(Method::GET, &format!("{BASE}{suffix}"), subject, None, None);
    request.extensions_mut().insert(context(subject));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("request-credential".into()));
    agents_tests::json_response(router, request).await
}
#[tokio::test]
async fn owner_reads_borrowed_associations_paging_retry_and_retirement_without_changing_agent() {
    let (f, access, _, _) = fixture(Reply::Member).await;
    let before = f.store.get_agent(&AgentId(ID.into())).await.unwrap();
    let first = attach(&f.store, "binding-a", ID, true).await;
    attach(&f.store, "binding-b", ID, true).await;
    let router = super::agent_provider_associations_router(AgentProviderAssociationService::new(
        f.store.clone(),
        access,
    ));
    let (status, page) = response(&router, "owner-a", "?limit=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"][0]["binding_id"], "binding-a");
    assert_eq!(page["items"][0]["relationship"], "borrowed");
    assert_eq!(page, response(&router, "owner-a", "?limit=1").await.1);
    let next = page["next"].as_str().unwrap();
    let (_, second) = response(&router, "owner-a", &format!("?limit=1&cursor={next}")).await;
    assert_eq!(second["items"][0]["binding_id"], "binding-b");
    assert!(second.get("next").is_none());
    retire(&f.store, first).await;
    let (_, retired) = response(&router, "owner-a", "").await;
    assert_eq!(retired["items"][0]["conversation_state"], "retired");
    assert_eq!(retired["items"][0]["binding_state"], "retired");
    assert_eq!(retired["items"][1]["binding_state"], "ready");
    assert_eq!(
        before,
        f.store.get_agent(&AgentId(ID.into())).await.unwrap()
    );
    let encoded = retired.to_string();
    for secret in [
        "provider-manager",
        "key_sha256",
        "tool_endpoint",
        "profile_digest",
        "first_used_at",
        "last_used_at",
    ] {
        assert!(!encoded.contains(secret));
    }
}
#[tokio::test]
async fn per_event_agents_share_parent_without_mixing_relationships_or_origin() {
    let (f, access, _, _) = fixture(Reply::Member).await;
    let first = attach(&f.store, "binding-events", ID, false).await;
    let second_agent = agents_tests::insert_agent(
        &f.store,
        agents_tests::seeded_agent("agt_event_two", AgentStatus::Ready, 0, None),
        vec![],
    )
    .await;
    let parent = f.store.binding("binding-events").await.unwrap().unwrap();
    let mut second = BindingConversation::new(&parent, "event-0002".into(), NOW.into()).unwrap();
    second.agent_id = second_agent.id.clone();
    second.worktree_id = second_agent.worktree_id.clone();
    f.store
        .save_binding_conversation(parent.revision, None, second)
        .await
        .unwrap();
    let service = AgentProviderAssociationService::new(f.store.clone(), access);
    for (id, conversation) in [
        (first.agent_id, "event-0001"),
        (second_agent.id, "event-0002"),
    ] {
        let read = service
            .read_personal(
                &context("owner-a").caller,
                &id,
                &PageRequest::new(50, None).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read.items.len(), 1);
        assert_eq!(read.items[0].provider_conversation_id, conversation);
        assert_eq!(
            read.items[0].relationship,
            ProviderAssociationRelationship::Managed
        );
    }
}
#[tokio::test]
async fn foreign_shared_worker_and_manager_credentials_never_gain_owner_read_authority() {
    let (f, access, _, _) = fixture(Reply::Member).await;
    attach(&f.store, "binding-private", ID, true).await;
    let router = super::agent_provider_associations_router(AgentProviderAssociationService::new(
        f.store, access,
    ));
    for subject in ["other-owner", "provider-manager", "recipient"] {
        let (status, body) = response(&router, subject, "").await;
        assert!(matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
        ));
        assert!(!body.to_string().contains("binding-private"));
    }
    // Even if a worker's verified subject equals the resource owner, its
    // provider capability does not become ambient owner-management authority.
    let mut request = agents_tests::request(Method::GET, BASE, "owner-a", None, None);
    request
        .extensions_mut()
        .insert(Authorization::new([Capability::ManageProviderBindings]));
    assert!(matches!(
        agents_tests::json_response(&router, request).await.0,
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    let mut request = agents_tests::request(Method::GET, BASE, "owner-a", None, None);
    request
        .extensions_mut()
        .remove::<crate::AuthenticatedSubject>();
    assert_eq!(
        agents_tests::json_response(&router, request).await.0,
        StatusCode::UNAUTHORIZED
    );
    let request = agents_tests::request(Method::POST, BASE, "owner-a", None, None);
    assert_eq!(
        agents_tests::response(&router, request).await.0,
        StatusCode::METHOD_NOT_ALLOWED
    );
}
#[tokio::test]
async fn query_limits_cursor_scope_and_corrupt_rows_are_not_empty_success() {
    let (f, access, _, _) = fixture(Reply::Member).await;
    attach(&f.store, "binding-a", ID, true).await;
    attach(&f.store, "binding-b", ID, true).await;
    let router = super::agent_provider_associations_router(AgentProviderAssociationService::new(
        f.store.clone(),
        access,
    ));
    for suffix in ["?limit=0", "?limit=201", "?cursor=broken", "?unexpected=1"] {
        assert_eq!(
            response(&router, "owner-a", suffix).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let snapshot = f
        .store
        .agent_provider_associations(
            &SubjectId::new("owner-a"),
            &AgentId(ID.into()),
            &PageRequest::new(1, None).unwrap(),
        )
        .await
        .unwrap();
    let first = association_page(
        &snapshot,
        &SubjectId::new("owner-a"),
        &AgentId(ID.into()),
        &PageRequest::new(1, None).unwrap(),
    )
    .unwrap();
    let mut creation = snapshot
        .provenance
        .creation_provenance(&AgentId(ID.into()))
        .unwrap();
    let cursor_page = PageRequest::new(1, first.next).unwrap();
    for changed in [0, 1, 2] {
        match changed {
            0 => creation.agent_id = AgentId("another-agent".into()),
            1 => {
                creation.owner = ResourceOwner::Person {
                    subject: SubjectId::new("foreign"),
                }
            }
            _ => creation.agent_created_at = "2026-09-06T00:00:00Z".into(),
        }
        assert_eq!(
            association_cursor(&cursor_page, &creation),
            Err(SandboxStoreError::InvalidPage)
        );
    }
    for changed in 0..7 {
        let mut broken = snapshot.clone();
        match changed {
            0 => broken.records[0].conversation.owner = SubjectId::new("foreign"),
            1 => {
                broken.records[0].conversation.worktree_id = layer2_core::WorktreeId("wrong".into())
            }
            2 => broken.records[0].binding.owner = SubjectId::new("foreign"),
            3 => broken.records[0].binding.profile.organization_owned = true,
            4 => broken.records.reverse(),
            5 => broken.records.push(broken.records[0].clone()),
            _ => broken.records[0].conversation.generation = 2,
        }
        assert!(association_page(
            &broken,
            &SubjectId::new("owner-a"),
            &AgentId(ID.into()),
            &PageRequest::new(1, None).unwrap()
        )
        .is_err());
    }
}
#[tokio::test]
async fn retained_historical_generation_does_not_require_current_parent_environment() {
    let (f, _, _, _) = fixture(Reply::Member).await;
    attach(&f.store, "binding-events", ID, false).await;
    let id = AgentId(ID.into());
    let owner = SubjectId::new("owner-a");
    let page = PageRequest::new(50, None).unwrap();
    let mut snapshot = f
        .store
        .agent_provider_associations(&owner, &id, &page)
        .await
        .unwrap();
    // The current and previous environment may have advanced beyond the
    // retained conversation. Membership must not disappear after generation 3.
    let parent = &mut snapshot.records[0].binding;
    parent.generation = 3;
    parent.sandbox_id = layer2_core::SandboxId("sbx_replacement".into());
    parent.workspace_id = layer2_core::WorkspaceId("wks_replacement".into());
    parent.previous = None;
    parent.validate().unwrap();
    snapshot.records[0].conversation.state = BindingConversationState::Retired;
    let read = association_page(&snapshot, &owner, &id, &page).unwrap();
    assert!(!read.items[0].is_current_generation);
    assert_eq!(read.items[0].current_binding_generation, 3);
    assert_eq!(read.items[0].sandbox_id, "sbx_route");
    snapshot.records[0].conversation.generation = 3;
    assert!(association_page(&snapshot, &owner, &id, &page).is_err());
}

struct Buffered {
    store: Arc<InMemorySandboxStore>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl AgentProviderAssociationStore for Buffered {
    async fn agent_provider_associations(
        &self,
        owner: &SubjectId,
        id: &AgentId,
        page: &PageRequest,
    ) -> Result<AgentProviderAssociationSnapshot, SandboxStoreError> {
        let snapshot = self
            .store
            .agent_provider_associations(owner, id, page)
            .await;
        self.entered.notify_one();
        self.release.notified().await;
        snapshot
    }
}
#[tokio::test]
async fn policy_revocation_during_buffered_read_discards_associations() {
    let (f, access, _, share) = fixture(Reply::Member).await;
    attach(&f.store, "binding-private", ID, true).await;
    let buffered = Arc::new(Buffered {
        store: f.store,
        entered: Default::default(),
        release: Default::default(),
    });
    let service = AgentProviderAssociationService::new(buffered.clone(), access.clone());
    let read = tokio::spawn(async move {
        service
            .read_personal(
                &context("owner-a").caller,
                &AgentId(ID.into()),
                &PageRequest::new(50, None).unwrap(),
            )
            .await
    });
    buffered.entered.notified().await;
    access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-during-association-read".into(),
            },
        )
        .await
        .unwrap();
    buffered.release.notify_one();
    assert!(matches!(
        read.await.unwrap(),
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Conflict
        ))
    ));
}

#[tokio::test]
async fn worktree_recovery_preserves_proved_membership_but_rejects_concurrent_or_forged_lineage() {
    use super::agents_tests::worktree_recovery::{recovery_fixture, ID as RECOVERY_ID};
    let f = recovery_fixture().await;
    let association = attach(&f.store, "binding-recovery", RECOVERY_ID, false).await;
    let original_worktree = association.worktree_id.to_string();
    let access = Arc::new(ResourceAccessService::new(
        f.store.clone(),
        Arc::new(super::shared_agent_tests::Authority(std::sync::Mutex::new(
            Reply::Member,
        ))),
    ));
    let buffered = Arc::new(Buffered {
        store: f.store.clone(),
        entered: Default::default(),
        release: Default::default(),
    });
    let service = AgentProviderAssociationService::new(buffered.clone(), access);
    let read = tokio::spawn(async move {
        service
            .read_personal(
                &context("owner-a").caller,
                &AgentId(RECOVERY_ID.into()),
                &PageRequest::new(50, None).unwrap(),
            )
            .await
    });
    buffered.entered.notified().await;
    let request = agents_tests::request(
        Method::POST,
        &format!("/api/v1/agents/{RECOVERY_ID}/recover-worktree"),
        "owner-a",
        Some("association-recovery"),
        None,
    );
    assert_eq!(
        agents_tests::json_response(&f.router, request).await.0,
        StatusCode::ACCEPTED
    );
    buffered.release.notify_one();
    assert!(matches!(
        read.await.unwrap(),
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Conflict
        ))
    ));
    let owner = SubjectId::new("owner-a");
    let id = AgentId(RECOVERY_ID.into());
    let page = PageRequest::new(50, None).unwrap();
    let snapshot = f
        .store
        .agent_provider_associations(&owner, &id, &page)
        .await
        .unwrap();
    let current_worktree = &snapshot
        .provenance
        .resource
        .lineage
        .agent
        .as_ref()
        .unwrap()
        .worktree_id;
    assert_ne!(current_worktree.as_str(), original_worktree);
    assert_eq!(
        association_page(&snapshot, &owner, &id, &page)
            .unwrap()
            .items[0]
            .worktree_id,
        original_worktree
    );
    for changed in 0..3 {
        let mut broken = snapshot.clone();
        match changed {
            0 => {
                broken
                    .provenance
                    .resource
                    .lineage
                    .agent
                    .as_mut()
                    .unwrap()
                    .work
                    .worktree_recoveries[0]
                    .original_worktree
                    .owner = SubjectId::new("foreign-owner")
            }
            1 => {
                broken
                    .provenance
                    .resource
                    .lineage
                    .agent
                    .as_mut()
                    .unwrap()
                    .work
                    .worktree_recoveries[0]
                    .successor_worktree_id = layer2_core::WorktreeId("unproved-successor".into())
            }
            _ => {
                broken.records[0].conversation.worktree_id =
                    layer2_core::WorktreeId("unproved-old-worktree".into())
            }
        }
        assert!(association_page(&broken, &owner, &id, &page).is_err());
    }
}
