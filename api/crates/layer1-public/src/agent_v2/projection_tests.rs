use super::{fixtures::*, projection};
use async_trait::async_trait;
use layer2_core::{
    resource_access::*,
    sandbox::{store::*, AgentListQuery, AgentLogKind, OwnerSandboxQuery},
    AgentId, Caller, SubjectId,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct SnapshotProbe {
    inner: Arc<InMemorySandboxStore>,
    reads: AtomicUsize,
}

#[async_trait]
impl ResourceAccessStore for SnapshotProbe {
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.resource_access_snapshot(target).await
    }

    async fn organization_sandboxes(
        &self,
        _: &SubjectId,
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!("event polls do not list Sandboxes")
    }

    async fn shared_sandboxes(
        &self,
        _: &SubjectId,
        _: &[SubjectId],
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!("event polls do not list shares")
    }

    async fn organization_agents(
        &self,
        _: &SubjectId,
        _: &AgentListQuery,
        _: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!("event polls do not list Agents")
    }
}

#[async_trait]
impl ResourceShareStore for SnapshotProbe {
    async fn share_mutation_receipt(
        &self,
        _: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        unreachable!("event polls do not read share mutations")
    }

    async fn mutate_share(
        &self,
        _: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        unreachable!("event polls do not mutate shares")
    }

    async fn resource_share(
        &self,
        _: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        unreachable!("event polls use the current authorization snapshot")
    }
}

struct PersonalOnly;

#[async_trait]
impl OrganizationAuthority for PersonalOnly {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        unreachable!("personal owner needs no organization")
    }

    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!("event polls do not list organizations")
    }
}

#[tokio::test]
async fn empty_tail_polls_read_current_authority_twice_and_new_events_keep_binding_checks() {
    let mut f = fixture(true, true).await;
    let probe = Arc::new(SnapshotProbe {
        inner: f.store.clone(),
        reads: AtomicUsize::new(0),
    });
    f.state.reads = f
        .state
        .reads
        .with_resource_access(Arc::new(ResourceAccessService::new(
            probe.clone(),
            Arc::new(PersonalOnly),
        )));
    let held = f
        .state
        .reads
        .authorize_event_stream(&context("owner-a"), None, &AgentId("agt_fixture".into()))
        .await
        .unwrap();
    let header = f.state.reads.event_stream_header(&held).await.unwrap();
    for message_bindings in [false, true] {
        probe.reads.store(0, Ordering::Relaxed);
        for _ in 0..10 {
            let window =
                projection::window(&f.state, &held, header.head, None, 200, message_bindings)
                    .await
                    .unwrap();
            assert!(window.entries.is_empty());
            assert_eq!(window.header.head, header.head);
        }
        assert_eq!(probe.reads.load(Ordering::Relaxed), 20);
    }

    // The next nonempty window must still bind the exact durable source command.
    let window = projection::window(&f.state, &held, 0, None, 200, true)
        .await
        .unwrap();
    assert!(window.entries.iter().any(|entry| entry
        .response_binding
        .as_ref()
        .is_some_and(|binding| binding.command_id.as_str() == "cmd_control_source")));

    append(
        &f.store,
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: header.head + 1,
            origin_key: Some("command:cmd_missing".into()),
        },
    )
    .await;
    assert!(
        projection::window(&f.state, &held, header.head, None, 200, true)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn empty_tail_poll_still_rejects_an_uncommitted_log_entry() {
    let f = fixture(false, false).await;
    let id = AgentId("agt_fixture".into());
    let held = f
        .state
        .reads
        .authorize_event_stream(&context("owner-a"), None, &id)
        .await
        .unwrap();
    let header = f.state.reads.event_stream_header(&held).await.unwrap();
    f.store
        .append_agent_log(
            &id,
            "2026-09-16T00:00:00Z".into(),
            AgentLogKind::AgentMessage {
                content: "Uncommitted fixture message".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        projection::window(&f.state, &held, header.head, None, 200, true)
            .await
            .is_err()
    );
}
