use super::*;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

struct Gate {
    started: Semaphore,
    release: Semaphore,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }

    async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.started.acquire())
            .await
            .expect("read did not start while the other response was held")
            .unwrap()
            .forget();
    }

    fn respond(&self) {
        self.release.add_permits(1);
    }
}

struct Reads {
    client: kube::Client,
    calls: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Reads {
    fn new(
        fixture: &Fixture,
        overrides: BTreeMap<String, (u16, serde_json::Value)>,
        gates: BTreeMap<String, Arc<Gate>>,
    ) -> Self {
        let mut replies: BTreeMap<_, _> = fixture
            .state
            .lock()
            .unwrap()
            .rows
            .iter()
            .map(|(path, row)| (path.clone(), (200, row.clone())))
            .collect();
        replies.extend(overrides);
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(request.method(), http::Method::GET);
            let path = request.uri().path().to_owned();
            recorded.lock().unwrap().push(path.clone());
            let (status, body) = replies
                .get(&path)
                .expect("unexpected resource read")
                .clone();
            let gate = gates.get(&path).cloned();
            async move {
                if let Some(gate) = gate {
                    gate.started.add_permits(1);
                    gate.release.acquire().await.unwrap().forget();
                }
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(status)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        });
        Self {
            client: kube::Client::new(service, "cma-test"),
            calls,
        }
    }

    fn pools(&self, fixture: &Fixture) -> AgentSandboxPools {
        AgentSandboxPools::new(self.client.clone(), fixture.client.config.clone())
    }
}

fn api_failure(code: u16, reason: &str) -> (u16, serde_json::Value) {
    (
        code,
        json!({"apiVersion":"v1","kind":"Status","status":"Failure",
            "reason":reason,"message":"fixture failure","code":code}),
    )
}

#[tokio::test]
async fn native_home_verification_reads_member_and_pvc_together_after_claim() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let claim_path = format!(
        "{}/sandboxclaims/{}",
        super::super::fixture::prefix(),
        claim.name
    );
    let claim_gate = Gate::new();
    let member_gate = Gate::new();
    let volume_gate = Gate::new();
    let reads = Reads::new(
        &fixture,
        BTreeMap::new(),
        BTreeMap::from([
            (claim_path.clone(), claim_gate.clone()),
            (MEMBER.into(), member_gate.clone()),
            (VOLUME.into(), volume_gate.clone()),
        ]),
    );
    let homes = NativeHomeVolumes::new(
        reads.client.clone(),
        fixture.client.config.clone(),
        "fixture",
    )
    .unwrap();
    let task = tokio::spawn(async move { homes.verify(&home).await });
    claim_gate.wait().await;
    assert_eq!(
        reads.calls.lock().unwrap().as_slice(),
        std::slice::from_ref(&claim_path)
    );
    claim_gate.respond();
    member_gate.wait().await;
    volume_gate.wait().await;
    assert!(!task.is_finished(), "both dependent responses are held");
    volume_gate.respond();
    member_gate.respond();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let mut calls = reads.calls.lock().unwrap().clone();
    assert_eq!(calls.remove(0), claim_path);
    calls.sort();
    let mut expected = vec![MEMBER.to_owned(), VOLUME.to_owned()];
    expected.sort();
    assert_eq!(calls, expected, "one read of each original resource");
}

#[tokio::test]
async fn native_home_recovery_preserves_member_result_before_faster_pvc_failure() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    adopted(&fixture, &claim).await;
    for change in [
        "absent",
        "deleting",
        "annotation",
        "owner",
        "uid",
        "spec",
        "api",
    ] {
        let mut member = fixture.state.lock().unwrap().rows[MEMBER].clone();
        match change {
            "deleting" => member["metadata"]["deletionTimestamp"] = json!("2026-10-14T00:00:00Z"),
            "annotation" => {
                member["metadata"]["annotations"]
                    .as_object_mut()
                    .unwrap()
                    .remove(NATIVE_HOME_ANNOTATION);
            }
            "owner" => member["metadata"]["ownerReferences"][0]["uid"] = json!("replacement"),
            "uid" => member["metadata"]["uid"] = json!("replacement"),
            "spec" => {
                member["spec"]["podTemplate"]["spec"]["containers"][0]["image"] = json!("other")
            }
            _ => {}
        }
        let member_reply = match change {
            "absent" => api_failure(404, "NotFound"),
            "api" => api_failure(429, "TooManyRequests"),
            _ => (200, member),
        };
        let member_gate = Gate::new();
        let volume_gate = Gate::new();
        let reads = Reads::new(
            &fixture,
            BTreeMap::from([
                (MEMBER.into(), member_reply),
                (VOLUME.into(), api_failure(403, "Forbidden")),
            ]),
            BTreeMap::from([
                (MEMBER.into(), member_gate.clone()),
                (VOLUME.into(), volume_gate.clone()),
            ]),
        );
        let pools = reads.pools(&fixture);
        let original = claim.clone();
        let mut task = tokio::spawn(async move { pools.recover_home(&original).await });
        member_gate.wait().await;
        volume_gate.wait().await;
        volume_gate.respond();
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut task)
                .await
                .is_err(),
            "{change}: PVC failure must wait for member validation"
        );
        member_gate.respond();
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        match change {
            "absent" | "deleting" | "annotation" => assert_eq!(result.unwrap(), None, "{change}"),
            "api" => assert!(matches!(result, Err(crate::AdapterError::AtCapacity(_)))),
            _ => {
                let crate::AdapterError::Rejected(detail) = result.unwrap_err() else {
                    panic!("{change}: member identity must reject before PVC API failure");
                };
                let expected = match change {
                    "owner" => "native pool member is not owned by the admitted claim",
                    "uid" => "native Sandbox UID changed",
                    _ => "native VCT ownership cannot be converted",
                };
                assert_eq!(detail, expected, "{change}");
            }
        }
    }
}

#[tokio::test]
async fn native_home_recovery_never_reads_dependents_after_invalid_claim() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    adopted(&fixture, &claim).await;
    fixture.edit("sandboxclaims", &claim.name, |row| {
        row["metadata"]["uid"] = json!("replacement")
    });
    let reads = Reads::new(&fixture, BTreeMap::new(), BTreeMap::new());
    assert!(matches!(reads.pools(&fixture).recover_home(&claim).await,
        Err(crate::AdapterError::Rejected(detail)) if detail == "native claim UID changed"));
    assert_eq!(
        *reads.calls.lock().unwrap(),
        [format!(
            "{}/sandboxclaims/{}",
            super::super::fixture::prefix(),
            claim.name
        )]
    );
}

#[tokio::test]
async fn native_home_recovery_returns_pvc_failure_after_valid_member() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    adopted(&fixture, &claim).await;
    let reads = Reads::new(
        &fixture,
        BTreeMap::from([(VOLUME.into(), api_failure(403, "Forbidden"))]),
        BTreeMap::new(),
    );
    assert!(matches!(reads.pools(&fixture).recover_home(&claim).await,
        Err(crate::AdapterError::Rejected(detail)) if detail == "Kubernetes rejected Sandbox operation (403)"));
}
