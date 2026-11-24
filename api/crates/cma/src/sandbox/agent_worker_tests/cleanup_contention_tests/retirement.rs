use super::*;
use crate::store::MongoOperationStore;
use axum::{body::Body, http::Request, http::StatusCode, Router};
use layer2_core::sandbox::{SandboxEventKind, SandboxWorkStep};
use layer2_core::{Authorization, Capability};
use mongodb::{
    bson::Document,
    event::{command::CommandEvent, EventHandler},
    Database,
};
use tower::ServiceExt;

mod outcomes;
mod retry_guards;

const LEASE: Duration = Duration::from_secs(18 * 60);

type PublicationBarrier = (
    tokio::sync::oneshot::Sender<()>,
    std::sync::mpsc::Receiver<()>,
);

struct Fixture {
    h: Harness<MongoSandboxStore>,
    admin: Database,
    app_name: String,
    routes: Router,
    agent: Agent,
    commit_session: Arc<Mutex<Option<Document>>>,
    publication_barrier: Arc<Mutex<Option<PublicationBarrier>>>,
}

impl Fixture {
    async fn new(segmented: bool) -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("cma_cleanup_retired_{suffix}");
        let app_name = format!("cleanup-retired-{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap().database(&name);
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        assert!(options.hosts.iter().all(|address| matches!(address,
            mongodb::options::ServerAddress::Tcp { host, .. }
                if matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1")
        )));
        options.app_name = Some(app_name.clone());
        let commit_session = Arc::new(Mutex::new(None));
        let observed = commit_session.clone();
        let publication_barrier = Arc::new(Mutex::new(None::<PublicationBarrier>));
        let barrier = publication_barrier.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            let CommandEvent::Started(event) = event else {
                return;
            };
            if event.command_name == "commitTransaction" {
                *observed.lock().unwrap() =
                    Some(event.command.get_document("lsid").unwrap().clone());
            }
            if matches!(event.command.get_bool("startTransaction"), Ok(true)) {
                let pending = barrier.lock().unwrap().take();
                if let Some((entered, release)) = pending {
                    tokio::task::block_in_place(|| {
                        entered.send(()).unwrap();
                        release.recv_timeout(Duration::from_secs(15)).unwrap();
                    });
                }
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let h = Harness::with_store(store.clone(), Arc::new(EmptyAgentSkillResolver), 1).await;
        let admitted = h.admit("retirement-cleanup", vec![]).await;
        let ready = h.drive_ready(&admitted.id).await;
        store
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&ready),
                reason: AgentStopReason::Owner,
                requested_at: rfc3339_at(SystemTime::now()),
            })
            .await
            .unwrap();
        for _ in 0..3 {
            assert!(h
                .worker
                .run_one(&ready.id, SystemTime::now())
                .await
                .unwrap());
        }
        let agent = h.stored(&ready.id).await;
        assert_eq!(agent.status, AgentStatus::Stopped);
        if segmented {
            store.install_segmented_control_fixture(&agent).await;
        }
        let operations = MongoOperationStore::open_for_test(admin.client(), &name)
            .await
            .unwrap();
        let routes = crate::projections::AgentProjectionServices::new(
            layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
            store,
            Arc::new(layer2_core::profiles::store::InMemoryProfileStore::default()),
            h.registry.clone(),
            Arc::new(crate::projections::UnavailableSandboxResources),
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        )
        .router(Arc::new(operations));
        Self {
            h,
            admin,
            app_name,
            routes,
            agent,
            commit_session,
            publication_barrier,
        }
    }

    async fn delete(&self, key: &str) {
        let mut request = Request::delete(format!("/api/v1/agents/{}", self.agent.id))
            .header("Idempotency-Key", key)
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(layer1_public::AuthenticatedSubject(
                self.agent.owner.clone(),
            ));
        request
            .extensions_mut()
            .insert(Authorization::new([Capability::UseOwnSandboxes]));
        assert_eq!(
            self.routes.clone().oneshot(request).await.unwrap().status(),
            StatusCode::ACCEPTED
        );
    }

    async fn claim(&self, store: &MongoSandboxStore, holder: &str, now: SystemTime) -> Agent {
        store
            .claim_agent_work(
                &self.agent.id,
                holder,
                rfc3339_at(now),
                rfc3339_at(now + LEASE),
            )
            .await
            .unwrap()
            .unwrap()
    }

    async fn begin_retirement(&self, now: SystemTime) -> Sandbox {
        let mut parent = self
            .h
            .store
            .get_sandbox(&self.agent.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let revision = parent.revision;
        parent.begin_retirement(rfc3339_at(now)).unwrap();
        self.h
            .store
            .save_sandbox(revision, parent.clone())
            .await
            .unwrap();
        parent
    }

    async fn retire(&self, mut parent: Sandbox, now: SystemTime, remove_runtime: bool) {
        if remove_runtime {
            self.h.adapter.teardown(&self.h.runtime).await.unwrap();
        }
        let revision = parent.revision;
        parent.runtime = None;
        parent.credential.current = None;
        parent.credential.delivered = false;
        let at = rfc3339_at(now);
        parent
            .transition(SandboxStatus::Retired, at.clone(), None)
            .unwrap();
        parent.work.schedule(SandboxWorkStep::WaitForPurge, at);
        self.h.store.save_sandbox(revision, parent).await.unwrap();
    }

    async fn fail(&self, times: i32, mut data: Document) {
        data.insert("appName", &self.app_name);
        self.admin
            .client()
            .database("admin")
            .run_command(
                doc! {"configureFailPoint":"failCommand","mode":{"times":times},"data":data},
            )
            .await
            .unwrap();
    }

    async fn clear_fault(&self) {
        self.admin
            .client()
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
    }

    async fn stored(&self) -> Agent {
        self.h.stored(&self.agent.id).await
    }

    async fn logs(&self) -> Vec<layer2_core::sandbox::AgentLogEntry> {
        self.h
            .store
            .agent_log(&self.agent.id, 0, 100)
            .await
            .unwrap()
    }

    async fn assert_terminal(&self, worker: &AgentWorker, now: SystemTime) {
        assert!(!worker.run_one(&self.agent.id, now).await.unwrap());
        let deleted = self.stored().await;
        assert_eq!(deleted.status, AgentStatus::Deleted);
        assert_eq!(deleted.work.receipts, self.agent.work.receipts);
        assert_eq!(deleted.worktree_id, self.agent.worktree_id);
        let worktree = self
            .h
            .store
            .get_worktree(&deleted.worktree_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(worktree.status, WorktreeStatus::Removed);
        let events = self
            .h
            .store
            .sandbox_events(&deleted.sandbox_id, 0, 100)
            .await
            .unwrap();
        assert_eq!(events.iter().filter(|event| matches!(
            &event.kind,
            SandboxEventKind::WorktreeRemoved { worktree_id, .. } if worktree_id == &deleted.worktree_id
        )).count(), 1);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    &event.kind,
                    SandboxEventKind::AgentDeleted { agent_id } if agent_id == &deleted.id
                ))
                .count(),
            1
        );
        let tail = self
            .h
            .store
            .agent_log_entry(&deleted.id, deleted.conversation.last_seq)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(tail.kind, AgentLogKind::Deleted));
    }

    async fn converge(&self, worker: &AgentWorker, now: SystemTime) {
        for _ in 0..4 {
            let current = self.stored().await;
            if current.status == AgentStatus::Deleted {
                break;
            }
            assert!(worker
                .run_one(&self.agent.id, now.max(ledger_hold_tests::due(&current)))
                .await
                .unwrap());
        }
        self.assert_terminal(worker, now).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable loopback replica set"]
async fn retired_cleanup_requeues_a_known_read_failure_under_both_log_formats() {
    for segmented in [false, true] {
        let fixture = Fixture::new(segmented).await;
        let prefix = fixture.logs().await;
        fixture.delete("original-delete").await;
        let now = SystemTime::now().max(ledger_hold_tests::due(&fixture.stored().await));
        let claim = fixture.claim(&fixture.h.store, "agent-worker", now).await;
        assert_eq!(claim.work.step, AgentWorkStep::EnsureProcessClosed);
        fixture.delete("duplicate-before-retirement").await;
        assert_eq!(fixture.stored().await, claim);
        let parent = fixture.begin_retirement(now).await;
        fixture.retire(parent, now, false).await;
        fixture.delete("duplicate-after-retirement").await;
        assert_eq!(fixture.stored().await, claim);
        assert!(!fixture
            .h
            .worker
            .due_work(now, 100)
            .await
            .unwrap()
            .contains(&claim.id));

        let requests = fixture.h.backend.requests().await;
        // The first command of executing this already-claimed step reads the
        // parent. Mongo's nonretryable read error occurs before any effect.
        fixture
            .fail(1, doc! {"failCommands":["find"],"errorCode":2_i32})
            .await;
        let result = fixture
            .h
            .worker
            .execute_claimed_for_test(claim.clone(), now)
            .await;
        fixture.clear_fault().await;
        result.unwrap();
        let retry = fixture.stored().await;
        ledger_hold_tests::preserved(&claim, &retry);
        let due = ledger_hold_tests::due(&retry);
        assert!(due < now + LEASE);
        assert!(fixture
            .h
            .worker
            .due_work(due, 100)
            .await
            .unwrap()
            .contains(&claim.id));
        assert!(matches!(&fixture.logs().await[prefix.len()].kind,
            AgentLogKind::WorkStepFailed { failure, .. }
                if failure.class == "agent_cleanup_preflight_unavailable"
        ));
        assert_eq!(fixture.h.backend.requests().await, requests);

        // The original runtime still exists. A fresh inventory must refuse
        // cleanup even though the Sandbox's durable binding has been cleared.
        assert!(fixture.h.worker.run_one(&claim.id, due).await.unwrap());
        let held = fixture.stored().await;
        assert_eq!(held.work.step, AgentWorkStep::EnsureProcessClosed);
        assert!(matches!(&fixture.logs().await.last().unwrap().kind,
            AgentLogKind::WorkStepFailed { failure, .. } if failure.class == "retired_runtime_present"
        ));
        fixture
            .h
            .adapter
            .teardown(&fixture.h.runtime)
            .await
            .unwrap();
        fixture.h.backend.set_inventory_unavailable(true);
        assert!(fixture
            .h
            .worker
            .run_one(&claim.id, ledger_hold_tests::due(&held))
            .await
            .unwrap());
        assert_eq!(
            fixture.stored().await.work.step,
            AgentWorkStep::EnsureProcessClosed
        );
        assert!(matches!(&fixture.logs().await.last().unwrap().kind,
            AgentLogKind::WorkStepFailed { failure, .. } if failure.class == "agent_cleanup_unavailable"
        ));
        fixture.h.backend.set_inventory_unavailable(false);
        let requests = fixture.h.backend.requests().await;
        fixture.converge(&fixture.h.worker, due).await;
        assert_eq!(fixture.h.backend.requests().await, requests);
        assert!(fixture.logs().await.starts_with(&prefix));
        fixture.admin.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable loopback replica set"]
async fn cleanup_claim_requeues_without_advancing_until_archive_clearance() {
    use layer2_core::sandbox::retirement_archive::{
        RetirementArchiveReceipt, RetirementArchiveRequirement,
    };

    let fixture = Fixture::new(false).await;
    fixture.delete("original-delete").await;
    let now = SystemTime::now().max(ledger_hold_tests::due(&fixture.stored().await));
    let claim = fixture.claim(&fixture.h.store, "agent-worker", now).await;
    let mut parent = fixture.begin_retirement(now).await;
    let mut gate = RetirementArchiveRequirement {
        format: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        plan_sha256: "a".repeat(64),
        source_sha256: "b".repeat(64),
        exclusion_sha256: "c".repeat(64),
        verified: None,
    };
    // Model the two archive coordinator writes without performing export.
    for cleared in [false, true] {
        if cleared {
            gate.verified = Some(RetirementArchiveReceipt {
                manifest_sha256: "d".repeat(64),
                custody_sha256: "e".repeat(64),
                verification_sha256: "f".repeat(64),
            });
        }
        let previous = parent.revision as i64;
        parent.lifecycle.retirement_archive = Some(gate.clone());
        parent.revision += 1;
        parent.validate().unwrap();
        let result = fixture
            .admin
            .collection::<Document>("sandboxes")
            .update_one(
                doc! {"_id":parent.id.as_str(),"revision":previous},
                doc! {
                    "$set":{"record.lifecycle.retirement_archive":
                        mongodb::bson::serialize_to_document(&gate).unwrap()},
                    "$inc":{"revision":1_i64,"record.revision":1_i64},
                },
            )
            .await
            .unwrap();
        assert_eq!(result.modified_count, 1);
        if !cleared {
            let requests = fixture.h.backend.requests().await;
            fixture
                .h
                .worker
                .execute_claimed_for_test(claim.clone(), now)
                .await
                .unwrap();
            let deferred = fixture.stored().await;
            ledger_hold_tests::preserved(&claim, &deferred);
            assert!(matches!(&fixture.logs().await.last().unwrap().kind,
                AgentLogKind::WorkStepFailed { failure, .. } if failure.class == "agent_cleanup_archive_pending"
            ));
            assert!(!fixture
                .h
                .worker
                .run_one(&claim.id, ledger_hold_tests::due(&deferred))
                .await
                .unwrap());
            assert_eq!(fixture.h.backend.requests().await, requests);
        }
    }
    fixture.retire(parent, now, true).await;
    fixture.converge(&fixture.h.worker, now).await;
    fixture.admin.drop().await.unwrap();
}
