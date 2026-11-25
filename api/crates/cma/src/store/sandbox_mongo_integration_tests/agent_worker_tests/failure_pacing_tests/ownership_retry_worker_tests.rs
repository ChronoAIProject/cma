use super::ownership_retry_wake_tests::{bind_runtime, member};
use super::*;
use crate::sandbox::binding_lifecycle_test_support::{
    AgentWorker, AgentWorkerConfig, AgentWorkerDependencies, EmptyAgentSkillResolver,
};
use crate::sandbox::ownership_retry_hint::{
    AdvisoryDecision, CapturedOwnershipProbe, HintWriteResult, OwnershipRetryHints,
};
use crate::sandbox::LiveAgentRegistry;
use crate::store::agent_failure_pacing::ownership::FIELD as HINT;
use l3::codex::{CodexAdapter, CodexSandboxSpec};
use l3::hosted::{HostedControlTransport, InMemoryHostedBackend, ProviderHostedControlTransport};
use layer2_core::agent_ownership::{OwnershipCoordinator, OwnershipStore, OwnershipToken};
use layer2_core::sandbox::agent_io::{AgentCommandStore, CommandReservationResult};
use layer2_core::sandbox::{AgentOperationService, DurableAgentUpdateRecorder};
use layer3_adapter as l3;
#[path = "ownership_retry_runtime_fixture.rs"]
mod runtime_fixture;
use runtime_fixture::{Backend, Home, Runtime, HANDLE};
#[path = "failed_inspection_takeover_tests.rs"]
mod failed_inspection_takeover_tests;
#[path = "legacy_receiver_handoff_tests.rs"]
mod legacy_receiver_handoff;

fn build_worker(
    store: Arc<MongoSandboxStore>,
    manager: Arc<OwnershipCoordinator>,
    transport: Arc<ProviderHostedControlTransport>,
    home: l3::HomeVolumeAttachment,
    hints: Option<Arc<dyn OwnershipRetryHints>>,
) -> Arc<AgentWorker> {
    let registry = Arc::new(LiveAgentRegistry::default());
    AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: Arc::new(
                CodexAdapter::with_backend(
                    Backend(transport, InMemoryHostedBackend::default()),
                    CodexSandboxSpec::default(),
                )
                .with_home_volumes(Arc::new(Home(home))),
            ),
            skills: Arc::new(EmptyAgentSkillResolver),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store.clone(), registry.clone()),
            registry,
        },
        AgentWorkerConfig::new("retry-worker", "retry-fixture")
            .with_ownership(Some(manager))
            .with_ownership_retry_hints(hints.or_else(|| store.ownership_retry_hints())),
    )
}

struct CleanupFirst {
    hints: Arc<dyn OwnershipRetryHints>,
    manager: Arc<OwnershipCoordinator>,
    runtime: Arc<runtime_fixture::Counts>,
}

#[async_trait::async_trait]
impl OwnershipRetryHints for CleanupFirst {
    async fn preflight(&self, id: &AgentId) -> AdvisoryDecision {
        self.hints.preflight(id).await
    }
    async fn capture(
        &self,
        agent: &Agent,
        owner: &OwnershipToken,
    ) -> Option<Arc<dyn CapturedOwnershipProbe>> {
        Some(Arc::new(CleanupBeforeRecord {
            captured: self.hints.capture(agent, owner).await?,
            manager: self.manager.clone(),
            runtime: self.runtime.clone(),
        }))
    }
}

struct CleanupBeforeRecord {
    captured: Arc<dyn CapturedOwnershipProbe>,
    manager: Arc<OwnershipCoordinator>,
    runtime: Arc<runtime_fixture::Counts>,
}

#[async_trait::async_trait]
impl CapturedOwnershipProbe for CleanupBeforeRecord {
    async fn record(&self, failure: &l3::owner_protocol::FirstOwnershipFailure) -> HintWriteResult {
        let authority = self.manager.owned().await.pop().unwrap();
        assert!(authority.receiver_cancelled());
        self.runtime.cleanup.store(true, Ordering::SeqCst);
        self.manager
            .handoff(&authority, authority.revoke_transports())
            .await
            .unwrap();
        let result = self.captured.record(failure).await;
        assert_eq!(result, HintWriteResult::Stale);
        result
    }
}

struct UnavailableRecord {
    hints: Arc<dyn OwnershipRetryHints>,
    admin: Client,
    app_name: String,
    timeout: bool,
}

#[async_trait::async_trait]
impl OwnershipRetryHints for UnavailableRecord {
    async fn preflight(&self, id: &AgentId) -> AdvisoryDecision {
        self.hints.preflight(id).await
    }
    async fn capture(
        &self,
        agent: &Agent,
        owner: &OwnershipToken,
    ) -> Option<Arc<dyn CapturedOwnershipProbe>> {
        Some(Arc::new(UnavailableProbe {
            captured: self.hints.capture(agent, owner).await?,
            admin: self.admin.clone(),
            app_name: self.app_name.clone(),
            timeout: self.timeout,
        }))
    }
}

struct UnavailableProbe {
    captured: Arc<dyn CapturedOwnershipProbe>,
    admin: Client,
    app_name: String,
    timeout: bool,
}

#[async_trait::async_trait]
impl CapturedOwnershipProbe for UnavailableProbe {
    async fn record(&self, failure: &l3::owner_protocol::FirstOwnershipFailure) -> HintWriteResult {
        let data = if self.timeout {
            doc! {"failCommands":["update"], "appName":&self.app_name,
            "blockConnection":true, "blockTimeMS":2500_i32}
        } else {
            doc! {"failCommands":["commitTransaction"], "appName":&self.app_name,
            "writeConcernError":{"code":50_i32, "codeName":"MaxTimeMSExpired", "errmsg":"injected ambiguous commit"}}
        };
        self.admin.database("admin").run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":if self.timeout {1_i32} else {5_i32}}, "data":data,
        }).await.unwrap();
        let started = std::time::Instant::now();
        let result = self.captured.record(failure).await;
        let elapsed = started.elapsed();
        self.admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
            .await
            .unwrap();
        assert_eq!(result, HintWriteResult::Unavailable);
        assert!(
            elapsed < Duration::from_secs(3),
            "optional write exceeded its bounded budget: {elapsed:?}"
        );
        result
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn ownership_retry_hint_composed_transport_cancellation_and_restart_preserve_custody() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("isolated failpoint-enabled replica set");
    for reason in [
        "capability_missing",
        "challenge_protocol_mismatch",
        "challenge_lifetime_mismatch",
        "malformed",
        "auth",
        "confirm_ambiguous",
        "timeout",
        "cleanup_first",
        "commit_unknown",
        "store_timeout",
    ] {
        let name = format!("cma_retry_composed_{}", uuid::Uuid::new_v4().simple());
        let client = Client::with_uri_str(&uri).await.unwrap();
        let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(name.clone());
        let worker_client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&worker_client, &name)
            .await
            .unwrap();
        let agent = ready_conversation_fixture(&store, "composed").await;
        let peer = ready_conversation_fixture(&store, "healthy-peer").await;
        let workspace = store
            .get_workspace(&agent.workspace_id)
            .await
            .unwrap()
            .unwrap();
        let mut worktree = store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap();
        let revision = worktree.revision;
        worktree.path = format!("{}/.worktrees/{}", workspace.path, worktree.name);
        worktree.base_commit = workspace.resolved_commit.unwrap();
        worktree.revision += 1;
        store.save_worktree(revision, worktree).await.unwrap();
        let home = bind_runtime(&store, &agent, HANDLE).await;
        let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
        let reservation =
            crate::store::sandbox_mongo_integration_tests::command_ledger_tests::request(
                &agent,
                &parent,
                "retained-command",
            );
        let command_id = reservation.envelope.command_id.to_string();
        assert!(matches!(
            store.reserve_command(reservation).await.unwrap(),
            CommandReservationResult::Created(_)
        ));
        let command = store
            .agent_commands
            .find_one(doc! {"_id": &command_id})
            .await
            .unwrap()
            .unwrap();
        let store = Arc::new(
            store
                .enable_agent_ownership("manager".into())
                .await
                .unwrap()
                .with_ownership_retry_hints(true),
        );
        let manager = OwnershipCoordinator::register(
            store.agent_ownership.as_ref().unwrap().clone(),
            member("manager"),
        )
        .await
        .unwrap();
        let cleanup_first = reason == "cleanup_first";
        let runtime_reason =
            if matches!(reason, "cleanup_first" | "commit_unknown" | "store_timeout") {
                "capability_missing"
            } else {
                reason
            };
        let runtime = Runtime::start(runtime_reason).await;
        let mut hints = cleanup_first.then(|| {
            Arc::new(CleanupFirst {
                hints: store.ownership_retry_hints().unwrap(),
                manager: manager.clone(),
                runtime: runtime.counts.clone(),
            }) as Arc<dyn OwnershipRetryHints>
        });
        if matches!(reason, "commit_unknown" | "store_timeout") {
            hints = Some(Arc::new(UnavailableRecord {
                hints: store.ownership_retry_hints().unwrap(),
                admin: client.clone(),
                app_name: name.clone(),
                timeout: reason == "store_timeout",
            }) as Arc<dyn OwnershipRetryHints>);
        }
        let worker = build_worker(
            store.clone(),
            manager.clone(),
            runtime.transport.clone(),
            home.clone(),
            hints,
        );
        assert_eq!(
            worker.run_one(&agent.id, SystemTime::now()).await,
            Err(SandboxStoreError::Conflict)
        );
        runtime.assert_counts_with_confirmation(1, usize::from(reason == "confirm_ambiguous"));
        // An injected write-concern error returns before the applied hint is
        // majority-visible; settle it before reading the row.
        crate::store::test_visibility::majority_barrier(&client, &name).await;
        let row = store
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let held = !matches!(
            reason,
            "malformed"
                | "auth"
                | "confirm_ambiguous"
                | "timeout"
                | "cleanup_first"
                | "store_timeout"
        );
        assert_eq!(row.contains_key(HINT), held, "{reason}");
        if held {
            assert_eq!(
                row.get_document(HINT).unwrap().get_str("reason").unwrap(),
                runtime_reason
            );
        }
        let saved = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(
            store
                .agent_commands
                .find_one(doc! {"_id": &command_id})
                .await
                .unwrap(),
            Some(command)
        );
        assert_eq!(saved.conversation, agent.conversation);
        assert_eq!(saved.work.attempts, agent.work.attempts);
        assert_eq!(saved.work.receipts, agent.work.receipts);
        assert_eq!(saved.work.cleanup_debt, agent.work.cleanup_debt);
        assert!(store
            .agent_log(&agent.id, agent.conversation.last_seq, 10)
            .await
            .unwrap()
            .is_empty());
        if !cleanup_first {
            let authority = manager.owned().await.pop().unwrap();
            assert!(authority.receiver_cancelled());
            assert!(authority.revoke_transports().await.is_err());
            runtime.counts.cleanup.store(true, Ordering::SeqCst);
            manager
                .handoff(&authority, authority.revoke_transports())
                .await
                .unwrap();
        }
        // Receiver cancellation and reconnect pacing are per Agent. The same
        // manager can still claim independent work without another runtime probe.
        let peer_authority = manager.acquire(&peer.id).await.unwrap().unwrap();
        let now = SystemTime::now();
        let peer_claim = peer_authority
            .run(store.claim_agent_work(
                &peer.id,
                "healthy-worker",
                layer2_core::rfc3339_at(now),
                layer2_core::rfc3339_at(now + Duration::from_secs(600)),
            ))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(peer_claim.id, peer.id);
        manager
            .handoff(&peer_authority, peer_authority.revoke_transports())
            .await
            .unwrap();
        store
            .agent_ownership
            .as_ref()
            .unwrap()
            .heartbeat("manager", true)
            .await
            .unwrap();
        drop(worker);
        drop(manager);
        let independent = Client::with_uri_str(&uri).await.unwrap();
        let restarted = Arc::new(
            MongoSandboxStore::open(&independent, &name)
                .await
                .unwrap()
                .enable_agent_ownership("successor".into())
                .await
                .unwrap()
                .with_ownership_retry_hints(true),
        );
        assert_eq!(
            restarted.preflight(&agent.id).await,
            if held {
                AdvisoryDecision::Hold
            } else {
                AdvisoryDecision::Proceed
            }
        );
        if held {
            let successor = OwnershipCoordinator::register(
                restarted.agent_ownership.as_ref().unwrap().clone(),
                member("successor"),
            )
            .await
            .unwrap();
            let worker = build_worker(
                restarted.clone(),
                successor.clone(),
                runtime.transport.clone(),
                home.clone(),
                None,
            );
            assert!(!worker.run_one(&agent.id, SystemTime::now()).await.unwrap());
            assert!(!worker.run_one(&agent.id, SystemTime::now()).await.unwrap());
            runtime.assert_counts(1);
            let mut expired = (*restarted).clone();
            let until = *row
                .get_document(HINT)
                .unwrap()
                .get_datetime("until")
                .unwrap();
            expired.ownership_retry_hint_now = Some(until);
            let expired = Arc::new(expired);
            let worker = build_worker(
                expired.clone(),
                successor,
                runtime.transport.clone(),
                home.clone(),
                None,
            );
            let (first, duplicate) = tokio::join!(
                worker.run_one(&agent.id, SystemTime::now()),
                worker.run_one(&agent.id, SystemTime::now())
            );
            assert!(first.is_err() || duplicate.is_err());
            runtime.assert_counts(2);
            let next = expired
                .agents
                .find_one(doc! {"_id": agent.id.as_str()})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                next.get_document(HINT)
                    .unwrap()
                    .get_datetime("observed_at")
                    .unwrap(),
                &until
            );
            assert_ne!(
                next.get_document(HINT)
                    .unwrap()
                    .get_str("observation")
                    .unwrap(),
                row.get_document(HINT)
                    .unwrap()
                    .get_str("observation")
                    .unwrap()
            );
        }
        runtime
            .transport
            .retire(&l3::RuntimeHandle::new(HANDLE))
            .await
            .unwrap();
        client.database(&name).drop().await.unwrap();
    }
}
