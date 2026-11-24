use super::*;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use layer2_core::{OperationClaim, OperationReservation, OperationStore};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

impl Fixture {
    pub(super) async fn raw(&self, ingress: usize, key: &str, body: &str) -> reqwest::Response {
        self.raw_with_headers(ingress, key, body, HeaderMap::new())
            .await
    }

    async fn raw_with_headers(
        &self,
        ingress: usize,
        key: &str,
        body: &str,
        headers: HeaderMap,
    ) -> reqwest::Response {
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(12))
            .build()
            .unwrap()
            .request(
                if self.deleting {
                    Method::DELETE
                } else {
                    Method::POST
                },
                format!(
                    "{}/api/v1/agents/{}{}",
                    self.replicas[ingress].address,
                    self.agent.id,
                    if self.deleting { "" } else { "/stop" }
                ),
            )
            .headers(headers)
            .header("idempotency-key", key)
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .unwrap()
    }

    async fn restarted_routes(&self, identity: &str) -> Router {
        let client =
            Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
                .await
                .unwrap();
        let store = Arc::new(MongoSandboxStore::open(&client, &self.name).await.unwrap());
        let registry = self.h.registry.clone();
        let agents = AgentService::new(
            store.clone(),
            Arc::new(layer2_core::profiles::store::InMemoryProfileStore::default()),
            registry.clone(),
            Default::default(),
        );
        let reads = AgentReadService::new(store.clone(), Arc::new(Resources));
        let operations = AgentOperationService::new(store.clone(), registry);
        let ledger = Arc::new(
            MongoOperationStore::open_for_test(&client, &self.name)
                .await
                .unwrap(),
        );
        let router = layer1_public::rest::agent_router(layer1_public::rest::AgentRoutesState::new(
            layer1_public::agent_page::AgentPageOrigin::new("http://localhost").unwrap(),
            agents,
            reads,
            operations,
            ledger,
        ));
        routes::compose(
            self.replicas[self.owner].coordinator.clone(),
            &self.h.updates,
            router,
            store,
            self.h.adapter.clone(),
            Some(identity),
        )
    }

    async fn restart_request(
        &self,
        identity: &str,
        target: &AgentId,
        key: &str,
    ) -> axum::response::Response {
        self.restart_request_with_headers(identity, target, key, HeaderMap::new())
            .await
    }

    async fn restart_request_with_headers(
        &self,
        identity: &str,
        target: &AgentId,
        key: &str,
        headers: HeaderMap,
    ) -> axum::response::Response {
        let mut request = axum::http::Request::builder()
            .method(if self.deleting {
                Method::DELETE
            } else {
                Method::POST
            })
            .uri(format!(
                "/api/v1/agents/{}{}",
                target,
                if self.deleting { "" } else { "/stop" }
            ))
            .header("idempotency-key", key)
            .header("content-type", "application/json");
        request.headers_mut().unwrap().extend(headers);
        self.restarted_routes(identity)
            .await
            .oneshot(request.body(axum::body::Body::from("{}")).unwrap())
            .await
            .unwrap()
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn ordinary_stop_and_delete_replay_ignore_qualification_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-cma-qualification-case",
        HeaderValue::from_static("fixture-case"),
    );
    headers.insert(
        "x-cma-qualification-actor",
        HeaderValue::from_static("fixture-observer"),
    );

    for deleting in [false, true] {
        let fixture = Fixture::new(deleting).await;
        let key = uuid::Uuid::new_v4().to_string();
        let operation = format!(
            "agent:{}:{}",
            if deleting { "delete" } else { "stop" },
            fixture.agent.id
        );
        let claims = fixture
            .admin
            .database(&fixture.name)
            .collection::<mongodb::bson::Document>("operation_claims");
        let claim_key = doc! {"_id.operation": operation, "_id.key": &key};
        let mut denied_key = claim_key.clone();
        denied_key.insert("_id.subject", "outsider");
        let mut owner_key = claim_key.clone();
        owner_key.insert("_id.subject", "owner");
        let mut header_actor_key = claim_key.clone();
        header_actor_key.insert("_id.subject", "fixture-observer");

        let denied = fixture
            .restart_request_with_headers("outsider", &fixture.agent.id, &key, headers.clone())
            .await;
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
        assert!(fixture.control_receipt().await.is_none());
        assert_eq!(
            fixture.h.stored(&fixture.agent.id).await.revision,
            fixture.agent.revision
        );
        // HTTP idempotency records the 404 for the outsider without scheduling work.
        assert_eq!(claims.count_documents(claim_key.clone()).await.unwrap(), 1);
        assert_eq!(claims.count_documents(owner_key.clone()).await.unwrap(), 0);
        let denied_claim = claims.find_one(denied_key.clone()).await.unwrap().unwrap();
        let denied_outcome: Value =
            serde_json::from_slice(denied_claim.get_binary_generic("outcome").unwrap()).unwrap();
        assert_eq!(denied_outcome["status"], 404);

        let first = fixture
            .raw_with_headers(1 - fixture.owner, &key, "{}", headers.clone())
            .await;
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        let accepted = first.bytes().await.unwrap();
        assert!(accepted.is_empty());
        let receipt = fixture
            .control_receipt()
            .await
            .expect("durable scheduling receipt");
        assert_eq!(
            receipt.get_i64("scheduled_revision").unwrap() as u64,
            fixture.agent.revision + 1
        );

        let replay = fixture
            .raw_with_headers(fixture.owner, &key, "{}", headers.clone())
            .await;
        assert_eq!(replay.status(), StatusCode::ACCEPTED);
        assert_eq!(replay.bytes().await.unwrap(), accepted);
        assert_eq!(fixture.control_receipt().await, Some(receipt));
        assert_eq!(
            fixture.h.stored(&fixture.agent.id).await.revision,
            fixture.agent.revision + 1
        );
        assert_eq!(claims.count_documents(claim_key).await.unwrap(), 2);
        assert_eq!(claims.count_documents(owner_key).await.unwrap(), 1);
        assert_eq!(claims.count_documents(header_actor_key).await.unwrap(), 0);
        assert_eq!(
            claims.find_one(denied_key).await.unwrap(),
            Some(denied_claim)
        );
        fixture.completed_response(&key, 202).await;
        fixture.complete().await;
        fixture.close().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn exact_key_replay_survives_restart_but_different_subject_target_or_body_cannot_reuse_it() {
    for deleting in [false, true] {
        let f = Fixture::new(deleting).await;
        let key = uuid::Uuid::new_v4().to_string();
        f.request_key("exact_key_first", f.owner, 202, None, &key)
            .await;
        let receipt = f.control_receipt().await;
        f.request_key("exact_key_other_ingress", 1 - f.owner, 202, None, &key)
            .await;
        assert_eq!(
            f.restart_request("owner", &f.agent.id, &key).await.status(),
            202
        );
        assert_eq!(
            f.restart_request("outsider", &f.agent.id, &key)
                .await
                .status(),
            404
        );
        assert_eq!(
            f.restart_request("owner", &AgentId::generate(), &key)
                .await
                .status(),
            404
        );
        if !deleting {
            let mismatch = f.raw(1 - f.owner, &key, r#"{"reason":"owner"}"#).await;
            assert_eq!(mismatch.status(), 422);
            assert_eq!(
                mismatch.json::<Value>().await.unwrap()["code"],
                "idempotency_key_reused"
            );
        }
        assert_eq!(f.control_receipt().await, receipt);
        assert_eq!(f.h.stored(&f.agent.id).await.revision, f.agent.revision + 1);
        f.complete().await;
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn pending_and_expired_http_claims_never_reexecute_after_restart() {
    for deleting in [false, true] {
        let f = Fixture::new(deleting).await;
        let ledger = MongoOperationStore::open_for_test(&f.admin, &f.name)
            .await
            .unwrap();
        let key = uuid::Uuid::new_v4().to_string();
        let fingerprint = Sha256::digest(if deleting { &b""[..] } else { &b"{}"[..] }).into();
        let claim = OperationClaim::new(
            SubjectId::new("owner"),
            format!(
                "agent:{}:{}",
                if deleting { "delete" } else { "stop" },
                f.agent.id
            ),
            &key,
            fingerprint,
            SystemTime::now() + Duration::from_secs(120),
            Duration::from_secs(86_400),
        );
        assert_eq!(
            ledger.reserve(&claim).await.unwrap(),
            OperationReservation::Acquired
        );
        for expired in [false, true] {
            if expired {
                f.admin
                    .database(&f.name)
                    .collection::<mongodb::bson::Document>("operation_claims")
                    .update_one(
                        doc! {"_id.key":&key},
                        doc! {"$set":{"lease_expires_at":mongodb::bson::DateTime::from_millis(0)}},
                    )
                    .await
                    .unwrap();
            }
            let response = f.restart_request("owner", &f.agent.id, &key).await;
            assert_eq!(response.status(), 409);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let problem: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                problem["code"],
                if expired {
                    "idempotency_recovery_required"
                } else {
                    "idempotency_in_flight"
                }
            );
            assert_eq!(f.h.stored(&f.agent.id).await.revision, f.agent.revision);
            assert!(f.control_receipt().await.is_none());
        }
        f.close().await;
    }
}
