use super::support_tests::*;
use async_trait::async_trait;
use axum::http::{header, Method, StatusCode};
use layer2_core::sandbox::{SandboxCredentialPrerequisite, SandboxCredentialPrerequisiteError};
use layer2_core::trigger_domain::{LaunchAdmissionPolicy, LaunchPolicy, LaunchStoreError};
use layer2_core::SubjectId;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Default)]
struct Credentials(AtomicUsize);

#[async_trait]
impl SandboxCredentialPrerequisite for Credentials {
    async fn check(&self, _: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

async fn app(policy: LaunchAdmissionPolicy) -> (TestApp, String, Arc<Credentials>) {
    let credentials = Arc::new(Credentials::default());
    let app = TestApp::with_admission_policy(policy, credentials.clone()).await;
    let trigger = create_trigger(&app, "always_new", "visitor-create").await;
    let published = publish_trigger(&app, trigger["id"].as_str().unwrap(), "visitor-publish").await;
    let path = format!(
        "/api/v1/triggers/public/{}/launches",
        published["public_id"].as_str().unwrap()
    );
    (app, path, credentials)
}

#[tokio::test]
async fn early_allowance_failure_is_unavailable_and_default_off_skips_the_read() {
    for policy in [
        LaunchAdmissionPolicy::Baseline,
        LaunchAdmissionPolicy::VisitorAllowance,
    ] {
        let (app, path, credentials) = app(policy).await;
        app.launches.fail(
            "visitor_launches_admitted",
            LaunchStoreError::Unavailable("private quota read failure".into()),
        );
        let (status, body) = json_response(
            &app.router,
            visitor_request(
                Method::POST,
                &path,
                Some(launch_body(false)),
                Some("quota-read-failure"),
            ),
        )
        .await;
        if policy == LaunchAdmissionPolicy::Baseline {
            assert_eq!(status, StatusCode::CREATED, "{body}");
            assert_eq!(credentials.0.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
            assert_eq!(body["code"], "launch_service_unavailable");
            assert!(!body.to_string().contains("private quota read failure"));
            assert!(body.get("reset_at").is_none());
            assert_eq!(credentials.0.load(Ordering::SeqCst), 0);
            assert_eq!(app.broker.metadata_reads(), 0);
        }
    }
}

#[tokio::test]
async fn exhausted_allowance_is_typed_without_credentials_and_exact_replay_still_wins() {
    let (app, path, credentials) = app(LaunchAdmissionPolicy::VisitorAllowance).await;
    let original = raw_response(
        &app.router,
        visitor_request(
            Method::POST,
            &path,
            Some(launch_body(false)),
            Some("original-visitor"),
        ),
    )
    .await;
    assert_eq!(original.0, StatusCode::CREATED);
    app.policy.set(Ok(LaunchPolicy {
        enabled: true,
        orchestration_paused: false,
        visitor_launches_per_day: 1,
    }));
    let calls = credentials.0.load(Ordering::SeqCst);
    let metadata = app.broker.metadata_reads();
    let refused = raw_response(
        &app.router,
        visitor_request(
            Method::POST,
            &path,
            Some(launch_body(false)),
            Some("exhausted-visitor"),
        ),
    )
    .await;
    assert_eq!(refused.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(refused.1.contains_key(header::RETRY_AFTER));
    assert_eq!(refused.1[header::CONTENT_TYPE], "application/problem+json");
    let body: serde_json::Value = serde_json::from_slice(&refused.2).unwrap();
    assert_eq!(body["code"], "resource_quota");
    assert_eq!(body["resource"], "visitor_launches");
    assert_eq!(body["limit"], 1);
    assert!(body["reset_at"].is_string());
    assert_eq!(credentials.0.load(Ordering::SeqCst), calls);
    assert_eq!(app.broker.metadata_reads(), metadata);
    let replay = raw_response(
        &app.router,
        visitor_request(
            Method::POST,
            &path,
            Some(launch_body(false)),
            Some("original-visitor"),
        ),
    )
    .await;
    assert_eq!(replay, original);
    assert_eq!(credentials.0.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn advisory_zero_does_not_replace_authoritative_quota_refusal() {
    for policy in [
        LaunchAdmissionPolicy::Baseline,
        LaunchAdmissionPolicy::VisitorAllowance,
    ] {
        let (app, path, _) = app(policy).await;
        app.launches.fail(
            "admit_launch",
            LaunchStoreError::VisitorQuota {
                limit: 5,
                reset_at: std::time::SystemTime::now() + std::time::Duration::from_secs(60),
            },
        );
        let (status, body) = json_response(
            &app.router,
            visitor_request(
                Method::POST,
                &path,
                Some(launch_body(false)),
                Some("stale-allowance"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
        assert_eq!(body["code"], "resource_quota");
        assert_eq!(body["limit"], 5);
    }
}
