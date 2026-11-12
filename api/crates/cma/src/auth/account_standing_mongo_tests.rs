use super::*;
use layer2_core::account_deletion::AccountDeletionStore;
use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions, Client};
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn gate_adds_no_mongo_command_per_request() {
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
        if matches!(event, CommandEvent::Started(_)) {
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
    assert_eq!(commands.load(Ordering::SeqCst), 0);
    db.drop().await.unwrap();
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
    let deletions = MongoAccountDeletionStore::open(&db).await.unwrap();
    let rows = Arc::new(crate::document_artifacts::MongoDocumentStore::new(
        db.clone(),
    ));
    rows.ensure_indexes().await.unwrap();
    let service = DocumentService::new(
        rows.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    );
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
    deletions
        .request(
            &SubjectId::new("recipient"),
            "delete",
            "2026-11-14T00:00:00Z",
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .grant(
                &owner,
                &document.id,
                "grant",
                "recipient",
                GrantDocument {
                    expected_revision: 1,
                    permission: DocumentPermission::Read
                }
            )
            .await,
        Err(DocumentError::ShareRecipientUnavailable)
    );
    assert!(rows
        .get(&document.id)
        .await
        .unwrap()
        .unwrap()
        .shares
        .is_empty());
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn resource_share_transaction_refuses_erasing_recipient() {
    use layer2_core::resource_access::*;
    use layer2_core::sandbox::store::{
        SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore,
    };
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
    let sandbox = crate::store::sandbox_admission_mongo_integration_tests::sandbox(
        "standing-grant",
        "standing-launch",
    );
    let target = ResourceTarget::Sandbox {
        sandbox_id: sandbox.id.clone(),
    };
    store
        .admit_sandbox(
            SandboxAdmission::new(
                sandbox,
                SandboxAdmissionName::Allocate("standing".into()),
                100,
                "2026-11-14T00:00:00Z".into(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    deletions
        .request(
            &SubjectId::new("recipient"),
            "delete",
            "2026-11-14T00:00:00Z",
        )
        .await
        .unwrap();
    let request = ShareMutation {
        selector: ResourceSelector::Sandbox {
            sandbox_id: snapshot.lineage.sandbox.id.clone(),
        },
        owner: ResourceOwner::Person {
            subject: SubjectId::new("owner"),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new("owner"),
        },
        key_sha256: layer2_core::document_artifacts::digest(b"grant"),
        change: ShareChange::Create {
            target_org_id: SubjectId::new("recipient"),
            mode: ShareMode::View,
        },
        expected_share_revision: None,
        policy_fence: snapshot.fence(),
        proposed_id: "shr_standing".into(),
        at: "2026-11-14T00:00:00Z".into(),
    };
    assert_eq!(
        store.mutate_share(request).await,
        Err(ResourceAccessStoreError::ShareRecipientUnavailable)
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
    store
        .admit_sandbox(
            SandboxAdmission::new(
                sandbox,
                SandboxAdmissionName::Allocate("standing".into()),
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
