use super::*;
use layer2_core::{OperationClaim, OperationReservation, OperationStore};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

impl Fixture {
    pub(super) async fn raw(&self, ingress: usize, key: &str, body: &str) -> reqwest::Response {
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
        self.restarted_routes(identity)
            .await
            .oneshot(
                axum::http::Request::builder()
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
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap()
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
