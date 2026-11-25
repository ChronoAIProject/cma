use super::*;
use crate::agent_sandbox::pool::{
    fixture::prefix,
    read_fixture::{failure, Gate, Reads},
    runtime_tests::{client, request},
};
use crate::{agent_sandbox::AgentSandboxClient, provider::ProviderSandboxRequest, RuntimeHandle};
use std::{collections::BTreeMap, time::Duration};

struct Resume {
    fixture: Fixture,
    home: crate::HomeVolumeAttachment,
    handle: RuntimeHandle,
    previous: ProviderSandboxRequest,
    next: ProviderSandboxRequest,
    claim_path: String,
}
impl Resume {
    async fn new() -> Self {
        Self::with_policy(false).await
    }
    async fn with_policy(explicit: bool) -> Self {
        let mut fixture = Fixture::new().await;
        if explicit {
            fixture.client.config.template_fs_group_change_policy =
                Some(crate::agent_sandbox::NativeFsGroupChangePolicy::OnRootMismatch);
        }
        let claim = assigned_spec(&fixture, &spec()).await;
        let home = adopted(&fixture, &claim).await;
        let previous = request(&home, 1);
        let handle = client(&fixture, false)
            .admit_pool_runtime(&claim, &home, &previous)
            .await
            .unwrap();
        client(&fixture, false)
            .suspend(&handle, &previous)
            .await
            .unwrap();
        let next = request(&home, 2);
        fixture.state.lock().unwrap().calls.clear();
        Self {
            fixture,
            home,
            handle,
            previous,
            next,
            claim_path: format!("{}/sandboxclaims/{}", prefix(), claim.name),
        }
    }
    async fn resume(&self) -> crate::Result<()> {
        client(&self.fixture, true)
            .resume_with_home(&self.handle, &self.previous, &self.next, &self.home)
            .await
    }
    fn reads(
        &self,
        overrides: BTreeMap<String, (u16, serde_json::Value)>,
        gates: BTreeMap<String, std::sync::Arc<Gate>>,
    ) -> (Reads, AgentSandboxClient) {
        let reads = Reads::new(&self.fixture, overrides, gates);
        let client = AgentSandboxClient::new(
            reads.client.clone(),
            self.fixture.client.config.clone(),
            "fixture",
        )
        .unwrap();
        (reads, client)
    }
    fn no_writes(&self) {
        assert!(self
            .fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|(call, _)| call.starts_with("GET ")));
    }
}

#[tokio::test]
async fn native_resume_proves_home_once_then_preserves_suspension_and_final_cas_reads() {
    let fixture = Resume::new().await;
    let before = fixture.fixture.state.lock().unwrap().rows[MEMBER].clone();
    fixture.resume().await.unwrap();
    let state = fixture.fixture.state.lock().unwrap();
    assert_eq!(
        state.calls.len(),
        8,
        "three graph reads, four transition reads, one CAS"
    );
    for (path, expected) in [
        (fixture.claim_path.as_str(), 1),
        (VOLUME, 1),
        (MEMBER, 4),
        ("/api/v1/namespaces/cma-test/pods", 1),
    ] {
        assert_eq!(
            state
                .calls
                .iter()
                .filter(|(c, _)| c == &format!("GET {path}"))
                .count(),
            expected,
            "{path}"
        );
    }
    assert_eq!(
        state.calls[6].0,
        format!("GET {MEMBER}"),
        "fresh final CAS authority"
    );
    let (call, patch) = &state.calls[7];
    assert_eq!(call, &format!("PATCH {MEMBER}"));
    assert_eq!(patch["metadata"]["uid"], before["metadata"]["uid"]);
    assert_eq!(
        patch["metadata"]["resourceVersion"],
        before["metadata"]["resourceVersion"]
    );
    assert_eq!(
        state.rows[MEMBER]["spec"]["podTemplate"],
        before["spec"]["podTemplate"]
    );
    assert_eq!(state.rows[VOLUME]["metadata"]["uid"], "pvc-uid");
    assert_eq!(
        state.rows[MEMBER]["metadata"]["annotations"]["cma/runtime-generation"],
        "2"
    );
}

#[tokio::test]
async fn native_resume_graph_reads_start_together_and_replay_reacquires_authority() {
    let fixture = Resume::new().await;
    fixture.resume().await.unwrap();
    for _ in 0..2 {
        let gates: BTreeMap<_, _> = [fixture.claim_path.clone(), MEMBER.into(), VOLUME.into()]
            .into_iter()
            .map(|p| (p, Gate::new()))
            .collect();
        let (reads, client) = fixture.reads(BTreeMap::new(), gates.clone());
        let (handle, previous, next, home) = (
            fixture.handle.clone(),
            fixture.previous.clone(),
            fixture.next.clone(),
            fixture.home.clone(),
        );
        let task = tokio::spawn(async move {
            client
                .resume_with_home(&handle, &previous, &next, &home)
                .await
        });
        for gate in gates.values() {
            gate.wait().await;
        }
        assert_eq!(reads.calls.lock().unwrap().len(), 3);
        assert!(!task.is_finished());
        for gate in gates.values() {
            gate.respond();
        }
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            reads.calls.lock().unwrap().len(),
            4,
            "fresh graph plus idempotency check"
        );
    }
}

#[tokio::test]
async fn native_resume_rejects_foreign_or_malformed_local_home_without_io() {
    let fixture = Resume::new().await;
    for change in [
        "scope",
        "origin",
        "claim",
        "sandbox",
        "volume",
        "handle",
        "generation",
    ] {
        let (reads, client) = fixture.reads(BTreeMap::new(), BTreeMap::new());
        let mut home = fixture.home.clone();
        let mut handle = fixture.handle.clone();
        let mut next = fixture.next.clone();
        match change {
            "scope" => {
                home.request.scope = Some(
                    crate::HomeVolumeScope::from_authority("native-vct-empty-v1", &["foreign"])
                        .unwrap(),
                );
                home.receipt
                    .native_authority
                    .as_mut()
                    .unwrap()
                    .claim
                    .request
                    .home = home.request.clone();
                home.receipt.request_sha256 = home.request.computed_sha256();
            }
            "origin" => home.receipt.native_authority = None,
            "claim" => {
                home.receipt.native_authority.as_mut().unwrap().claim.name = "foreign".into()
            }
            "sandbox" => {
                home.receipt.native_authority.as_mut().unwrap().sandbox_uid = "foreign".into()
            }
            "volume" => home.receipt.storage_generation = "foreign".into(),
            "handle" => handle = RuntimeHandle::new("agent-sandbox:foreign:member-uid"),
            "generation" => next = fixture.previous.clone(),
            _ => unreachable!(),
        }
        assert!(
            client
                .resume_with_home(&handle, &fixture.previous, &next, &home)
                .await
                .is_err(),
            "{change}"
        );
        assert!(reads.calls.lock().unwrap().is_empty(), "{change}");
    }
}

#[tokio::test]
async fn native_resume_rejects_stale_live_authority_before_transition() {
    for change in [
        "claim_uid",
        "claim_origin",
        "assignment",
        "member_uid",
        "member_origin",
        "member_owner",
        "member_spec",
        "pvc_uid",
        "pvc_owner",
        "pvc_spec",
    ] {
        let fixture = Resume::new().await;
        {
            let mut state = fixture.fixture.state.lock().unwrap();
            let (path, pointer, value) = match change {
                "claim_uid" => (
                    fixture.claim_path.as_str(),
                    "/metadata/uid",
                    json!("replacement"),
                ),
                "claim_origin" => (
                    fixture.claim_path.as_str(),
                    "/metadata/annotations/cma~1native-home-origin",
                    json!("{}"),
                ),
                "assignment" => (
                    fixture.claim_path.as_str(),
                    "/status/sandbox/name",
                    json!("replacement"),
                ),
                "member_uid" => (MEMBER, "/metadata/uid", json!("replacement")),
                "member_origin" => (
                    MEMBER,
                    "/metadata/annotations/cma~1native-home-origin",
                    json!("{}"),
                ),
                "member_owner" => (
                    MEMBER,
                    "/metadata/ownerReferences/0/uid",
                    json!("replacement"),
                ),
                "member_spec" => (
                    MEMBER,
                    "/spec/podTemplate/spec/containers/0/image",
                    json!("replacement"),
                ),
                "pvc_uid" => (VOLUME, "/metadata/uid", json!("replacement")),
                "pvc_owner" => (
                    VOLUME,
                    "/metadata/ownerReferences/0/uid",
                    json!("replacement"),
                ),
                "pvc_spec" => (VOLUME, "/spec/storageClassName", json!("replacement")),
                _ => unreachable!(),
            };
            *state
                .rows
                .get_mut(path)
                .unwrap()
                .pointer_mut(pointer)
                .unwrap() = value;
        }
        assert!(fixture.resume().await.is_err(), "{change}");
        fixture.no_writes();
    }
}

#[tokio::test]
async fn native_resume_lost_patch_and_conflict_replay_revalidate_original_home() {
    for lost in [true, false] {
        let fixture = Resume::new().await;
        {
            let mut state = fixture.fixture.state.lock().unwrap();
            if lost {
                &mut state.lose
            } else {
                &mut state.conflict
            }
            .push(format!("PATCH {MEMBER}"));
        }
        assert!(fixture.resume().await.is_err());
        fixture.fixture.state.lock().unwrap().calls.clear();
        fixture.resume().await.unwrap();
        {
            let mut state = fixture.fixture.state.lock().unwrap();
            assert_eq!(
                state
                    .calls
                    .iter()
                    .filter(|(c, _)| c.starts_with("PATCH "))
                    .count(),
                usize::from(!lost)
            );
            assert_eq!(
                state
                    .calls
                    .iter()
                    .filter(|(c, _)| c == &format!("GET {}", fixture.claim_path))
                    .count(),
                1
            );
            state.calls.clear();
            state.rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] =
                json!("replacement-after-replay");
        }
        assert!(fixture.resume().await.is_err());
        fixture.no_writes();
    }
}

#[tokio::test]
async fn native_resume_graph_failure_preserves_claim_before_faster_pvc_error() {
    let fixture = Resume::new().await;
    let claim_gate = Gate::new();
    let (reads, client) = fixture.reads(
        BTreeMap::from([
            (fixture.claim_path.clone(), failure(429)),
            (VOLUME.into(), failure(403)),
        ]),
        BTreeMap::from([(fixture.claim_path.clone(), claim_gate.clone())]),
    );
    let task = tokio::spawn(async move {
        client
            .resume_with_home(
                &fixture.handle,
                &fixture.previous,
                &fixture.next,
                &fixture.home,
            )
            .await
    });
    claim_gate.wait().await;
    assert!(!task.is_finished());
    claim_gate.respond();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap(),
        Err(crate::AdapterError::AtCapacity(_))
    ));
    assert_eq!(reads.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn native_resume_rejects_authority_changes_at_fresh_final_cas_boundary() {
    for change in ["uid", "version", "origin", "generation", "mode"] {
        let fixture = Resume::new().await;
        let rows = fixture.fixture.state.lock().unwrap().rows.clone();
        let member_reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let recorded = member_reads.clone();
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(
                request.method(),
                http::Method::GET,
                "stale authority cannot write"
            );
            let path = request.uri().path();
            let mut body = if path == "/api/v1/namespaces/cma-test/pods" {
                json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"1"},"items":[]})
            } else {
                rows.get(path).unwrap().clone()
            };
            if path == MEMBER && recorded.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 3 {
                match change {
                    "uid" => body["metadata"]["uid"] = json!("replacement"),
                    "version" => body["metadata"]["resourceVersion"] = json!(""),
                    "origin" => {
                        body["metadata"]["annotations"][NATIVE_HOME_ANNOTATION] = json!("{}")
                    }
                    "generation" => {
                        body["metadata"]["annotations"]["cma/runtime-generation"] = json!("3")
                    }
                    "mode" => body["spec"]["operatingMode"] = json!("Running"),
                    _ => unreachable!(),
                }
            }
            async move {
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(200)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        });
        let client = AgentSandboxClient::new(
            kube::Client::new(service, "cma-test"),
            fixture.fixture.client.config.clone(),
            "fixture",
        )
        .unwrap();
        assert!(
            client
                .resume_with_home(
                    &fixture.handle,
                    &fixture.previous,
                    &fixture.next,
                    &fixture.home
                )
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(
            member_reads.load(std::sync::atomic::Ordering::SeqCst),
            4,
            "{change}"
        );
    }
}

#[path = "native_upgrade_tests.rs"]
mod upgrade_tests;

#[path = "native_retained_policy_upgrade_tests.rs"]
mod retained_policy_upgrade_tests;

#[path = "native_resume_policy_read_tests.rs"]
mod policy_tests;
