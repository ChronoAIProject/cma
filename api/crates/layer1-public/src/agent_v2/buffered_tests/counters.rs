use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Counted {
    store: Arc<InMemorySandboxStore>,
    snapshots: AtomicUsize,
    deny_at: AtomicUsize,
    purged: AtomicBool,
    transferred: AtomicBool,
}
#[async_trait]
impl ResourceAccessStore for Counted {
    async fn organization_sandboxes(
        &self,
        _: &SubjectId,
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("page read listed organization Sandboxes")
    }
    async fn shared_sandboxes(
        &self,
        _: &SubjectId,
        _: &[SubjectId],
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("page read listed shared Sandboxes")
    }
    async fn organization_agents(
        &self,
        _: &SubjectId,
        _: &AgentListQuery,
        _: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("page read listed organization Agents")
    }
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        let call = self.snapshots.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.deny_at.load(Ordering::SeqCst) || self.purged.load(Ordering::SeqCst) {
            return Err(ResourceAccessStoreError::NotFound);
        }
        let mut snapshot = self.store.resource_access_snapshot(target).await?;
        if self.transferred.load(Ordering::SeqCst) {
            // Ownership is immutable through save_agent. Model a consistent
            // replacement at the authority port, not an invalid partial write.
            assert!(snapshot.policy.is_none() && snapshot.shares.is_empty());
            let owner = SubjectId::new("changed-owner");
            snapshot.lineage.sandbox.owner = owner.clone();
            snapshot.lineage.workspace.as_mut().unwrap().owner = owner.clone();
            snapshot.lineage.worktree.as_mut().unwrap().owner = owner.clone();
            snapshot.lineage.agent.as_mut().unwrap().owner = owner;
            snapshot.scope(target).unwrap();
        }
        Ok(snapshot)
    }
}
#[async_trait]
impl ResourceShareStore for Counted {
    async fn share_mutation_receipt(
        &self,
        _: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        panic!("read sought a mutation receipt")
    }
    async fn mutate_share(
        &self,
        _: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        panic!("read mutated a share")
    }
    async fn resource_share(
        &self,
        _: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        panic!("read bypassed its authorization snapshot")
    }
}
struct NoExternal;
#[async_trait]
impl OrganizationAuthority for NoExternal {
    async fn organization_context(
        &self,
        _: &layer2_core::Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("personal read requested membership")
    }
    async fn organization_choices(
        &self,
        _: &layer2_core::Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("personal read enumerated organizations")
    }
}
#[async_trait]
impl SandboxResourceReader for NoExternal {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("history read inspected runtime resources")
    }
}

async fn counted(
    f: &Fixture,
) -> (
    Arc<AgentReadService>,
    AgentEventStreamAuthorization,
    Arc<Counted>,
) {
    let count = Arc::new(Counted {
        store: f.store.clone(),
        snapshots: AtomicUsize::new(0),
        deny_at: AtomicUsize::new(usize::MAX),
        purged: AtomicBool::new(false),
        transferred: AtomicBool::new(false),
    });
    let access = Arc::new(ResourceAccessService::new(
        count.clone(),
        Arc::new(NoExternal),
    ));
    let reads =
        AgentReadService::new(f.store.clone(), Arc::new(NoExternal)).with_resource_access(access);
    let held = reads
        .authorize_event_stream(&context("owner-a"), None, &AgentId("agt_fixture".into()))
        .await
        .unwrap();
    assert_eq!(count.snapshots.load(Ordering::SeqCst), 1);
    (reads, held, count)
}

#[tokio::test]
async fn page_and_active_or_idle_head_use_exactly_three_snapshots_and_current_header() {
    for active in [false, true] {
        for head in [false, true] {
            let f = fixture(active, false).await;
            assert!(!f.state.single_fence_pages);
            let (reads, held, count) = counted(&f).await;
            append(
                &f.store,
                AgentLogKind::TitleSet {
                    title: "after entry authorization".into(),
                    source: "owner".into(),
                },
            )
            .await;
            let current = f
                .store
                .get_agent(&AgentId("agt_fixture".into()))
                .await
                .unwrap()
                .unwrap();
            let window = reads
                .buffered_event_window(&held, &f.state.commands, |header| {
                    assert_eq!(header.head, current.conversation.last_seq);
                    Ok::<_, AgentReadError>(if head {
                        AgentBufferedEventRequest::Head
                    } else {
                        AgentBufferedEventRequest::Page {
                            after: 0,
                            through: header.head,
                            limit: 100,
                            message_bindings: true,
                        }
                    })
                })
                .await
                .unwrap();
            assert_eq!(window.header.head, current.conversation.last_seq);
            assert_eq!(count.snapshots.load(Ordering::SeqCst), 3);
        }
    }
}

#[tokio::test]
async fn selector_and_invalid_coordinate_errors_cannot_bypass_the_final_fence() {
    for deny in [false, true] {
        for invalid_page in [false, true] {
            let f = fixture(true, false).await;
            let (reads, held, count) = counted(&f).await;
            if deny {
                count.deny_at.store(3, Ordering::SeqCst);
            }
            let result = reads
                .buffered_event_window(&held, &f.state.commands, |header| {
                    if invalid_page {
                        Ok(AgentBufferedEventRequest::Page {
                            after: 0,
                            through: header.head,
                            limit: 0,
                            message_bindings: false,
                        })
                    } else {
                        Err(AgentReadError::Unavailable(
                            "private selector detail".into(),
                        ))
                    }
                })
                .await;
            let error = match result {
                Ok(_) => panic!("invalid selection published a page"),
                Err(error) => error,
            };
            assert_eq!(count.snapshots.load(Ordering::SeqCst), 3);
            if deny {
                assert_eq!(error, AgentReadError::NotFound);
            } else if invalid_page {
                assert!(matches!(error, AgentReadError::InvalidPage));
            } else {
                assert!(matches!(error, AgentReadError::Unavailable(_)));
            }
            let bytes = to_bytes(Error::from(error).response().into_body(), 8192)
                .await
                .unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("private selector detail"));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn dependency_failure_and_timeout_each_run_one_final_snapshot() {
    for fail in [false, true] {
        let f = fixture(true, false).await;
        let (reads, held, count) = counted(&f).await;
        let (state, gate, resume) = gated(&f, fail);
        let pending = tokio::spawn(async move {
            reads
                .buffered_event_window(&held, &state.commands, |header| {
                    Ok::<_, AgentReadError>(AgentBufferedEventRequest::Page {
                        after: 0,
                        through: header.head,
                        limit: 100,
                        message_bindings: false,
                    })
                })
                .await
        });
        entered(&gate).await;
        assert_eq!(count.snapshots.load(Ordering::SeqCst), 2);
        if fail {
            resume.notify_one();
        } else {
            tokio::time::advance(Duration::from_secs(15)).await;
        }
        assert!(matches!(
            pending.await.unwrap(),
            Err(AgentReadError::Unavailable(_))
        ));
        assert_eq!(count.snapshots.load(Ordering::SeqCst), 3);
    }
}

#[tokio::test]
async fn purge_and_consistent_owner_transfer_after_binding_refuse_every_result() {
    for route in [SNAPSHOT, HEAD] {
        for purge in [false, true] {
            for fail in [false, true] {
                let f = fixture(true, false).await;
                let (reads, _, count) = counted(&f).await;
                count.snapshots.store(0, Ordering::SeqCst);
                let (mut state, gate, resume) = gated(&f, fail);
                state.reads = reads;
                let app = router(state);
                let pending = tokio::spawn(async move {
                    raw(&app, route, "owner-a", "cma-message-bindings").await
                });
                entered(&gate).await;
                assert_eq!(count.snapshots.load(Ordering::SeqCst), 2);
                if purge {
                    count.purged.store(true, Ordering::SeqCst);
                } else {
                    count.transferred.store(true, Ordering::SeqCst);
                }
                resume.notify_one();
                let (status, bytes) = pending.await.unwrap();
                assert_eq!(
                    status,
                    StatusCode::NOT_FOUND,
                    "{route} purge={purge} fail={fail}"
                );
                assert_eq!(count.snapshots.load(Ordering::SeqCst), 3);
                let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(body["code"], "agent_not_found");
                assert!(body.get("items").is_none());
                assert!(body.get("active_turn").is_none());
                for private in [
                    "Private source input",
                    "private-source-reference",
                    "changed-owner",
                    "cmd_control_source",
                ] {
                    assert!(!String::from_utf8_lossy(&bytes).contains(private));
                }
            }
        }
    }
}
