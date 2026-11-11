//! Exercise the real assessment GET with Mongo and admission explicitly disabled.
use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use futures_util::TryStreamExt;
use layer1_public::rest::{hierarchy_router, HierarchyState};
use layer2_core::{profiles::store::InMemoryProfileStore, Authorization, Capability};
use mongodb::{
    bson::RawDocumentBuf,
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tower::ServiceExt;

// Reuse complete creation/publication history and the real retained hierarchy.
#[path = "../retained_reopen/mongo_fixtures.rs"]
mod fixtures;

struct Inspector {
    hold: bool,
    unavailable: bool,
    reached: tokio::sync::Notify,
}
impl Inspector {
    fn new(hold: bool, unavailable: bool) -> Arc<Self> {
        Arc::new(Self {
            hold,
            unavailable,
            reached: Default::default(),
        })
    }
}
#[async_trait::async_trait]
impl RetainedRecoveryInspector for Inspector {
    async fn inspect(&self, _: &RetainedRecoverySnapshot) -> RetainedRecoveryDisposition {
        self.reached.notify_one();
        if self.hold {
            std::future::pending::<()>().await;
        }
        if self.unavailable {
            RetainedRecoveryDisposition::EvidenceUnavailable
        } else {
            RetainedRecoveryDisposition::RecordedBootPresent
        }
    }
}
struct NeverReopen;
#[async_trait::async_trait]
impl RetainedReopenVerifier for NeverReopen {
    async fn verify(
        &self,
        _: &RetainedReopenSnapshot,
    ) -> Result<RetainedReopenProof, SandboxStoreError> {
        panic!("GET must never invoke admission proof")
    }
}

struct Fixture {
    client: Client,
    name: String,
    store: Arc<MongoSandboxStore>,
    request: RetainedReopenRequest,
    commands: Arc<Mutex<Vec<String>>>,
}
impl Fixture {
    async fn new() -> Self {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let captured = commands.clone();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                captured.lock().unwrap().push(event.command_name);
            }
        }));
        let client = Client::with_options(options).unwrap();
        let name = format!("cma_retained_get_{}", uuid::Uuid::new_v4().simple());
        let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let request = fixtures::reopen_fixture(&store, "sbx_get_retained").await;
        Self {
            client,
            name,
            store,
            request,
            commands,
        }
    }

    async fn router(&self, inspector: Arc<Inspector>) -> Router {
        let profiles = Arc::new(InMemoryProfileStore::default());
        let operations = Arc::new(
            crate::store::operation::MongoOperationStore::open(&self.client, &self.name)
                .await
                .unwrap(),
        );
        hierarchy_router(
            HierarchyState::new(
                SandboxService::new(
                    self.store.clone(),
                    profiles.clone(),
                    SandboxPolicy::default(),
                    Arc::new(FixedSandboxCredentialPrerequisite::Ready),
                ),
                WorkspaceService::new(self.store.clone(), profiles),
                HierarchyReadService::new(self.store.clone()),
                operations,
            )
            .with_retained_recovery(Arc::new(RetainedRecoveryService::new(
                self.store.clone(),
                inspector,
            )))
            .with_retained_reopening(Arc::new(RetainedReopenService::new(
                self.store.clone(),
                Arc::new(NeverReopen),
                false,
            ))),
        )
    }

    fn get(&self) -> Request<Body> {
        let parent = &self.request.snapshot.retained.sandbox;
        let mut request = Request::builder()
            .uri(format!("/api/v1/sandboxes/{}/runtime-recovery", parent.id))
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(layer1_public::AuthenticatedSubject(parent.owner.clone()));
        request
            .extensions_mut()
            .insert(Authorization::new([Capability::UseOwnSandboxes]));
        request
    }

    async fn documents(&self) -> BTreeMap<String, Vec<Vec<u8>>> {
        let db = self.client.database(&self.name);
        let mut snapshot = BTreeMap::new();
        for name in db.list_collection_names().await.unwrap() {
            let rows: Vec<RawDocumentBuf> = db
                .collection(&name)
                .find(doc! {})
                .sort(doc! {"_id": 1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            snapshot.insert(
                name,
                rows.into_iter()
                    .map(|row| row.as_bytes().to_vec())
                    .collect(),
            );
        }
        snapshot
    }

    fn assert_read_commands(&self) {
        let commands = self.commands.lock().unwrap();
        assert!(commands.iter().any(|name| name == "find"));
        assert!(
            commands.iter().all(|name| matches!(
                name.as_str(),
                "find" | "getMore" | "abortTransaction" | "killCursors"
            )),
            "GET issued a non-read command: {commands:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_recovery_get_mongo_good_and_corrupt_evidence_never_writes_when_disabled() {
    for case in [
        "good",
        "prepared",
        "unsupported_metadata",
        "native_unavailable",
        "identity",
        "lineage",
        "gap",
        "metadata",
        "tail",
        "extra",
        "legacy_zero",
        "quarantined",
    ] {
        let mut f = Fixture::new().await;
        let agent = &f.request.snapshot.retained.agents[0];
        if matches!(case, "prepared" | "tail") {
            f.store = Arc::new(
                (*f.store)
                    .clone()
                    .prepare_bounded_admission(std::slice::from_ref(&agent.id))
                    .await
                    .unwrap(),
            );
        }
        let log_filter = doc! {"agent_id": agent.id.as_str(), "seq": 1_i64};
        match case {
            "identity" => {
                f.store
                    .agent_log
                    .update_one(
                        log_filter,
                        doc! {"$set": {"record.agent_id": "agt_foreign"}},
                    )
                    .await
                    .unwrap();
            }
            "lineage" => {
                let entry = AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq: 1,
                    appended_at: NOW.into(),
                    kind: AgentLogKind::Created {
                        workspace_id: agent.workspace_id.clone(),
                        worktree_id: layer2_core::WorktreeId("wkt_foreign".into()),
                    },
                };
                f.store
                    .agent_log
                    .replace_one(log_filter, log_document(&entry).unwrap())
                    .await
                    .unwrap();
            }
            "gap" => {
                f.store.agent_log.delete_one(log_filter).await.unwrap();
            }
            "metadata" | "unsupported_metadata" => {
                // Unknown versions fall back to complete validation; a known
                // envelope missing its required fields is corrupt evidence.
                let format = if case == "metadata" { 1_i64 } else { 99_i64 };
                f.store
                    .agents
                    .update_one(
                        doc! {"_id": agent.id.as_str()},
                        doc! {"$set": {"agent_log_integrity_v1": {"format": format}}},
                    )
                    .await
                    .unwrap();
            }
            "tail" => {
                f.store
                    .agent_log
                    .update_one(
                        doc! {"agent_id": agent.id.as_str(), "seq": 3_i64},
                        doc! {"$set": {"appended_at": "2026-09-05T00:31:00Z",
                        "record.appended_at": "2026-09-05T00:31:00Z"}},
                    )
                    .await
                    .unwrap();
            }
            "extra" => {
                let entry = AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq: 4,
                    appended_at: NOW.into(),
                    kind: AgentLogKind::StatusChanged {
                        from: AgentStatus::Ready,
                        to: AgentStatus::Paused,
                    },
                };
                f.store
                    .agent_log
                    .insert_one(log_document(&entry).unwrap())
                    .await
                    .unwrap();
            }
            "legacy_zero" => {
                f.store
                    .agents
                    .update_one(
                        doc! {"_id": agent.id.as_str()},
                        doc! {"$set": {"last_seq": 0_i64, "record.conversation.last_seq": 0_i64}},
                    )
                    .await
                    .unwrap();
            }
            "quarantined" => {
                f.store.agents.update_one(doc! {"_id": agent.id.as_str()},
                    doc! {"$set": {"agent_log_integrity_quarantine_v1": {
                        "format": 1_i64, "reason": "integrity_validation_failed", "observed_head": 3_i64,
                    }}}).await.unwrap();
            }
            _ => {}
        }
        let router = f
            .router(Inspector::new(false, case == "native_unavailable"))
            .await;
        let before = f.documents().await;
        f.commands.lock().unwrap().clear();
        let response = router.oneshot(f.get()).await.unwrap();
        let status = response.status();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        f.assert_read_commands();
        assert_eq!(f.documents().await, before, "GET changed documents: {case}");
        if matches!(
            case,
            "good" | "prepared" | "unsupported_metadata" | "native_unavailable"
        ) {
            assert_eq!(status, StatusCode::OK, "{case}: {body}");
            assert_eq!(body["admission_enabled"], false);
            assert_eq!(
                body["disposition"],
                if case == "native_unavailable" {
                    "evidence_unavailable"
                } else {
                    "recorded_boot_present"
                }
            );
        } else {
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{case}: {body}");
            assert_eq!(body["code"], "hierarchy_service_unavailable");
        }
        f.client.database(&f.name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_recovery_get_mongo_cancelled_native_assessment_leaves_every_document_unchanged() {
    let f = Fixture::new().await;
    let inspector = Inspector::new(true, false);
    let router = f.router(inspector.clone()).await;
    let before = f.documents().await;
    f.commands.lock().unwrap().clear();
    {
        let request = router.oneshot(f.get());
        tokio::pin!(request);
        tokio::select! {
            _ = inspector.reached.notified() => {}
            result = &mut request => panic!("assessment ended before cancellation: {result:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => panic!("native inspection not reached"),
        }
        // Drop the HTTP future after the real Mongo evidence load, before consent.
    }
    f.assert_read_commands();
    assert_eq!(f.documents().await, before);
    f.client.database(&f.name).drop().await.unwrap();
}
