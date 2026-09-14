use std::sync::atomic::{AtomicBool, Ordering};

use layer2_core::profiles::{EnvironmentVariable, PublicValueScanError, PublicValueScanner};

use super::*;

#[derive(Debug, Default)]
struct SwitchScanner(AtomicBool);

#[async_trait]
impl PublicValueScanner for SwitchScanner {
    async fn scan(&self, _: &[EnvironmentVariable]) -> Result<(), PublicValueScanError> {
        if self.0.load(Ordering::SeqCst) {
            Err(PublicValueScanError::Unavailable)
        } else {
            Ok(())
        }
    }
}

async fn private_revision(router: &Router) -> String {
    let (status, profile) = mutate(
        router,
        Method::POST,
        "/api/v1/sandbox-profiles",
        Some(sandbox_body(
            "Public-values",
            None,
            vec![],
            Some("plain-configuration"),
        )),
        "owner",
        "values-create",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = profile["id"].as_str().unwrap().to_owned();
    let (status, _) = mutate(
        router,
        Method::POST,
        &format!("/api/v1/sandbox-profiles/{id}/revisions"),
        Some(json!({ "draft_version": 1 })),
        "owner",
        "values-private-revision",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    id
}

#[tokio::test]
async fn scanner_unavailability_has_a_typed_refusal_and_never_makes_a_private_profile_public() {
    let scanner = Arc::new(SwitchScanner(AtomicBool::new(true)));
    let (router, _) = fixture_with_scanner(true, scanner);
    let id = private_revision(&router).await;
    let (status, problem) = set_visibility(
        &router,
        "owner",
        "sandbox-profiles",
        &id,
        "public",
        "values-public-refused",
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(problem["code"], "profile_secret_scanner_unavailable");
    assert!(!problem.to_string().contains("plain-configuration"));
    let (status, profile) = json_response(
        &router,
        request(
            Method::GET,
            &format!("/api/v1/sandbox-profiles/{id}"),
            None,
            Some(("owner", true)),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(profile["visibility"], "private");
}

#[tokio::test]
async fn scanner_unavailability_never_creates_another_public_revision() {
    let scanner = Arc::new(SwitchScanner::default());
    let (router, _) = fixture_with_scanner(true, scanner.clone());
    let id = private_revision(&router).await;
    assert_eq!(
        set_visibility(
            &router,
            "owner",
            "sandbox-profiles",
            &id,
            "public",
            "values-public"
        )
        .await
        .0,
        StatusCode::OK
    );
    scanner.0.store(true, Ordering::SeqCst);
    let (status, problem) = mutate(
        &router,
        Method::POST,
        &format!("/api/v1/sandbox-profiles/{id}/revisions"),
        Some(json!({ "draft_version": 1 })),
        "owner",
        "values-public-revision-refused",
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(problem["code"], "profile_secret_scanner_unavailable");
    let (status, revisions) = json_response(
        &router,
        request(
            Method::GET,
            &format!("/api/v1/sandbox-profiles/{id}/revisions"),
            None,
            Some(("owner", true)),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revisions["items"].as_array().unwrap().len(), 1);
}
