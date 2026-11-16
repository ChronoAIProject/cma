//! Make the service timer win without relying on adapter/network scheduling.
use std::{
    future::{pending, poll_fn, Future},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    task::Poll,
    time::Duration,
};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use layer1_public::{AuthenticatedSubject, DelegatedCredential};
use layer2_core::{
    resource_access::{
        organization_diagnostics::{Failure, Operation, Reason, Stage},
        CallerContext, CredentialContext, CredentialSource, MembershipError, OrganizationAuthority,
        OrganizationChoice, ResourceAccessService, ResourceActor,
    },
    sandbox::store::InMemorySandboxStore,
    Authorization, Caller, Capability, SubjectId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

// Share the native route tests' subscriber and sensitive-ancestor assertions.
use super::capture;

struct Authority {
    previous_failure: bool,
    complete_after: Option<Duration>,
    calls: AtomicUsize,
    dropped: AtomicBool,
}

struct DropSignal<'a>(&'a AtomicBool);

impl Drop for DropSignal<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("list route must not request exact membership")
    }

    async fn organization_choices(
        &self,
        _: &Caller,
        credential: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        assert_eq!(credential, "fixture-credential");
        self.calls.fetch_add(1, Ordering::SeqCst);
        let _drop = DropSignal(&self.dropped);
        if self.previous_failure {
            Failure {
                operation: Operation::Choices,
                stage: Stage::Status,
                reason: Reason::HttpStatus,
                status: Some(503),
            }
            .record();
        }
        if let Some(delay) = self.complete_after {
            tokio::time::sleep(delay).await;
            Ok(Vec::new())
        } else {
            pending().await
        }
    }
}

fn application(
    previous_failure: bool,
    complete_after: Option<Duration>,
) -> (Router, Arc<Authority>) {
    let authority = Arc::new(Authority {
        previous_failure,
        complete_after,
        calls: AtomicUsize::new(0),
        dropped: AtomicBool::new(false),
    });
    let service =
        ResourceAccessService::new(Arc::new(InMemorySandboxStore::default()), authority.clone());
    (
        layer1_public::rest::resource_access_router(Arc::new(service)),
        authority,
    )
}

async fn request(app: Router) -> (StatusCode, Value) {
    let subject = SubjectId::new("human");
    let context = CallerContext {
        credential_source: CredentialSource::Retained,
        caller: Caller::new(
            subject.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: subject.clone(),
            credential_id: "fixture-credential-id".into(),
        },
        actor: ResourceActor::Api {
            subject: subject.clone(),
            delegation_id: None,
        },
        memberships: Vec::new(),
    };
    // These are the extensions produced by successful authentication. The
    // existing socket tests independently cover that middleware boundary.
    let request = Request::get("/api/v1/organizations")
        .extension(AuthenticatedSubject(subject))
        .extension(context.caller.authorization.clone())
        .extension(context)
        .extension(DelegatedCredential::new("fixture-credential".into()))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn assert_outer_timeout(previous_failure: bool) {
    let (app, authority) = application(previous_failure, None);
    let started = tokio::time::Instant::now();
    let ((status, body), events) = capture::run(request(app)).await;
    assert_eq!(started.elapsed(), Duration::from_secs(15));
    assert_eq!(authority.calls.load(Ordering::SeqCst), 1);
    assert!(authority.dropped.load(Ordering::SeqCst));
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "organization_authority_unavailable");
    assert_eq!(body["status"], 503);
    let detail = body["detail"].as_str().unwrap();
    assert!(detail.contains("operation=choices; stage=deadline; reason=timeout"));
    for excluded in [
        "upstream_status",
        "http_status",
        "human",
        "fixture-credential",
    ] {
        assert!(
            !detail.contains(excluded),
            "stale or private failure detail"
        );
    }
    assert_eq!(events.len(), 1, "only the service completed a failure");
    let fields = &events[0]["fields"];
    assert_eq!(fields["event"], "organization_choices_failure");
    assert_eq!(fields["dependency_stage"], "deadline");
    assert_eq!(fields["dependency_reason"], "timeout");
    assert!(fields["upstream_status"].is_null());
}

#[tokio::test(start_paused = true)]
async fn outer_service_deadline_records_timeout_without_adapter_diagnostics() {
    assert_outer_timeout(false).await;
}

#[tokio::test(start_paused = true)]
async fn outer_service_deadline_replaces_an_earlier_attempt_status() {
    assert_outer_timeout(true).await;
}

#[tokio::test(start_paused = true)]
async fn success_before_service_deadline_ignores_an_earlier_attempt_failure() {
    let (app, authority) = application(true, Some(Duration::from_secs(1)));
    let ((status, body), events) = capture::run(request(app)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"items":[]}));
    assert!(events.is_empty());
    assert_eq!(authority.calls.load(Ordering::SeqCst), 1);
    assert!(authority.dropped.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn cancelling_before_service_deadline_does_not_report_a_completed_timeout() {
    for previous_failure in [false, true] {
        let (app, authority) = application(previous_failure, None);
        let (_, events) = capture::run(async {
            let mut request = Box::pin(request(app));
            poll_fn(|cx| {
                assert!(request.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            assert_eq!(authority.calls.load(Ordering::SeqCst), 1);
            assert!(!authority.dropped.load(Ordering::SeqCst));
            drop(request);
            assert!(authority.dropped.load(Ordering::SeqCst));
            tokio::time::advance(Duration::from_secs(20)).await;
        })
        .await;
        assert!(events.is_empty());
        assert_eq!(authority.calls.load(Ordering::SeqCst), 1);
    }
}
