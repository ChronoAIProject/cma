use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use layer1_public::rest::{hierarchy_router, HierarchyState};
use layer2_core::{
    profiles::store::InMemoryProfileStore,
    sandbox::{
        FixedSandboxCredentialPrerequisite, HierarchyReadService, RetainedRecoveryDisposition as D,
        RetainedRecoveryInspector, RetainedRecoveryService, RetainedRecoverySnapshot,
        SandboxPolicy, SandboxService, WorkspaceService,
    },
};
use serde_json::{json, Value};
use tower::ServiceExt;

struct RuntimeProof;

#[async_trait::async_trait]
impl RetainedRecoveryInspector for RuntimeProof {
    async fn inspect(&self, snapshot: &RetainedRecoverySnapshot) -> D {
        if snapshot.agents.is_empty() {
            D::ExactSuspendedRuntime
        } else {
            D::RecordedBootPresent
        }
    }
}

#[async_trait::async_trait]
impl RetainedReopenVerifier for RuntimeProof {
    async fn verify(
        &self,
        snapshot: &RetainedReopenSnapshot,
    ) -> Result<RetainedReopenProof, SandboxStoreError> {
        let mut proof = legacy_request(snapshot.clone(), "a".repeat(64)).proof;
        proof.observed_boot = snapshot.retained.source().unwrap().expected_boot.into();
        Ok(proof)
    }

    async fn verify_suspended(
        &self,
        snapshot: &RetainedReopenSnapshot,
    ) -> Result<RetainedSuspendedReopenProof, SandboxStoreError> {
        proof(snapshot)
    }
}

fn router(
    state: &HierarchyState,
    store: &Arc<MongoSandboxStore>,
    idle: bool,
    suspended: bool,
) -> Router {
    hierarchy_router(
        state.clone().with_retained_reopening(Arc::new(
            RetainedReopenService::new(store.clone(), Arc::new(RuntimeProof), idle)
                .with_suspended_admission(suspended),
        )),
    )
}

async fn response(
    router: &Router,
    owner: &SubjectId,
    id: &SandboxId,
    consent: Option<(u64, &str)>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().uri(format!("/api/v1/sandboxes/{id}/runtime-recovery"));
    let body = match consent {
        Some((revision, key)) => {
            request = request
                .method(Method::POST)
                .header("content-type", "application/json")
                .header("idempotency-key", key);
            Body::from(
                json!({
                    "expected_revision": revision,
                    "confirm_same_image_recovery": true,
                })
                .to_string(),
            )
        }
        None => Body::empty(),
    };
    let request = request
        .extension(layer1_public::AuthenticatedSubject(owner.clone()))
        .extension(Authorization::new([Capability::UseOwnSandboxes]))
        .body(body)
        .unwrap();
    let reply = router.clone().oneshot(request).await.unwrap();
    let status = reply.status();
    let bytes = to_bytes(reply.into_body(), 64 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn owner_assessment_offers_only_the_verified_path_and_replays_after_disabling() {
    let (client, name, store) = database().await;
    let idle =
        super::super::super::mongo_fixtures::reopen_fixture(&store, "sbx_consent_idle").await;
    let idle = idle.snapshot.retained.sandbox;
    let child = live(&store, "sbx_consent_live", true).await;
    let parent = store.get_sandbox(&child.sandbox_id).await.unwrap().unwrap();
    let empty =
        super::super::super::mongo_fixtures::reopen_fixture(&store, "sbx_consent_inspection").await;
    let empty_agent = &empty.snapshot.retained.agents[0];
    store
        .agents
        .delete_one(doc! {"_id":empty_agent.id.as_str()})
        .await
        .unwrap();
    store
        .worktrees
        .delete_one(doc! {"_id":empty_agent.worktree_id.as_str()})
        .await
        .unwrap();
    store
        .agent_log
        .delete_many(doc! {"agent_id":empty_agent.id.as_str()})
        .await
        .unwrap();
    let empty = empty.snapshot.retained.sandbox;
    let store = Arc::new(store);
    let profiles = Arc::new(InMemoryProfileStore::default());
    let operations = Arc::new(
        crate::store::operation::MongoOperationStore::open(&client, &name)
            .await
            .unwrap(),
    );
    let state = HierarchyState::new(
        SandboxService::new(
            store.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(store.clone(), profiles),
        HierarchyReadService::new(store.clone()),
        operations,
    )
    .with_retained_recovery(Arc::new(RetainedRecoveryService::new(
        store.clone(),
        Arc::new(RuntimeProof),
    )));

    for (idle_on, suspended_on) in [(false, false), (true, false), (false, true), (true, true)] {
        let app = router(&state, &store, idle_on, suspended_on);
        let live_disposition = if suspended_on {
            D::ExactSuspendedRuntime
        } else {
            D::ChildActivity
        };
        for (target, disposition, enabled) in [
            (&idle, D::RecordedBootPresent, idle_on),
            (&parent, live_disposition, suspended_on),
            (&empty, D::ExactSuspendedRuntime, false),
        ] {
            let before = row(&store, &target.id).await;
            assert_eq!(
                response(&app, &target.owner, &target.id, None).await,
                (
                    StatusCode::OK,
                    json!({"disposition":disposition.as_str(), "admission_enabled":enabled})
                )
            );
            assert_eq!(row(&store, &target.id).await, before);
            assert!(store
                .sandbox_events(&target.id, 0, 10)
                .await
                .unwrap()
                .is_empty());
        }
    }
    assert_eq!(store.get_agent(&child.id).await.unwrap().unwrap(), child);

    let app = router(&state, &store, false, true);
    let mut current = parent.clone();
    current.revision += 1;
    current.name = "updated-name".into();
    store
        .save_sandbox(parent.revision, current.clone())
        .await
        .unwrap();
    let key = "approved-suspended-recovery";
    let stale = response(
        &app,
        &current.owner,
        &current.id,
        Some((parent.revision, key)),
    )
    .await;
    assert_eq!(stale.0, StatusCode::CONFLICT);
    assert!(store
        .sandbox_events(&current.id, 0, 10)
        .await
        .unwrap()
        .is_empty());
    let accepted = response(
        &app,
        &current.owner,
        &current.id,
        Some((current.revision, key)),
    )
    .await;
    assert_eq!(accepted.0, StatusCode::ACCEPTED);
    assert_eq!(accepted.1["revision"], current.revision + 1);
    assert_eq!(
        store
            .sandbox_events(&current.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    let persisted = row(&store, &current.id).await;
    let off = router(&state, &store, false, false);
    assert_eq!(
        response(
            &off,
            &current.owner,
            &current.id,
            Some((current.revision, key))
        )
        .await,
        accepted,
    );
    assert_eq!(row(&store, &current.id).await, persisted);
    client.database(&name).drop().await.unwrap();
}
