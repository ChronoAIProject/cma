use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use layer2_core::binding_diagnostics::{BindingHold, HoldPhase, HoldReason};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tower::ServiceExt;

#[path = "tests_bounds.rs"]
mod bounds;

#[path = "tests_transport.rs"]
mod transport;

#[derive(Default)]
struct Members {
    values: Mutex<Vec<Member>>,
    owners: Mutex<BTreeMap<SandboxId, SubjectId>>,
    fail_owners: AtomicBool,
    truncated: AtomicBool,
}

#[async_trait::async_trait]
impl MembershipSource for Members {
    async fn list(&self) -> Result<MemberPage, BindingDiagnosticsError> {
        Ok(MemberPage {
            members: self.values.lock().unwrap().clone(),
            truncated: self.truncated.load(Ordering::Relaxed),
        })
    }
    async fn check(&self, sender: &str, recipient: &str) -> Result<(), BindingDiagnosticsError> {
        let values = self.values.lock().unwrap();
        if [sender, recipient].iter().all(|id| {
            values.iter().any(|member| {
                member.instance_id == *id && !member.draining && member.protocol == PROTOCOL_VERSION
            })
        }) {
            Ok(())
        } else {
            Err(BindingDiagnosticsError::Unavailable)
        }
    }
    async fn current_owners(
        &self,
        ids: &[SandboxId],
    ) -> Result<BTreeMap<SandboxId, SubjectId>, BindingDiagnosticsError> {
        if self.fail_owners.load(Ordering::Relaxed) {
            return Err(BindingDiagnosticsError::Unavailable);
        }
        let owners = self.owners.lock().unwrap();
        Ok(ids
            .iter()
            .filter_map(|id| owners.get(id).map(|owner| (id.clone(), owner.clone())))
            .collect())
    }
}

struct Local(Vec<BindingHold>);
impl LocalSource for Local {
    fn sample(&self) -> BindingHoldSample {
        BindingHoldSample {
            observed_at: layer2_core::now_rfc3339(),
            truncated: false,
            holds: self.0.clone(),
        }
    }
}

fn hold(id: &str) -> BindingHold {
    BindingHold {
        sandbox_id: SandboxId(id.into()),
        owner: SubjectId("owner".into()),
        reason: HoldReason::Unavailable,
        phase: HoldPhase::Deferred,
        first_held_at: layer2_core::now_rfc3339(),
        attempts: 1,
        retry_not_before: Some(layer2_core::now_rfc3339()),
        adapter_error_class: None,
    }
}

fn member(id: &str, address: &str) -> Member {
    Member {
        instance_id: id.into(),
        protocol: PROTOCOL_VERSION,
        relay_address: address.into(),
        expires_at_ms: i64::MAX,
        draining: false,
        owned_agents: 0,
        max_owned_agents: 16,
    }
}

fn service(id: &str, members: Arc<Members>, holds: Vec<BindingHold>) -> Arc<BindingDiagnostics> {
    BindingDiagnostics::assemble(
        id.into(),
        Box::new(|| true),
        members,
        Arc::new(Local(holds)),
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &[7; 32]),
    )
    .unwrap()
}

struct Server {
    address: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(router: Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server { address, task }
}

#[tokio::test]
async fn independent_http_reporters_are_authenticated_and_grouped_without_summing() {
    let members = Arc::new(Members::default());
    let a = service("a", members.clone(), vec![hold("shared")]);
    let b = service("b", members.clone(), vec![hold("shared"), hold("second")]);
    let server_a = server(a.router()).await;
    let server_b = server(b.router()).await;
    *members.values.lock().unwrap() = vec![
        member("a", &server_a.address),
        member("b", &server_b.address),
    ];
    *members.owners.lock().unwrap() = [hold("shared"), hold("second")]
        .into_iter()
        .map(|row| (row.sandbox_id, row.owner))
        .collect();
    for source in [a, b] {
        let view = source.observe(&SubjectId("operator".into())).await.unwrap();
        assert_eq!(view.observed_sandboxes, 2);
        assert_eq!(view.registered_reporters, 2);
        assert_eq!(view.unavailable_reporters, 0);
        assert_eq!(view.reports.len(), 2);
        assert!(!view.truncated);
        assert_eq!(
            view.reports
                .iter()
                .map(|r| r.sample.holds.len())
                .sum::<usize>(),
            3
        );
        assert!(view
            .reports
            .iter()
            .all(|r| r.reporter.starts_with("reporter-")));
    }
}

#[tokio::test]
async fn absent_peer_is_explicit_and_current_owner_filter_removes_stale_rows() {
    let members = Arc::new(Members::default());
    let a = service(
        "a",
        members.clone(),
        vec![hold("deleted"), hold("changed"), hold("current")],
    );
    let unavailable =
        server(Router::new().route(PATH, get(|| async { StatusCode::SERVICE_UNAVAILABLE }))).await;
    *members.values.lock().unwrap() = vec![
        member("a", "http://a.example.test/"),
        member("b", &unavailable.address),
    ];
    *members.owners.lock().unwrap() = [
        (SandboxId("changed".into()), SubjectId("other".into())),
        (SandboxId("current".into()), SubjectId("owner".into())),
    ]
    .into_iter()
    .collect();
    let view = a.observe(&SubjectId("operator".into())).await.unwrap();
    assert_eq!(view.registered_reporters, 2);
    assert_eq!(view.unavailable_reporters, 1);
    assert_eq!(view.observed_sandboxes, 1);
    assert!(view.truncated);
    assert_eq!(
        view.reports[0].sample.holds[0].sandbox_id.as_str(),
        "current"
    );
    members.fail_owners.store(true, Ordering::Relaxed);
    assert!(a.observe(&SubjectId("operator".into())).await.is_err());
}

#[tokio::test]
async fn peer_requires_proof_and_current_sender_and_rejects_replay_and_request_body() {
    let members = Arc::new(Members::default());
    *members.values.lock().unwrap() = vec![
        member("a", "http://a.example.test/"),
        member("b", "http://b.example.test/"),
    ];
    let a = service("a", members.clone(), vec![]);
    let b = service("b", members.clone(), vec![]);
    let plain = Request::builder().uri(PATH).body(Body::empty()).unwrap();
    assert_eq!(
        b.router().oneshot(plain).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let (_, proof) = a.authentication.request("a", "b", "operator").unwrap();
    let request = || {
        Request::builder()
            .uri(PATH)
            .header(authentication::PROOF_HEADER, &proof)
            .body(Body::empty())
            .unwrap()
    };
    let response = b.router().oneshot(request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert!(!to_bytes(response.into_body(), MAX_BODY)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        b.router().oneshot(request()).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let (_, proof) = a.authentication.request("a", "b", "operator").unwrap();
    let request = Request::builder()
        .uri(PATH)
        .header(authentication::PROOF_HEADER, proof)
        .body(Body::from("x"))
        .unwrap();
    assert_eq!(
        b.router().oneshot(request).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    members
        .values
        .lock()
        .unwrap()
        .retain(|member| member.instance_id != "a");
    let (_, proof) = a.authentication.request("a", "b", "operator").unwrap();
    let request = Request::builder()
        .uri(PATH)
        .header(authentication::PROOF_HEADER, proof)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        b.router().oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn transport_refuses_unsigned_and_oversize_replies_and_does_not_follow_redirects() {
    for response in [0, 1, 2] {
        let upstream = server(Router::new().route(
            PATH,
            get(move || async move {
                match response {
                    0 => (StatusCode::OK, "unsigned").into_response(),
                    1 => (
                        StatusCode::OK,
                        [(authentication::PROOF_HEADER, "AA")],
                        vec![b'x'; MAX_BODY + 1],
                    )
                        .into_response(),
                    _ => (
                        StatusCode::FOUND,
                        [("location", "http://unreachable.example.test/")],
                    )
                        .into_response(),
                }
            }),
        ))
        .await;
        let members = Arc::new(Members::default());
        let a = service("a", members.clone(), vec![]);
        *members.values.lock().unwrap() = vec![
            member("a", "http://a.example.test/"),
            member("b", &upstream.address),
        ];
        let view = a.observe(&SubjectId("operator".into())).await.unwrap();
        assert_eq!(view.unavailable_reporters, 1);
        assert_eq!(view.reports.len(), 1);
    }
}

#[tokio::test]
async fn limits_and_draining_members_do_not_become_complete_empty_success() {
    let members = Arc::new(Members::default());
    let a = service("a", members.clone(), vec![]);
    let mut draining = member("b", "http://b.example.test/");
    draining.draining = true;
    *members.values.lock().unwrap() = vec![member("a", "http://a.example.test/"), draining];
    members.truncated.store(true, Ordering::Relaxed);
    let view = a.observe(&SubjectId("operator".into())).await.unwrap();
    assert!(view.truncated);
    assert_eq!(view.unavailable_reporters, 1);
    let _one = a.slots.try_acquire().unwrap();
    let _two = a.slots.try_acquire().unwrap();
    assert!(a.observe(&SubjectId("operator".into())).await.is_err());
    drop((_one, _two));
    *members.values.lock().unwrap() = (0..=MAX_REPORTERS)
        .map(|i| member(&format!("m{i}"), "http://a.example.test/"))
        .collect();
    assert!(a.observe(&SubjectId("operator".into())).await.is_err());
}

#[test]
fn malformed_oversize_duplicate_and_stale_samples_are_refused() {
    let mut sample = Local(vec![hold("one")]).sample();
    valid_sample(&sample).unwrap();
    sample.holds.push(hold("one"));
    assert!(valid_sample(&sample).is_err());
    sample.holds = (0..=MAX_HOLDS_PER_REPORTER)
        .map(|i| hold(&format!("s{i}")))
        .collect();
    assert!(valid_sample(&sample).is_err());
    sample.holds = vec![hold("one")];
    sample.observed_at = "2000-01-01T00:00:00Z".into();
    assert!(valid_sample(&sample).is_err());
    sample.observed_at = layer2_core::now_rfc3339();
    sample.holds[0].attempts = 0;
    assert!(valid_sample(&sample).is_err());
}

#[test]
fn phase_and_retry_deadline_must_agree() {
    let mut sample = Local(vec![hold("one")]).sample();
    valid_sample(&sample).unwrap();
    sample.holds[0].retry_not_before = None;
    assert!(valid_sample(&sample).is_err());
    sample.holds[0].phase = HoldPhase::Retrying;
    valid_sample(&sample).unwrap();
    sample.holds[0].retry_not_before = Some(layer2_core::now_rfc3339());
    assert!(valid_sample(&sample).is_err());
}
