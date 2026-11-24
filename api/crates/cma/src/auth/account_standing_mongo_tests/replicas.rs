use super::*;
use axum::{body::Body, http::Request, routing::get, Router};
use futures_util::StreamExt;
use layer1_public::agui::{launch_companion_router, LaunchCompanionState};
use layer2_core::{
    account_deletion::{AccountDeletionService, OwnerSessionAuthority},
    trigger_domain::*,
    Authorization, Caller, LaunchId,
};
use tower::ServiceExt;

const NOW: &str = "2026-11-20T00:00:00Z";

struct RetainedLaunch(Launch);
#[async_trait::async_trait]
impl LaunchStore for RetainedLaunch {
    async fn admit_launch(
        &self,
        _: LaunchAdmission,
    ) -> Result<LaunchAdmissionResult, LaunchStoreError> {
        unreachable!()
    }
    async fn launch_by_command(
        &self,
        _: &SubjectId,
        _: &PublicTriggerId,
        _: &str,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        unreachable!()
    }
    async fn launch(
        &self,
        id: &LaunchId,
        owner: &SubjectId,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        Ok((&self.0.id == id && &self.0.owner == owner).then(|| self.0.clone()))
    }
    async fn list_launches(
        &self,
        _: &SubjectId,
        _: &LaunchPageRequest,
    ) -> Result<LaunchPage, LaunchStoreError> {
        unreachable!()
    }
    async fn placement_stats(
        &self,
        _: &TriggerId,
    ) -> Result<Vec<LaunchPlacementStats>, LaunchStoreError> {
        unreachable!()
    }
    async fn commit_launch(&self, _: LaunchCommit) -> Result<Launch, LaunchStoreError> {
        unreachable!()
    }
    async fn launch_events(
        &self,
        id: &LaunchId,
        owner: &SubjectId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<LaunchEvent>, LaunchStoreError> {
        self.launch(id, owner)
            .await?
            .ok_or(LaunchStoreError::NotFound)?;
        Ok([LaunchEventKind::Created, LaunchEventKind::Completed]
            .into_iter()
            .enumerate()
            .map(|(index, kind)| LaunchEvent {
                launch_id: id.clone(),
                seq: index as u64 + 1,
                appended_at: NOW.into(),
                kind,
            })
            .filter(|event| event.seq > after)
            .take(usize::from(limit))
            .collect())
    }
    async fn due_launches(&self, _: &str, _: u16) -> Result<Vec<LaunchId>, LaunchStoreError> {
        unreachable!()
    }
    async fn claim_launch(
        &self,
        _: &LaunchId,
        _: &str,
        _: String,
        _: String,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        unreachable!()
    }
}

fn retained_launch() -> Launch {
    let mut launch = Launch::new(LaunchSeed {
        trigger_id: TriggerId("trg_one".into()),
        revision_digest: "a".repeat(64),
        public_id: PublicTriggerId("pub_one".into()),
        owner: SubjectId::new("visitor"),
        authority: LaunchAuthorityClass::SandboxCreator,
        placement_id: None,
        referrer_origin: None,
        requirements: RequirementsSnapshot {
            services: vec![],
            checked_at: NOW.into(),
        },
        review: ReviewRequirements {
            needs: vec![],
            can_start_now: true,
        },
        trust: LaunchTrustManifest::none(),
        resolution: LaunchResolution::pending(),
        has_first_message: false,
        command_key_hash: "b".repeat(64),
        command_fingerprint: "c".repeat(64),
        created_at: NOW.into(),
    })
    .unwrap();
    launch.event_head = 2;
    launch
}

async fn router(
    standing: Arc<ErasingSnapshot>,
) -> (Router, Arc<super::super::super::AuthState>, String) {
    let launch = retained_launch();
    let path = format!("/api/v1/agui/launches/{}/events?follow=false", launch.id);
    let auth = super::super::super::AuthState::in_memory(
        super::super::super::AuthMode::Trusted {
            subject: "visitor".into(),
        },
        None,
    )
    .with_account_standing(standing.clone());
    let router = launch_companion_router(
        LaunchCompanionState::new(Arc::new(RetainedLaunch(launch))).with_account_standing(standing),
    )
    .route("/api/v1/protected", get(|| async { "owned data" }))
    .layer(axum::middleware::from_fn_with_state(
        auth.clone(),
        super::super::super::middleware::attach_subject,
    ));
    (router, auth, path)
}

async fn get_response(app: &Router, path: &str) -> axum::response::Response {
    app.clone()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn other_replica_acceptance_revokes_fresh_request_stream_frame_and_command() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let writer = Client::with_uri_str(&uri).await.unwrap();
    let reader = Client::with_uri_str(&uri).await.unwrap();
    let db = writer.database(&format!(
        "cma_standing_replicas_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let first = Arc::new(MongoAccountDeletionStore::open(&db).await.unwrap());
    let other = Arc::new(MongoAccountDeletionStore::from_database(
        &reader.database(db.name()),
    ));
    let accepting = Arc::new(ErasingSnapshot::new(first.clone()));
    let observing = Arc::new(ErasingSnapshot::new(other));
    accepting.refresh().await.unwrap();
    observing.refresh().await.unwrap();
    let owner = SubjectId::new("visitor");
    assert!(!observing.erasing(&owner).await.unwrap());
    let (app, auth, path) = router(observing.clone()).await;
    assert_eq!(get_response(&app, "/api/v1/protected").await.status(), 200);
    let response = get_response(&app, &path).await;
    assert_eq!(response.status(), 200);
    let mut frames = response.into_body().into_data_stream();
    assert!(String::from_utf8_lossy(&frames.next().await.unwrap().unwrap()).contains("id: 1"));
    let service = AccountDeletionService::new(first, accepting, true);
    let caller = Caller::new(owner.clone(), Authorization::default());
    let authority = OwnerSessionAuthority::verified_personal(owner.clone());
    service
        .request(&caller, Some(&authority), "delete", true)
        .await
        .unwrap();
    assert!(
        observing.fixture_hashes_empty(),
        "replica B has not refreshed"
    );
    assert_eq!(
        auth.require_command_account(&owner).await,
        Err(CommandAuthorizationError::Revoked)
    );
    let response = get_response(&app, "/api/v1/protected").await;
    assert_eq!(response.status(), 403);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["code"], "account_deletion_in_progress");
    assert_eq!(get_response(&app, &path).await.status(), 403);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), frames.next())
            .await
            .unwrap()
            .is_none(),
        "buffered pre-deletion frame escaped after durable acceptance"
    );
    assert!(!observing
        .erasing(&SubjectId::new("other-owner"))
        .await
        .unwrap());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CMA_IM_TEST_MONGODB_URI pointing at an authenticated replica set"]
async fn fresh_snapshot_cannot_mask_real_database_permission_failure() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let db = admin.database(&format!(
        "cma_standing_denied_{}",
        uuid::Uuid::new_v4().simple()
    ));
    MongoAccountDeletionStore::open(&db).await.unwrap();
    let mut anonymous_options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
    anonymous_options.credential = None;
    let anonymous = Client::with_options(anonymous_options).unwrap();
    let denied = anonymous
        .database(db.name())
        .collection::<mongodb::bson::Document>("account_deletions")
        .find_one(mongodb::bson::doc! {})
        .await
        .expect_err("the permission-loss fixture must reject unauthenticated reads");
    assert!(
        matches!(denied.kind.as_ref(), mongodb::error::ErrorKind::Command(error) if error.code == 13)
    );
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(
        mongodb::bson::doc! {"createUser":"standing-reader","pwd":&password,
        "roles":[{"role":"read","db":db.name()}]},
    )
    .await
    .unwrap();
    let mut uri = url::Url::parse(&uri).unwrap();
    uri.set_username("standing-reader").unwrap();
    uri.set_password(Some(&password)).unwrap();
    uri.query_pairs_mut().append_pair("authSource", db.name());
    let reader = Client::with_uri_str(uri.as_str()).await.unwrap();
    let standing = Arc::new(ErasingSnapshot::new(Arc::new(
        MongoAccountDeletionStore::from_database(&reader.database(db.name())),
    )));
    standing.refresh().await.unwrap();
    let (app, auth, path) = router(standing).await;
    let response = get_response(&app, &path).await;
    assert_eq!(response.status(), 200);
    let mut frames = response.into_body().into_data_stream();
    assert!(frames.next().await.unwrap().is_ok());
    db.run_command(
        mongodb::bson::doc! {"revokeRolesFromUser":"standing-reader",
        "roles":[{"role":"read","db":db.name()}]},
    )
    .await
    .unwrap();
    assert_eq!(get_response(&app, "/api/v1/protected").await.status(), 503);
    assert_eq!(
        auth.require_command_account(&SubjectId::new("visitor"))
            .await,
        Err(CommandAuthorizationError::Unavailable)
    );
    assert!(frames.next().await.is_none());
    db.run_command(mongodb::bson::doc! {"dropUser":"standing-reader"})
        .await
        .unwrap();
    db.drop().await.unwrap();
}
