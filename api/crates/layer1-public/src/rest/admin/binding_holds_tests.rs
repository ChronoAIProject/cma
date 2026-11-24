use super::*;
use layer2_core::binding_diagnostics::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const PATH: &str = "/api/v1/admin/runtime-binding-holds";

struct Source {
    calls: AtomicUsize,
    unavailable: bool,
    hang: bool,
}

#[async_trait]
impl BindingHoldSource for Source {
    async fn observe(
        &self,
        principal: &SubjectId,
    ) -> Result<BindingHoldOverview, BindingDiagnosticsError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(principal.as_str(), "operator-a");
        if self.hang {
            std::future::pending::<()>().await;
        }
        if self.unavailable {
            return Err(BindingDiagnosticsError::Unavailable);
        }
        Ok(BindingHoldOverview {
            observed_at: NOW.into(),
            observed_sandboxes: 1,
            registered_reporters: 2,
            unavailable_reporters: 1,
            truncated: false,
            reports: vec![BindingHoldReport {
                reporter: "reporter-1".into(),
                sample: BindingHoldSample {
                    observed_at: NOW.into(),
                    truncated: false,
                    holds: vec![BindingHold {
                        sandbox_id: SandboxId("sandbox-a".into()),
                        owner: SubjectId("owner-a".into()),
                        reason: HoldReason::Unavailable,
                        phase: HoldPhase::Deferred,
                        first_held_at: NOW.into(),
                        attempts: 3,
                        retry_not_before: Some("2026-09-05T00:01:00Z".into()),
                        adapter_error_class: None,
                    }],
                },
            }],
        })
    }
}

fn observed(unavailable: bool, hang: bool) -> (Fixture, Arc<Source>) {
    let source = Arc::new(Source {
        calls: AtomicUsize::new(0),
        unavailable,
        hang,
    });
    let service = Arc::new(BindingDiagnosticsService::new(source.clone()));
    (fixture_with_options(false, None, Some(service)), source)
}

pub(in crate::rest) fn contract_scenario(
    name: &str,
) -> Option<crate::rest::public_contract_tests::Scenario> {
    use crate::rest::public_contract_tests::Scenario;
    use layer2_core::Authorization;

    let (fixture, subject, can_read) = match name.strip_prefix("binding_holds_")? {
        "operator" => (observed(false, false).0, Some("operator-a"), true),
        "forbidden" => (observed(false, false).0, Some("operator-a"), false),
        "anonymous" => (observed(false, false).0, None, false),
        "unavailable" => (observed(true, false).0, Some("operator-a"), true),
        "disabled" => (fixture(false), Some("operator-a"), true),
        _ => return None,
    };
    Some(Scenario {
        router: fixture.app,
        subject,
        authorization: Authorization::new(can_read.then_some(Capability::ReadAllSandboxes)),
        symbols: Default::default(),
    })
}

#[tokio::test]
async fn binding_observations_require_cross_owner_read_before_sampling() {
    let (f, source) = observed(false, false);
    for capabilities in [
        vec![],
        vec![Capability::UseOwnSandboxes],
        vec![Capability::ControlAnySandbox],
    ] {
        let response = f
            .app
            .clone()
            .oneshot(request(Method::GET, PATH, None, capabilities))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
    }
    let response = f
        .app
        .clone()
        .oneshot(Request::builder().uri(PATH).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.names.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn binding_observation_projection_preserves_partial_results_and_unknown_error_class() {
    let (f, source) = observed(false, false);
    let response = f
        .app
        .clone()
        .oneshot(request(
            Method::GET,
            PATH,
            None,
            [Capability::ReadAllSandboxes],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let value = response_json(response).await;
    assert_eq!(value["observed_sandboxes"], 1);
    assert_eq!(value["unavailable_reporters"], 1);
    assert_eq!(
        value["reports"][0]["sample"]["holds"][0]["reason"],
        "unavailable"
    );
    assert_eq!(
        value["reports"][0]["sample"]["holds"][0]["adapter_error_class"],
        Value::Null
    );
    assert_eq!(
        value["reports"][0]["sample"]["holds"][0]["owner_name"],
        "Alex Chen"
    );
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert!(f.activity.records.lock().unwrap().is_empty());
    let response = f
        .app
        .oneshot(request(
            Method::POST,
            PATH,
            Some(json!({"retry":true})),
            [Capability::ReadAllSandboxes],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn binding_observation_failures_and_deadline_remain_unavailable() {
    for (unavailable, hang) in [(true, false), (false, true)] {
        let (f, source) = observed(unavailable, hang);
        let response = f
            .app
            .oneshot(request(
                Method::GET,
                PATH,
                None,
                [Capability::ReadAllSandboxes],
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let value = response_json(response).await;
        assert_eq!(value["code"], "binding_observation_unavailable");
        assert!(value.get("reports").is_none());
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn disabled_binding_observation_route_is_absent() {
    let f = fixture(false);
    let response = f
        .app
        .oneshot(request(
            Method::GET,
            PATH,
            None,
            [Capability::ReadAllSandboxes],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unusable_owner_names_remain_unknown_without_failing_observations() {
    use layer2_core::{PublicIdentity, UserDirectoryStore};
    for name in ["owner-a".into(), "n".repeat(201), "bad\nname".into()] {
        let (f, _) = observed(false, false);
        f.names
            .observe(&PublicIdentity {
                subject: SubjectId("owner-a".into()),
                display_name: Some(name),
                avatar_url: None,
            })
            .await
            .unwrap();
        let response = f
            .app
            .oneshot(request(
                Method::GET,
                PATH,
                None,
                [Capability::ReadAllSandboxes],
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value = response_json(response).await;
        assert_eq!(value["observed_sandboxes"], 1);
        assert_eq!(
            value["reports"][0]["sample"]["holds"][0]["owner_name"],
            Value::Null
        );
    }
}
