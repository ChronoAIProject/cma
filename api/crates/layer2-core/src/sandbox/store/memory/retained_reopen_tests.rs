use super::*;
use crate::sandbox::AgentStatus;
use crate::sandbox::{
    RetainedReopenError as E, RetainedReopenProof, RetainedReopenService, RetainedReopenSnapshot,
    RetainedReopenVerifier,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn fixture() -> (Arc<InMemorySandboxStore>, crate::Caller, SandboxId) {
    let request = crate::sandbox::retained_reopen_tests::fixture();
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = request.snapshot.retained.sandbox;
    let caller = crate::Caller {
        subject: parent.owner.clone(),
        authorization: crate::Authorization::new([crate::Capability::UseOwnSandboxes]),
    };
    let id = parent.id.clone();
    {
        let mut state = store.lock();
        state.sandboxes.insert(id.clone(), parent);
        for mut agent in request.snapshot.retained.agents {
            let logs = [
                AgentLogKind::Created {
                    workspace_id: agent.workspace_id.clone(),
                    worktree_id: agent.worktree_id.clone(),
                },
                AgentLogKind::ConversationOpened {
                    conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                    runtime_generation: agent.conversation.runtime_generation.unwrap(),
                },
                AgentLogKind::StatusChanged {
                    from: AgentStatus::Ready,
                    to: AgentStatus::Paused,
                },
            ];
            agent.conversation.last_seq = logs.len() as u64;
            state.agent_logs.insert(
                agent.id.clone(),
                logs.into_iter()
                    .enumerate()
                    .map(|(i, kind)| AgentLogEntry {
                        agent_id: agent.id.clone(),
                        seq: i as u64 + 1,
                        appended_at: agent.updated_at.clone(),
                        kind,
                    })
                    .collect(),
            );
            state.agents.insert(agent.id.clone(), agent);
        }
        for workspace in request.snapshot.retained.workspaces {
            state.workspaces.insert(workspace.id.clone(), workspace);
        }
        for tree in request.snapshot.worktrees {
            state.worktrees.insert(tree.id.clone(), tree);
        }
    }
    (store, caller, id)
}

struct Verifier {
    store: Arc<InMemorySandboxStore>,
    calls: AtomicUsize,
    race: u8,
}

#[async_trait]
impl RetainedReopenVerifier for Verifier {
    async fn verify(
        &self,
        snapshot: &RetainedReopenSnapshot,
    ) -> Result<RetainedReopenProof, SandboxStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.race == 6 {
            std::future::pending::<()>().await;
        }
        if self.race != 0 {
            let mut state = self.store.lock();
            match self.race {
                1 => {
                    state
                        .sandboxes
                        .get_mut(&snapshot.retained.sandbox.id)
                        .unwrap()
                        .revision += 1
                }
                2 => {
                    state
                        .agents
                        .get_mut(&snapshot.retained.agents[0].id)
                        .unwrap()
                        .revision += 1
                }
                3 => {
                    state
                        .workspaces
                        .get_mut(&snapshot.retained.workspaces[0].id)
                        .unwrap()
                        .revision += 1
                }
                4 => {
                    state
                        .worktrees
                        .get_mut(&snapshot.worktrees[0].id)
                        .unwrap()
                        .revision += 1
                }
                5 => {
                    state
                        .sandboxes
                        .get_mut(&snapshot.retained.sandbox.id)
                        .unwrap()
                        .owner = crate::SubjectId("new-owner".into())
                }
                _ => unreachable!(),
            }
        }
        Ok(proof(snapshot))
    }
}

fn proof(snapshot: &RetainedReopenSnapshot) -> RetainedReopenProof {
    let mut proof = crate::sandbox::retained_reopen_tests::fixture().proof;
    let source = snapshot.retained.source().unwrap();
    proof.expected_runtime = source.binding.clone();
    proof.home_identity = source.home_identity;
    proof
}

#[tokio::test]
async fn idle_reopen_refuses_a_live_child_before_observing_a_replacement_boot() {
    for live_child in [false, true] {
        let (store, caller, id) = fixture();
        let parent = {
            let mut state = store.lock();
            let parent = state.sandboxes.get_mut(&id).unwrap();
            parent.failure.as_mut().unwrap().class = "runtime_lost".into();
            let parent = parent.clone();
            if live_child {
                let agent = state.agents.values_mut().next().unwrap();
                agent.status = AgentStatus::Ready;
                agent.conversation.last_seq -= 1;
                let agent_id = agent.id.clone();
                state.agent_logs.get_mut(&agent_id).unwrap().pop().unwrap();
            }
            parent
        };
        let (service, verifier) = service(&store, true, 0);
        let request = service.request(&caller, &id, parent.revision, "same-image", true);
        if live_child {
            assert!(matches!(request.await, Err(E::Refused)));
            assert_eq!(store.get_sandbox(&id).await.unwrap(), Some(parent));
            assert!(store.retained_reopen_receipt(&id).await.unwrap().is_none());
            assert_eq!(verifier.calls.load(Ordering::SeqCst), 0);
        } else {
            request.await.unwrap();
            let accepted = store.retained_reopen_receipt(&id).await.unwrap().unwrap();
            let expected = parent.runtime.unwrap();
            assert_eq!(accepted.proof.expected_runtime, expected);
            assert_ne!(accepted.proof.observed_boot, expected.boot_id.unwrap());
            assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
        }
    }
}

fn service(
    store: &Arc<InMemorySandboxStore>,
    enabled: bool,
    race: u8,
) -> (RetainedReopenService, Arc<Verifier>) {
    let verifier = Arc::new(Verifier {
        store: store.clone(),
        calls: AtomicUsize::new(0),
        race,
    });
    (
        RetainedReopenService::new(store.clone(), verifier.clone(), enabled),
        verifier,
    )
}

#[tokio::test]
async fn consent_and_owner_checks_precede_proof_and_default_off_admits_nothing() {
    let (store, caller, id) = fixture();
    let (service, verifier) = service(&store, false, 0);
    let before = store.retained_reopen_snapshot(&id).await.unwrap();
    assert!(matches!(
        service.request(&caller, &id, 7, "key", true).await,
        Err(E::Unavailable)
    ));
    assert!(matches!(
        service.request(&caller, &id, 7, "key", false).await,
        Err(E::InvalidConsent)
    ));
    let foreign = crate::Caller {
        subject: crate::SubjectId("another-owner".into()),
        ..caller.clone()
    };
    assert!(matches!(
        service.request(&foreign, &id, 7, "key", true).await,
        Err(E::NotFound)
    ));
    let operator = crate::Caller {
        authorization: crate::Authorization::new([crate::Capability::ControlAnySandbox]),
        ..caller
    };
    assert!(matches!(
        service.request(&operator, &id, 7, "key", true).await,
        Err(E::Forbidden)
    ));
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 0);
    assert_eq!(store.retained_reopen_snapshot(&id).await.unwrap(), before);
    assert!(store.lock().retained_reopen_receipts.is_empty());
}

#[tokio::test]
async fn exactly_one_admission_and_original_receipt_survive_progress_and_disabling() {
    let (store, caller, id) = fixture();
    let (enabled, _) = service(&store, true, 0);
    let before = store.retained_reopen_snapshot(&id).await.unwrap();
    before.validate().expect("fixture source must qualify");
    let mut checked = crate::sandbox::retained_reopen_tests::fixture();
    checked.snapshot = before.clone();
    checked.proof = proof(&before);
    checked.requested_at = crate::now_rfc3339();
    crate::sandbox::apply_retained_reopen(&before, &checked)
        .expect("fixture transition must qualify");
    let accepted = enabled
        .request(&caller, &id, 7, "same-key", true)
        .await
        .unwrap();
    assert_eq!(accepted.revision, 8);
    assert_eq!(store.lock().sandbox_events[&id].len(), 1);
    assert_eq!(
        store.lock().agents.values().next(),
        before.retained.agents.first()
    );
    assert_eq!(store.lock().agent_logs.values().next().unwrap().len(), 3);
    {
        let mut state = store.lock();
        let parent = state.sandboxes.get_mut(&id).unwrap();
        parent.revision += 1;
    }
    let (disabled, verifier) = service(&store, false, 0);
    assert_eq!(
        disabled
            .request(&caller, &id, 7, "same-key", true)
            .await
            .unwrap(),
        accepted
    );
    assert!(matches!(
        disabled
            .request(&caller, &id, 7, "different-key", true)
            .await,
        Err(E::StaleRequest)
    ));
    assert!(matches!(
        disabled.request(&caller, &id, 9, "same-key", true).await,
        Err(E::Conflict)
    ));
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 0);
    assert_eq!(store.lock().sandbox_events[&id].len(), 1);
}

#[tokio::test]
async fn provider_observation_never_overrides_new_parent_child_or_worktree_state() {
    for race in 1..=5 {
        let (store, caller, id) = fixture();
        let (service, _) = service(&store, true, race);
        assert!(
            service.request(&caller, &id, 7, "key", true).await.is_err(),
            "race {race}"
        );
        assert!(store.lock().retained_reopen_receipts.is_empty());
        assert!(store.lock().sandbox_events.is_empty());
    }
}

#[tokio::test]
async fn two_independent_instances_replay_the_same_accepted_operation() {
    let (store, caller, id) = fixture();
    let (first, _) = service(&store, true, 0);
    let (second, _) = service(&store, true, 0);
    let (a, b) = tokio::join!(
        first.request(&caller, &id, 7, "key", true),
        second.request(&caller, &id, 7, "key", true)
    );
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(store.lock().retained_reopen_receipts.len(), 1);
    assert_eq!(store.lock().sandbox_events[&id].len(), 1);
}

#[tokio::test(start_paused = true)]
async fn cancelled_and_timed_out_proof_never_admits_and_releases_the_permit() {
    let (store, caller, id) = fixture();
    let before = store.retained_reopen_snapshot(&id).await.unwrap();
    let (service, verifier) = service(&store, true, 6);
    let service = Arc::new(service);
    let launch = || {
        let (service, caller, id) = (service.clone(), caller.clone(), id.clone());
        tokio::spawn(async move { service.request(&caller, &id, 7, "retained-key", true).await })
    };
    let first = launch();
    tokio::task::yield_now().await;
    assert!(matches!(
        service.request(&caller, &id, 7, "retained-key", true).await,
        Err(E::Unavailable)
    ));
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let second = launch();
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(26)).await;
    assert!(matches!(second.await.unwrap(), Err(E::Unavailable)));
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 2);
    let third = launch();
    tokio::task::yield_now().await;
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 3);
    third.abort();
    assert!(third.await.unwrap_err().is_cancelled());
    assert_eq!(store.retained_reopen_snapshot(&id).await.unwrap(), before);
    assert!(store.lock().retained_reopen_receipts.is_empty());
    assert!(store.lock().sandbox_events.is_empty());
}
