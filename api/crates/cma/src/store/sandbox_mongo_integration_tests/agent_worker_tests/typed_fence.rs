//! The worker publication root must retain typed ownership failures. Native
//! effects precede this pure transaction; their durable receipts stay exact.
use super::*;
use crate::store::sandbox_mongo::TRANSACTION_ATTEMPTS;
use layer2_core::agent_ownership::{Authority, OwnershipStore};
use std::time::Duration;

mod fixture;
use fixture::{assert_attempts, Fixture};

struct Publication {
    fixture: Fixture,
    authority: Arc<Authority>,
    before: Agent,
    worktree: Worktree,
    mutation: AgentWorkMutation,
}

impl Publication {
    async fn new() -> Self {
        let mut fixture = Fixture::new().await;
        let (_, worktree, claimed) = claimed_agent_work_fixture(&fixture.store, "typed").await;
        let before = ready_publication_claim(claimed, "completed-native-open");
        replace_agent_fixture(&fixture.store, &before).await;
        let fence = before.work.lease.as_ref().unwrap().fence;
        let mut agent = before.clone();
        agent
            .work
            .advance(
                "worker",
                fence,
                AgentWorkStep::PublishConnection,
                NOW.into(),
            )
            .unwrap();
        agent
            .transition(AgentStatus::Ready, NOW.into(), None)
            .unwrap();
        agent.conversation.last_seq += 2;
        let mut next_worktree = worktree.clone();
        next_worktree.transition(WorktreeStatus::Ready).unwrap();
        let mutation = AgentWorkMutation {
            expected_agent_revision: before.revision,
            holder: "worker".into(),
            fence,
            agent,
            worktree: Some(VersionedWorktree {
                expected_revision: worktree.revision,
                worktree: next_worktree,
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: before.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            }),
            agent_logs: vec![
                AgentLogKind::ConversationOpened {
                    conversation_id: "completed-native-open".into(),
                    runtime_generation: 1,
                },
                AgentLogKind::StatusChanged {
                    from: AgentStatus::Starting,
                    to: AgentStatus::Ready,
                },
            ],
        };
        let authority = fixture.own(&before.id).await;
        Self {
            fixture,
            authority,
            before,
            worktree,
            mutation,
        }
    }

    async fn publish(&self) -> Result<(), SandboxStoreError> {
        tokio::time::timeout(
            Duration::from_secs(20),
            self.authority
                .run(self.fixture.store.commit_agent_work(self.mutation.clone())),
        )
        .await
        .expect("publication retry budget must remain bounded")
        .expect("local ownership token is still current")
    }

    async fn verify(&self, committed: bool, commands: &[Document], attempts: usize) {
        assert_attempts(commands, attempts);
        let agent = self
            .fixture
            .store
            .get_agent(&self.before.id)
            .await
            .unwrap()
            .unwrap();
        let worktree = self
            .fixture
            .store
            .get_worktree(&self.before.worktree_id)
            .await
            .unwrap()
            .unwrap();
        let logs = self
            .fixture
            .store
            .agent_log(&self.before.id, 0, 10)
            .await
            .unwrap();
        let events = self
            .fixture
            .store
            .sandbox_events(&self.before.sandbox_id, 0, 10)
            .await
            .unwrap();
        assert_eq!(
            agent.work.receipts, self.before.work.receipts,
            "native effect receipts cannot be replayed or replaced"
        );
        if committed {
            assert_eq!(agent, self.mutation.agent);
            assert_eq!(worktree, self.mutation.worktree.as_ref().unwrap().worktree);
            assert_eq!(
                logs.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
                [1, 2]
            );
            assert_eq!(
                logs.iter()
                    .map(|entry| entry.kind.clone())
                    .collect::<Vec<_>>(),
                self.mutation.agent_logs
            );
            assert_eq!(events.len(), 1);
            assert_eq!(Some(&events[0].kind), self.mutation.sandbox_event.as_ref());
        } else {
            assert_eq!(
                agent, self.before,
                "failed fence cannot publish Agent state"
            );
            assert_eq!(
                worktree, self.worktree,
                "failed fence cannot publish Worktree state"
            );
            assert!(
                logs.is_empty(),
                "failed fence cannot append conversation logs"
            );
            assert!(
                events.is_empty(),
                "failed fence cannot append sandbox events"
            );
        }
        let starts: Vec<_> = commands
            .iter()
            .filter(|c| c.get_bool("startTransaction").is_ok_and(|value| value))
            .collect();
        assert!(
            starts
                .iter()
                .all(|c| c.get_str("find").is_ok_and(|value| value == "agents")),
            "every retry must freshly read the Agent before fencing"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn worker_publication_fence_known_abort_retries_fresh_snapshot() {
    let p = Publication::new().await;
    p.fixture
        .arm(1, &["TransientTransactionError"], false)
        .await;
    let result = p.publish().await;
    let (hits, commands) = p.fixture.clear().await;
    // Read back before dropping, but clean up even when the old adapter fails.
    let agent = p.fixture.store.get_agent(&p.before.id).await.unwrap();
    if result.is_ok() {
        p.verify(true, &commands, 2).await;
    }
    p.fixture.cleanup().await;
    assert_eq!(hits, 1);
    result.expect("M3_FENCE_KNOWN_ABORT: a known-aborted fence must retry publication");
    assert_eq!(agent, Some(p.mutation.agent));
}

async fn refused(labels: &[&str], failures: usize, attempts: usize) -> SandboxStoreError {
    let p = Publication::new().await;
    p.fixture.arm(failures, labels, false).await;
    let result = p.publish().await;
    let (hits, commands) = p.fixture.clear().await;
    p.verify(false, &commands, attempts).await;
    p.fixture.cleanup().await;
    assert_eq!(hits, attempts);
    result.expect_err("a refused ownership fence cannot succeed")
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn worker_publication_fence_exhaustion_is_bounded_without_partial_effects() {
    let error = refused(
        &["TransientTransactionError"],
        TRANSACTION_ATTEMPTS,
        TRANSACTION_ATTEMPTS,
    )
    .await;
    assert_eq!(error, SandboxStoreError::Conflict);
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn worker_publication_fence_unknown_label_never_retries() {
    let error = refused(&["UnknownTransactionCommitResult"], 1, 1).await;
    assert!(matches!(error, SandboxStoreError::Unavailable(_)));
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn worker_publication_fence_unknown_dominates_both_labels() {
    let error = refused(
        &[
            "TransientTransactionError",
            "UnknownTransactionCommitResult",
        ],
        1,
        1,
    )
    .await;
    assert!(matches!(error, SandboxStoreError::Unavailable(_)));
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn worker_publication_fence_stale_epoch_after_abort_never_publishes() {
    let p = Publication::new().await;
    p.fixture.arm(1, &["TransientTransactionError"], true).await;
    let (result, ()) = tokio::join!(p.publish(), p.fixture.takeover(&p.authority));
    let (hits, commands) = p.fixture.clear().await;
    let owner = p.fixture.owners.owner(&p.before.id).await.unwrap().unwrap();
    p.verify(false, &commands, 2).await;
    p.fixture.cleanup().await;
    assert_eq!(hits, 1);
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert!(owner.token.epoch > p.authority.token.epoch);
    assert_eq!(owner.token.instance_id, "m3-successor");
}
