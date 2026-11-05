use super::{
    fixture::Fixture,
    home_tests::{adopted, assigned_spec, MEMBER, VOLUME},
    *,
};
use crate::{HomeVolumeMode, HomeVolumeStore};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

type Rows = Arc<Mutex<BTreeMap<String, Value>>>;
struct Reads {
    store: NativeHomeVolumes,
    calls: Arc<Mutex<Vec<String>>>,
    rows: Rows,
    slow: Arc<Mutex<Option<String>>>,
}

impl Reads {
    fn new(fixture: &Fixture, claim: &NativePoolClaim) -> Self {
        let rows = Arc::new(Mutex::new(fixture.state.lock().unwrap().rows.clone()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let slow = Arc::new(Mutex::new(None::<String>));
        let (responses, recorded, stall) = (rows.clone(), calls.clone(), slow.clone());
        let claim_path = format!("{}/sandboxclaims/{}", fixture::prefix(), claim.name);
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(request.method(), http::Method::GET);
            let path = request.uri().path().to_owned();
            recorded.lock().unwrap().push(path.clone());
            let body = if path.ends_with("/persistentvolumeclaims") {
                json!({"apiVersion":"v1","kind":"PersistentVolumeClaimList","metadata":{},"items":[]})
            } else if path.ends_with("/sandboxclaims") {
                json!({"apiVersion":API_VERSION,"kind":"SandboxClaimList", "metadata":{"resourceVersion":"500"},
                    "items":[responses.lock().unwrap()[&claim_path].clone()]})
            } else {
                responses
                    .lock()
                    .unwrap()
                    .get(&path)
                    .expect("known child request")
                    .clone()
            };
            let delay = {
                let mut stall = stall.lock().unwrap();
                if stall.as_ref() == Some(&path) {
                    stall.take();
                    Duration::from_secs(10)
                } else {
                    Duration::from_secs(2)
                }
            };
            async move {
                tokio::time::sleep(delay).await;
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        });
        let client = kube::Client::new(service, "cma-test");
        Self {
            store: NativeHomeVolumes::new(client, fixture.client.config.clone(), "fixture")
                .unwrap(),
            calls,
            rows,
            slow,
        }
    }
}

async fn setup() -> (Fixture, NativePoolClaim, crate::HomeVolumeAttachment) {
    let fixture = Fixture::new().await;
    let mut spec = tests::spec();
    spec.home.mode = HomeVolumeMode::NativeVctEmptyV1;
    spec.home.template_name.clear();
    spec.home.template_generation.clear();
    let claim = assigned_spec(&fixture, &spec).await;
    let home = adopted(&fixture, &claim).await;
    (fixture, claim, home)
}

#[tokio::test]
async fn inventory_retries_only_the_slow_attachment_and_preserves_complete_prefix() {
    let (fixture, claim, home) = setup().await;
    let reads = Reads::new(&fixture, &claim);
    *reads.slow.lock().unwrap() = Some(VOLUME.into());
    tokio::time::pause();
    let mut cursor = reads.store.inventory_cursor().unwrap();
    let started = tokio::time::Instant::now();
    // Legacy list, claim list, claim reread: each fits five seconds, together do not.
    for _ in 0..3 {
        assert!(cursor.advance().await.unwrap().is_none());
    }
    assert!(started.elapsed() >= Duration::from_secs(6));
    assert!(cursor.advance().await.is_err());
    assert!(cursor.advance().await.unwrap().is_none());
    assert_eq!(cursor.advance().await.unwrap(), Some(vec![home]));
    let calls = reads.calls.lock().unwrap();
    assert_eq!(calls.len(), 6);
    assert_eq!(calls.iter().filter(|path| *path == MEMBER).count(), 1);
    assert_eq!(calls.iter().filter(|path| *path == VOLUME).count(), 2);
    assert_eq!(
        calls
            .iter()
            .filter(|path| path.ends_with(&claim.name))
            .count(),
        1
    );
}

#[tokio::test]
async fn outer_cancellation_retains_successful_parallel_child() {
    let (fixture, claim, home) = setup().await;
    let reads = Reads::new(&fixture, &claim);
    *reads.slow.lock().unwrap() = Some(VOLUME.into());
    tokio::time::pause();
    let mut cursor = reads.store.inventory_cursor().unwrap();
    for _ in 0..3 {
        assert!(cursor.advance().await.unwrap().is_none());
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(3), cursor.advance())
            .await
            .is_err()
    );
    assert!(cursor.advance().await.unwrap().is_none());
    assert_eq!(cursor.advance().await.unwrap(), Some(vec![home]));
    assert_eq!(
        reads
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|path| *path == MEMBER)
            .count(),
        1
    );
    assert_eq!(
        reads
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|path| *path == VOLUME)
            .count(),
        2
    );
}

#[tokio::test]
async fn rejected_attachment_requires_fresh_item_validation_before_completion() {
    let (fixture, claim, home) = setup().await;
    let reads = Reads::new(&fixture, &claim);
    let original = reads.rows.lock().unwrap()[VOLUME].clone();
    reads.rows.lock().unwrap().get_mut(VOLUME).unwrap()["metadata"]["uid"] = json!("replaced");
    tokio::time::pause();
    let mut cursor = reads.store.inventory_cursor().unwrap();
    for _ in 0..3 {
        assert!(cursor.advance().await.unwrap().is_none());
    }
    assert!(cursor.advance().await.is_err());
    reads.rows.lock().unwrap().insert(VOLUME.into(), original);
    assert!(cursor.advance().await.unwrap().is_none());
    assert!(cursor.advance().await.unwrap().is_none());
    assert_eq!(cursor.advance().await.unwrap(), Some(vec![home]));
    assert_eq!(
        reads
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.ends_with(&claim.name))
            .count(),
        2
    );
}

#[tokio::test]
async fn claim_inventory_retries_a_deadline_or_invalid_later_page_atomically() {
    let (fixture, claim, _) = setup().await;
    let row = fixture.row("sandboxclaims", &claim.name);
    let expected = fixture.client.observe_claim(&claim).await.unwrap();
    for deadline in [true, false] {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let row = row.clone();
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            let query = request.uri().query().unwrap_or_default().to_owned();
            let number = {
                let mut calls = recorded.lock().unwrap();
                calls.push(query);
                calls.len()
            };
            let body = match number {
                1 => {
                    json!({"apiVersion":API_VERSION,"kind":"SandboxClaimList","metadata":{"resourceVersion":"500","continue":"next"},"items":[row.clone()]})
                }
                2 if !deadline => {
                    json!({"apiVersion":API_VERSION,"kind":"SandboxClaimList","metadata":{"resourceVersion":"changed"},"items":[]})
                }
                _ => {
                    json!({"apiVersion":API_VERSION,"kind":"SandboxClaimList","metadata":{"resourceVersion":"500"},"items":[]})
                }
            };
            async move {
                if number == 2 && deadline {
                    std::future::pending::<()>().await;
                }
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        });
        let pools = AgentSandboxPools::new(
            kube::Client::new(service, "cma-test"),
            fixture.client.config.clone(),
        );
        tokio::time::pause();
        let mut cursor = pools.claim_inventory_cursor();
        assert!(cursor.advance().await.unwrap().is_none());
        assert!(cursor.advance().await.is_err());
        assert_eq!(
            cursor.advance().await.unwrap(),
            Some(vec![expected.clone()])
        );
        tokio::time::resume();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(!calls[0].contains("continue="));
        assert!(calls[1].contains("continue=next"));
        assert_eq!(calls[1], calls[2]);
    }
}
