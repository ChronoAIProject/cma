use super::*;
use crate::sandbox::{auxiliary_request, AuxiliaryKind};
use layer2_core::budget::RequestHold;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::task::JoinHandle;

#[path = "http_tests/peer.rs"]
mod peer;
use peer::{json_response, Captured, Peer};

const KINDS: [AuxiliaryKind; 2] = [AuxiliaryKind::Title, AuxiliaryKind::Suggestion];

struct Fixture {
    client: Client,
    name: String,
    store: Arc<MongoSandboxStore>,
    clock: Arc<AtomicU64>,
    scope: RequestScope,
}

impl Fixture {
    async fn new() -> Self {
        let (client, name) = database("auxiliary_http").await;
        let (store, scope, _) = completed_without_finality(&client, &name, Some(65_536)).await;
        store
            .budget_settings
            .update_one(
                doc! {"_id": "platform"},
                doc! {"$set": {"settings.model_budget.request_models": [
                    {"model": "budget-model", "input_overhead_tokens": 1_i64}
                ]}},
            )
            .await
            .unwrap();
        let owner = store
            .model_budget
            .find_one(doc! {"_id": "owner:owner"})
            .await
            .unwrap()
            .unwrap();
        let day = owner
            .get_document("record")
            .unwrap()
            .get_i64("day")
            .unwrap() as u64;
        let clock = Arc::new(AtomicU64::new(day * 86_400 + 43_200));
        let mut store = (*store).clone();
        store.request_budget_clock = Some(clock.clone());
        Self {
            client,
            name,
            store: Arc::new(store),
            clock,
            scope,
        }
    }

    async fn seed_finality_precondition(&self) {
        // This is a fixture precondition, not a production certification path.
        self.store
            .model_budget
            .update_one(
                doc! {"_id": format!("turn:{}:{}", self.scope.agent_id, self.scope.turn_sequence)},
                doc! {"$set": {"record.reservation.final_usage_known": true,
                "record.reservation.final_model_calls": 1_i64}},
            )
            .await
            .unwrap();
    }

    async fn rows(&self) -> Vec<Document> {
        self.store
            .model_budget
            .find(doc! {})
            .sort(doc! {"_id": 1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap()
    }

    async fn reopen(&self) -> Arc<MongoSandboxStore> {
        let mut store = MongoSandboxStore::open(&self.client, &self.name)
            .await
            .unwrap();
        store.request_budget_clock = Some(self.clock.clone());
        Arc::new(store)
    }

    fn start(
        &self,
        kind: AuxiliaryKind,
        peer: &Peer,
        store: Arc<MongoSandboxStore>,
    ) -> JoinHandle<bool> {
        let origin = peer.origin.clone();
        let scope = self.scope.clone();
        tokio::spawn(async move { auxiliary_request(kind, &origin, store, scope).await })
    }

    async fn committed_hold(&self, captured: &Captured) -> (RequestHold, Vec<Document>) {
        let rows = self.rows().await;
        let key = format!("request:{}:{}", self.scope.agent_id, self.scope.request_id);
        let record = rows
            .iter()
            .find(|row| row.get_str("_id").unwrap() == key)
            .expect("the request hold is committed before the peer receives the body")
            .get_document("record")
            .unwrap();
        let hold: RequestHold =
            mongodb::bson::deserialize_from_document(record.get_document("hold").unwrap().clone())
                .unwrap();
        assert_eq!(hold.scope, self.scope);
        assert_eq!(record.get_str("owner").unwrap(), "owner");
        assert_eq!(record.get("settled"), Some(&mongodb::bson::Bson::Null));
        assert_eq!(hold.request.model, "budget-model");
        assert_eq!(hold.request.input_bytes, captured.body.len() as u64);
        assert_eq!(
            hold.request.fingerprint,
            format!("{:x}", Sha256::digest(&captured.body))
        );
        assert_eq!(hold.request.max_output_tokens, 1024);
        assert_eq!(hold.input_tokens_bound, hold.request.input_bytes + 1);
        assert_eq!(
            record.get_i64("reserved").unwrap() as u64,
            hold.input_tokens_bound + hold.request.max_output_tokens
        );
        let ledger_key = format!(
            "request-turn:{}:{}",
            self.scope.agent_id, self.scope.turn_sequence
        );
        for key in ["owner:owner", ledger_key.as_str()] {
            let ledger = rows
                .iter()
                .find(|row| row.get_str("_id").unwrap() == key)
                .unwrap()
                .get_document("record")
                .unwrap();
            assert_eq!(
                ledger.get_i64("reserved").unwrap(),
                record.get_i64("reserved").unwrap()
            );
            if key == ledger_key {
                assert_eq!(ledger.get_i64("calls").unwrap(), 1);
                assert_eq!(ledger.get_i64("spent").unwrap(), 0);
            }
        }
        assert_eq!(
            rows.iter()
                .filter(|row| row.get_str("_id").unwrap().starts_with("request:"))
                .count(),
            1
        );
        assert!(captured
            .headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer budget-test-bearer")));
        (hold, rows)
    }

    async fn assert_replay_refused(&self, kind: AuxiliaryKind, peer: &Peer, expected: &[Document]) {
        assert!(!finish(self.start(kind, peer, self.reopen().await)).await);
        assert_eq!(self.rows().await, expected);
        assert_eq!(
            peer.calls(),
            1,
            "reopening must not grant a second send permit"
        );
    }

    async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}

async fn finish(mut task: JoinHandle<bool>) -> bool {
    let result = tokio::time::timeout(Duration::from_secs(15), &mut task).await;
    if result.is_err() {
        task.abort();
        let _ = task.await;
    }
    result.expect("HTTP adapter finishes").unwrap()
}

fn output() -> serde_json::Value {
    serde_json::json!({
        "status": "completed", "model": "budget-model",
        "output_text": "Reserved title",
        "output": [{"type": "message", "role": "assistant", "content": [
            {"type": "output_text", "text": "{\"suggestions\":[]}"}
        ]}]
    })
}

async fn retains_hold(response: Option<Vec<u8>>, generated: bool) {
    for kind in KINDS {
        let f = Fixture::new().await;
        f.seed_finality_precondition().await;
        let mut peer = Peer::new(response.clone()).await;
        let task = f.start(kind, &peer, f.store.clone());
        let captured = peer.received().await;
        let (_, reserved) = f.committed_hold(&captured).await;
        peer.respond();
        assert_eq!(finish(task).await, generated, "{kind:?}");
        assert_eq!(peer.calls(), 1);
        assert_eq!(f.rows().await, reserved);
        f.assert_replay_refused(kind, &peer, &reserved).await;
        peer.stop().await;
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn disconnected_response_preserves_committed_auxiliary_holds_after_reopen() {
    retains_hold(None, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn truncated_response_preserves_committed_auxiliary_holds_after_reopen() {
    retains_hold(Some(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"status\":".to_vec()), false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn missing_or_mismatched_usage_never_releases_a_persisted_auxiliary_hold() {
    let absent = output();
    let mut partial = output();
    partial["usage"] = serde_json::json!({"input_tokens": 7});
    let mut wrong_model = output();
    wrong_model["model"] = serde_json::json!("another-model");
    wrong_model["usage"] = serde_json::json!({"input_tokens": 7, "output_tokens": 1});
    for response in [absent, partial, wrong_model] {
        retains_hold(Some(json_response(&response)), true).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn ordinary_parent_completion_without_certification_refuses_both_http_adapters() {
    for kind in KINDS {
        let f = Fixture::new().await;
        let peer = Peer::new(None).await;
        let before = f.rows().await;
        let parent = before
            .iter()
            .find(|row| row.get_str("_id").unwrap().starts_with("turn:"))
            .unwrap()
            .get_document("record")
            .unwrap()
            .get_document("reservation")
            .unwrap();
        assert!(parent.get_bool("settled").unwrap());
        assert!(!parent.get_bool("final_usage_known").unwrap());
        let owner = before
            .iter()
            .find(|row| row.get_str("_id").unwrap() == "owner:owner")
            .unwrap()
            .get_document("record")
            .unwrap();
        assert!(owner.get_i64("spent").unwrap() >= parent.get_i64("reserved_units").unwrap());
        assert!(parent
            .get("final_model_calls")
            .is_none_or(|value| value == &mongodb::bson::Bson::Null));
        assert!(!finish(f.start(kind, &peer, f.store.clone())).await);
        assert!(!finish(f.start(kind, &peer, f.reopen().await)).await);
        assert_eq!(peer.calls(), 0);
        assert_eq!(f.rows().await, before);
        assert!(before
            .iter()
            .all(|row| !row.get_str("_id").unwrap().starts_with("request:")));
        peer.stop().await;
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn complete_usage_settles_once_even_when_auxiliary_output_is_invalid() {
    for valid_output in [true, false] {
        for kind in KINDS {
            let f = Fixture::new().await;
            f.seed_finality_precondition().await;
            let mut response = if valid_output {
                output()
            } else {
                serde_json::json!({"status": "completed", "model": "budget-model"})
            };
            response["usage"] = serde_json::json!({"input_tokens": 7, "output_tokens": 1});
            let mut peer = Peer::new(Some(json_response(&response))).await;
            let task = f.start(kind, &peer, f.store.clone());
            let captured = peer.received().await;
            let (hold, reserved) = f.committed_hold(&captured).await;
            peer.respond();
            assert_eq!(finish(task).await, valid_output, "{kind:?}");
            let expected = settled_rows(reserved, &hold);
            assert_eq!(f.rows().await, expected);
            let reopened = f.reopen().await;
            reopened
                .settle_request(
                    &hold,
                    RequestUsage {
                        input_tokens: 7,
                        output_tokens: 1,
                    },
                )
                .await
                .unwrap();
            assert_eq!(f.rows().await, expected);
            f.assert_replay_refused(kind, &peer, &expected).await;
            peer.stop().await;
            f.cleanup().await;
        }
    }
}

fn settled_rows(mut rows: Vec<Document>, hold: &RequestHold) -> Vec<Document> {
    let request_key = format!("request:{}:{}", hold.scope.agent_id, hold.scope.request_id);
    let ledger_key = format!(
        "request-turn:{}:{}",
        hold.scope.agent_id, hold.scope.turn_sequence
    );
    let liability =
        i64::try_from(hold.input_tokens_bound + hold.request.max_output_tokens).unwrap();
    for row in &mut rows {
        let key = row.get_str("_id").unwrap().to_owned();
        let record = row.get_document_mut("record").unwrap();
        if key == "owner:owner" || key == ledger_key {
            record.insert("reserved", record.get_i64("reserved").unwrap() - liability);
            record.insert("spent", record.get_i64("spent").unwrap() + 8);
        } else if key == request_key {
            record.insert(
                "settled",
                doc! {"input_tokens": 7_i64, "output_tokens": 1_i64},
            );
        }
    }
    rows
}
