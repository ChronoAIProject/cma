use super::*;
use crate::store::account_deletions::MongoAccountDeletionStore;
use layer2_core::{
    account_deletion::{AccountDeletionError, AccountDeletionStore, AccountStanding},
    sandbox::agent_io::CommandAuthorizationError,
    SubjectId,
};
use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions, Client};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{collections::BTreeSet, sync::Arc, time::Duration};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn allowed_requests_read_current_mongo_authority() {
    use axum::{body::Body, http::Request, routing::get, Router};
    use tower::ServiceExt;
    let mut options = ClientOptions::parse(
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("isolated replica set required"),
    )
    .await
    .unwrap();
    let commands = Arc::new(AtomicUsize::new(0));
    let observed = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CommandEvent::Started(ref event) if event.command_name == "find") {
            observed.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let client = Client::with_options(options).unwrap();
    let db = client.database(&format!("cma_standing_{}", uuid::Uuid::new_v4().simple()));
    let store = Arc::new(MongoAccountDeletionStore::open(&db).await.unwrap());
    store
        .request(
            &SubjectId::new("deleting-owner"),
            "delete",
            "2026-11-14T00:00:00Z",
        )
        .await
        .unwrap();
    let snapshot = Arc::new(ErasingSnapshot::new(store));
    snapshot.refresh().await.unwrap();
    let auth = super::super::AuthState::in_memory(
        super::super::AuthMode::Trusted {
            subject: "other-owner".into(),
        },
        None,
    )
    .with_account_standing(snapshot);
    let app = Router::new()
        .route("/api/v1/sandboxes", get(|| async { "data" }))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            super::super::middleware::attach_subject,
        ));
    commands.store(0, Ordering::SeqCst);
    for _ in 0..100 {
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/v1/sandboxes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }
    assert_eq!(commands.load(Ordering::SeqCst), 200);
    db.drop().await.unwrap();
}

struct GatedMongoPointSource {
    store: Arc<MongoAccountDeletionStore>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}
#[async_trait::async_trait]
impl SnapshotSource for GatedMongoPointSource {
    async fn active_hashes(&self) -> Result<BTreeSet<String>, AccountDeletionError> {
        self.store.active_hashes().await
    }
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        self.store.erasing(subject).await
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_frames_issue_independent_real_mongo_reads() {
    const FRAMES: usize = 32;
    for erasing in [false, true] {
        let mut options = ClientOptions::parse(
            std::env::var("CHRONO_TEST_MONGODB_URI").expect("isolated replica set required"),
        )
        .await
        .unwrap();
        let point_reads = Arc::new(AtomicUsize::new(0));
        let observed = point_reads.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                if event.command_name == "find"
                    && event.command.get_str("find").ok()
                        == Some(crate::store::account_deletions::RECORDS)
                {
                    observed.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
        let client = Client::with_options(options).unwrap();
        let db = client.database(&format!(
            "cma_standing_fallback_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let store = Arc::new(MongoAccountDeletionStore::open(&db).await.unwrap());
        let owner = SubjectId::new("owner");
        if erasing {
            store
                .request(&owner, "delete", "2026-11-14T00:00:00Z")
                .await
                .unwrap();
        }
        let source = Arc::new(GatedMongoPointSource {
            store,
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        });
        let gate = Arc::new(ErasingSnapshot::new(source.clone()));
        gate.set_fixture_snapshot_age(Duration::from_secs(11));
        let barrier = Arc::new(tokio::sync::Barrier::new(FRAMES + 1));
        let started = Arc::new(AtomicUsize::new(0));
        let mut frames = tokio::task::JoinSet::new();
        point_reads.store(0, Ordering::SeqCst);
        for _ in 0..FRAMES {
            let gate = gate.clone();
            let owner = owner.clone();
            let barrier = barrier.clone();
            let started = started.clone();
            frames.spawn(async move {
                barrier.wait().await;
                started.fetch_add(1, Ordering::SeqCst);
                gate.erasing(&owner).await
            });
        }
        barrier.wait().await;
        source.entered.notified().await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while started.load(Ordering::SeqCst) < FRAMES {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        source.release.add_permits(FRAMES);
        while let Some(frame) = frames.join_next().await {
            assert_eq!(frame.unwrap(), Ok(erasing));
        }
        assert_eq!(point_reads.load(Ordering::SeqCst), FRAMES);
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn document_grant_transaction_refuses_erasing_recipient() {
    use layer2_core::{document_artifacts::*, Authorization, Caller, Capability};
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let db = client.database(&format!(
        "cma_standing_grant_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let deletions = Arc::new(MongoAccountDeletionStore::open(&db).await.unwrap());
    let standing = Arc::new(GrantStanding {
        store: deletions.clone(),
        checked: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let rows = Arc::new(crate::document_artifacts::MongoDocumentStore::new(
        db.clone(),
    ));
    rows.ensure_indexes().await.unwrap();
    let service = DocumentService::new(
        rows.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    )
    .with_account_standing(standing.clone())
    .with_directory(memory::recipient_directory(&["recipient"]));
    let owner = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let document = service
        .create(
            &owner,
            "create",
            CreateDocument {
                title: "Title".into(),
                markdown: "# Source".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    let before = serde_json::to_value(rows.get(&document.id).await.unwrap()).unwrap();
    let grant = service.grant(
        &owner,
        &document.id,
        "grant",
        "recipient",
        GrantDocument {
            expected_revision: 1,
            permission: DocumentPermission::Read,
        },
    );
    let erase = async {
        standing.checked.notified().await;
        deletions
            .request(
                &SubjectId::new("recipient"),
                "delete",
                "2026-11-14T00:00:00Z",
            )
            .await
            .unwrap();
        standing.release.notify_one();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(grant, erase)
    })
    .await
    .unwrap();
    assert_eq!(result, Err(DocumentError::ShareRecipientUnavailable));
    assert_eq!(
        serde_json::to_value(rows.get(&document.id).await.unwrap()).unwrap(),
        before
    );
    assert!(service
        .shares(&owner, &document.id)
        .await
        .unwrap()
        .is_empty());
    db.drop().await.unwrap();
}

struct GrantStanding {
    store: Arc<MongoAccountDeletionStore>,
    checked: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for GrantStanding {
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        self.store.erasing(subject).await
    }

    async fn authoritative(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        assert_eq!(subject.as_str(), "recipient");
        assert!(!self.store.erasing(subject).await?);
        // Hold the successful check until the tombstone commits, before the grant CAS.
        self.checked.notify_one();
        self.release.notified().await;
        Ok(false)
    }
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn resource_share_transaction_refuses_erasing_recipient() {
    use crate::auth::test_resources;
    use layer2_core::resource_access::*;
    use layer2_core::sandbox::store::{AgentStore, SandboxStore, WorkspaceStore, WorktreeStore};
    use layer2_core::sandbox::{AgentStatus, AgentWorkState, AgentWorkStep};
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let db = client.database(&format!(
        "cma_standing_share_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let deletions = MongoAccountDeletionStore::open(&db).await.unwrap();
    let store = crate::store::MongoSandboxStore::open(&client, db.name())
        .await
        .unwrap();
    let sandbox = test_resources::sandbox("standing-grant", "owner");
    let workspace = test_resources::workspace("standing-workspace", "standing-grant", "owner");
    let worktree = test_resources::worktree(
        "standing-worktree",
        "standing-workspace",
        "standing-grant",
        "standing-agent",
        "owner",
    );
    let mut agent = test_resources::agent(
        "standing-agent",
        "standing-worktree",
        "standing-workspace",
        "standing-grant",
        "owner",
        AgentStatus::Starting,
    );
    agent.conversation.last_seq = 0;
    agent.work = AgentWorkState::new(AgentWorkStep::AddWorktree, agent.created_at.clone());
    let target = ResourceTarget::Agent {
        agent_id: agent.id.clone(),
    };
    store.create_sandbox(sandbox).await.unwrap();
    store.create_workspace(workspace).await.unwrap();
    store.create_worktree(worktree).await.unwrap();
    store.create_agent(agent).await.unwrap();
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let request = ShareMutation {
        selector: ResourceSelector::Agent {
            agent_id: snapshot.lineage.agent.as_ref().unwrap().id.clone(),
        },
        owner: ResourceOwner::Person {
            subject: SubjectId::new("owner"),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new("owner"),
        },
        key_sha256: layer2_core::document_artifacts::digest(b"grant"),
        change: ShareChange::CreateLink {
            audience: AgentShareAudience::Users {
                subjects: vec![SubjectId::new("recipient")],
            },
            mode: ShareMode::View,
            token_sha256: layer2_core::document_artifacts::digest(b"standing-link"),
        },
        expected_share_revision: None,
        policy_fence: snapshot.fence(),
        proposed_id: "shr_standing".into(),
        at: "2026-11-14T00:00:00Z".into(),
    };
    plan_share_mutation(&request, &snapshot, None, 0).unwrap();
    deletions
        .request(
            &SubjectId::new("recipient"),
            "delete",
            "2026-11-14T00:00:00Z",
        )
        .await
        .unwrap();
    assert_eq!(
        store.mutate_share(request).await,
        Err(ResourceAccessStoreError::ShareRecipientUnavailable)
    );
    assert_eq!(
        store.resource_access_snapshot(&target).await.unwrap(),
        snapshot
    );
    db.drop().await.unwrap();
}

struct NoSubscriptionTokens;
#[async_trait::async_trait]
impl crate::connections::subscription_store::SubscriptionStore for NoSubscriptionTokens {
    async fn get(
        &self,
        _: &SubjectId,
    ) -> Result<
        Option<crate::connections::subscription_store::SubscriptionRecord>,
        crate::connections::SubscriptionError,
    > {
        panic!("erasing owner cannot read subscription custody")
    }
    async fn insert(
        &self,
        _: crate::connections::subscription_store::SubscriptionRecord,
    ) -> Result<bool, crate::connections::SubscriptionError> {
        unreachable!()
    }
    async fn replace(
        &self,
        _: &crate::connections::subscription_store::SubscriptionRecord,
        _: crate::connections::subscription_store::SubscriptionRecord,
    ) -> Result<bool, crate::connections::SubscriptionError> {
        unreachable!()
    }
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_subscriptions_refuse_erasing_owner_before_token_custody() {
    use layer2_core::sandbox::store::{
        SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore,
    };
    use layer3_adapter::hosted::HostedSubscriptionSource;
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let db = client.database(&format!(
        "cma_standing_runtime_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let deletions = Arc::new(MongoAccountDeletionStore::open(&db).await.unwrap());
    let store = Arc::new(
        crate::store::MongoSandboxStore::open(&client, db.name())
            .await
            .unwrap(),
    );
    let sandbox = crate::store::sandbox_admission_mongo_integration_tests::sandbox(
        "standing-runtime",
        "standing-launch",
    );
    let id = sandbox.id.clone();
    let name = sandbox.name.clone();
    store
        .admit_sandbox(
            SandboxAdmission::new(
                sandbox,
                SandboxAdmissionName::Allocate(name),
                100,
                "2026-11-14T00:00:00Z".into(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    deletions
        .request(&SubjectId::new("owner"), "delete", "2026-11-14T00:00:00Z")
        .await
        .unwrap();
    let snapshot = Arc::new(ErasingSnapshot::new(deletions));
    snapshot.refresh().await.unwrap();
    let subscriptions = crate::connections::CodexSubscriptions::new(Arc::new(NoSubscriptionTokens));
    let runtime = crate::connections::RuntimeSubscriptions::new(store, subscriptions)
        .with_account_standing(snapshot);
    let result = runtime
        .credential(
            id.as_str(),
            &layer3_adapter::RuntimeHandle("runtime".into()),
            &layer3_adapter::AgentSlot("agent".into()),
            false,
            None,
        )
        .await;
    assert!(matches!(
        result,
        Err(layer3_adapter::AdapterError::AccessRevoked)
    ));
    db.drop().await.unwrap();
}

#[path = "account_standing_mongo_tests/replicas.rs"]
mod replicas;
