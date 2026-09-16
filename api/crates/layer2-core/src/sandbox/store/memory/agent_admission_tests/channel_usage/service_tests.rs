use super::*;
use crate::resource_access::*;
use crate::sandbox::store::{AgentChannelUsageSnapshot, Page};
use crate::{Authorization, Caller, Capability};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: crate::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: SubjectId::new(subject),
            credential_id: "credential".into(),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new(subject),
        },
        memberships: vec![],
    }
}
struct Authority;
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let mut context = context(caller.subject.as_str());
        let now = SystemTime::now();
        context.memberships.push(OrganizationMembership::new(
            context.credential.clone(),
            org.clone(),
            OrganizationRole::Viewer,
            now,
            now + Duration::from_secs(1),
        )?);
        Ok(context)
    }
}
struct Buffered {
    store: Arc<InMemorySandboxStore>,
    gate: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl Buffered {
    async fn wait(&self) {
        if self.gate.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
    }
}
#[async_trait::async_trait]
impl AgentChannelUsageStore for Buffered {
    async fn agent_channel_usage_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<AgentChannelUsageSnapshot, SandboxStoreError> {
        let snapshot = self.store.agent_channel_usage_snapshot(id).await?;
        self.wait().await;
        Ok(snapshot)
    }
    async fn record_agent_channel_usage(
        &self,
        request: ChannelUsageMutation,
    ) -> Result<ChannelUsageEvent, SandboxStoreError> {
        self.store.record_agent_channel_usage(request).await
    }
    async fn agent_channel_usage_history(
        &self,
        id: &AgentId,
        page: &PageRequest,
    ) -> Result<(AgentChannelUsageSnapshot, Page<ChannelUsageEvent>), SandboxStoreError> {
        let result = self.store.agent_channel_usage_history(id, page).await?;
        self.wait().await;
        Ok(result)
    }
}
struct Fixture {
    store: Arc<InMemorySandboxStore>,
    buffered: Arc<Buffered>,
    service: Arc<AgentChannelUsageService>,
    access: Arc<ResourceAccessService>,
    id: AgentId,
    share: ResourceShare,
}
async fn fixture() -> Fixture {
    let store = Arc::new(ready_store(3).await);
    let id = created(store.admit_agent(admission("usage-read")).await.unwrap())
        .agent
        .id;
    attach(&store, &id).await;
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(Authority),
    ));
    let share = access
        .change_share(
            &context("owner"),
            Some("credential"),
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: id.clone(),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: "share".into(),
            },
        )
        .await
        .unwrap()
        .share;
    let buffered = Arc::new(Buffered {
        store: store.clone(),
        gate: AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let service = AgentChannelUsageService::new(buffered.clone(), access.clone());
    Fixture {
        store,
        buffered,
        service,
        access,
        id,
        share,
    }
}

#[tokio::test]
async fn channel_usage_service_conceals_binding_and_actor_ids_from_shared_readers() {
    let f = fixture().await;
    let shared = f
        .service
        .read(&context("recipient"), Some("credential"), &f.id, None)
        .await
        .unwrap();
    assert!(shared.summary.active && shared.summary.has_history);
    assert!(shared.active_bindings.is_none());
    assert!(shared.history.is_none());
    let broad = serde_json::to_string(&shared.summary).unwrap();
    for private in ["bridge-one", "partition-one", "binding-one", "owner"] {
        assert!(!broad.contains(private));
    }
    assert!(f
        .service
        .read(
            &context("recipient"),
            Some("credential"),
            &f.id,
            Some(&PageRequest::default())
        )
        .await
        .is_err());
    let owner = f
        .service
        .read_personal(
            &context("owner").caller,
            &f.id,
            Some(&PageRequest::default()),
        )
        .await
        .unwrap();
    assert_eq!(
        owner.active_bindings.unwrap()[0].reference.binding_id,
        "binding-one"
    );
    assert_eq!(owner.history.unwrap().items.len(), 1);
    assert_eq!(f.store.lock().channel_usage_events.len(), 1);
}

#[tokio::test]
async fn channel_usage_service_revocation_during_buffered_read_refuses_disclosure() {
    let f = fixture().await;
    f.buffered.gate.store(true, Ordering::SeqCst);
    let service = f.service.clone();
    let id = f.id.clone();
    let read = tokio::spawn(async move {
        service
            .read(&context("recipient"), Some("credential"), &id, None)
            .await
    });
    f.buffered.entered.notified().await;
    f.access
        .change_share(
            &context("owner"),
            Some("credential"),
            ShareCommand {
                selector: f.share.selector,
                change: ShareChange::Revoke {
                    share_id: f.share.id,
                },
                expected_share_revision: Some(f.share.revision),
                idempotency_key: "revoke".into(),
            },
        )
        .await
        .unwrap();
    f.buffered.release.notify_one();
    assert!(read.await.unwrap().is_err());
    assert_eq!(
        f.store
            .agent_channel_usage_snapshot(&f.id)
            .await
            .unwrap()
            .state
            .revision,
        1
    );
}

#[tokio::test(start_paused = true)]
async fn channel_usage_service_preserves_original_membership_deadline_during_buffered_read() {
    let f = fixture().await;
    f.buffered.gate.store(true, Ordering::SeqCst);
    let service = f.service.clone();
    let id = f.id.clone();
    let read = tokio::spawn(async move {
        service
            .read(&context("recipient"), Some("credential"), &id, None)
            .await
    });
    f.buffered.entered.notified().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    f.buffered.release.notify_one();
    assert!(read.await.unwrap().is_err());
    assert_eq!(f.store.lock().channel_usage_events.len(), 1);
}
