//! Route-level evidence for delegated Image Manager authority on inline Triggers.
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use layer2_core::{
    trigger_domain::{TriggerInlineTargetMaterializer, TriggerReferenceError, TriggerTarget},
    SubjectId,
};
use serde_json::{json, Value};

use super::support_tests::*;
use crate::DelegatedCredential;

const CREDENTIAL: &str = "fixture-delegated-image-authority";
const FIELD: &str = "target.inline.image";

#[derive(Default)]
pub struct CredentialMaterializer {
    owners: Mutex<Vec<SubjectId>>,
}

#[async_trait]
impl TriggerInlineTargetMaterializer for CredentialMaterializer {
    async fn materialize(
        &self,
        _owner: &SubjectId,
        _target: &TriggerTarget,
    ) -> Result<TriggerTarget, TriggerReferenceError> {
        panic!("inline routes must use the credential-aware materializer")
    }

    async fn materialize_with_credential(
        &self,
        owner: &SubjectId,
        target: &TriggerTarget,
        credential: Option<&str>,
    ) -> Result<TriggerTarget, TriggerReferenceError> {
        if target.inline.is_none() {
            return Ok(target.clone());
        }
        self.owners.lock().unwrap().push(owner.clone());
        match credential {
            Some(CREDENTIAL) => Ok(target.clone()),
            Some(_) => Err(TriggerReferenceError::ImageAccessDenied { field: FIELD }),
            None => Err(TriggerReferenceError::ImageSignInRequired { field: FIELD }),
        }
    }
}

fn inline_body() -> Value {
    let mut body = trigger_body("always_new");
    body["content"]["target"]["inline"] = json!({
        "image": {"id": "private-codex", "version": "v1",
            "reference": format!("registry.example/runtime@sha256:{}", "a".repeat(64))},
        "agent": {"model": "gpt-5.6-sol", "reasoning_effort": "medium",
            "permission_mode": "guarded", "skills": [], "system_prompt": "",
            "services": []}
    });
    body
}

async fn write(
    app: &TestApp,
    method: Method,
    path: &str,
    body: Value,
    credential: Option<&str>,
    key: &str,
) -> (StatusCode, Value) {
    let mut request = publisher_request(method, path, Some(body), Some(key));
    if let Some(credential) = credential {
        request
            .extensions_mut()
            .insert(DelegatedCredential::new(credential.into()));
    }
    let response = json_response(&app.router, request).await;
    assert!(!response.1.to_string().contains(CREDENTIAL));
    response
}

#[tokio::test]
async fn create_requires_and_forwards_image_authority_without_leaking_the_credential() {
    let materializer = Arc::new(CredentialMaterializer::default());
    let app = TestApp::with_inline_materializer(materializer.clone()).await;
    for (credential, status, code, key) in [
        (
            None,
            StatusCode::UNAUTHORIZED,
            "trigger_image_sign_in_required",
            "missing",
        ),
        (
            Some("fixture-denied"),
            StatusCode::FORBIDDEN,
            "trigger_image_access_denied",
            "denied",
        ),
    ] {
        let (actual, problem) = write(
            &app,
            Method::POST,
            "/api/v1/triggers",
            inline_body(),
            credential,
            key,
        )
        .await;
        assert_eq!(actual, status, "{problem}");
        assert_eq!(problem["code"], code);
        assert_eq!(problem["field"], FIELD);
    }
    let (_, empty) = json_response(
        &app.router,
        publisher_request(Method::GET, "/api/v1/triggers", None, None),
    )
    .await;
    assert_eq!(empty["items"], json!([]));
    let (status, created) = write(
        &app,
        Method::POST,
        "/api/v1/triggers",
        inline_body(),
        Some(CREDENTIAL),
        "allowed",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        created["draft"]["target"]["inline"]["image"]["id"],
        "private-codex"
    );
    assert_eq!(
        materializer.owners.lock().unwrap().as_slice(),
        vec![SubjectId::new(OWNER); 3]
    );
}

#[tokio::test]
async fn update_conceals_foreign_owners_and_preserves_the_draft_when_image_authority_fails() {
    let materializer = Arc::new(CredentialMaterializer::default());
    let app = TestApp::with_inline_materializer(materializer.clone()).await;
    let created = create_trigger(&app, "always_new", "legacy-draft").await;
    let path = format!("/api/v1/triggers/{}", created["id"].as_str().unwrap());
    let body = json!({"draft_version": 1, "content": inline_body()["content"]});
    let (status, _) = json_response(
        &app.router,
        request(
            Method::PUT,
            &path,
            Some(body.clone()),
            Some((OTHER, publisher_authority())),
            Some("foreign"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(materializer.owners.lock().unwrap().is_empty());
    for (credential, status, code, key) in [
        (
            None,
            StatusCode::UNAUTHORIZED,
            "trigger_image_sign_in_required",
            "missing",
        ),
        (
            Some("fixture-denied"),
            StatusCode::FORBIDDEN,
            "trigger_image_access_denied",
            "denied",
        ),
    ] {
        let (actual, problem) =
            write(&app, Method::PUT, &path, body.clone(), credential, key).await;
        assert_eq!(actual, status, "{problem}");
        assert_eq!(problem["code"], code);
        assert_eq!(problem["field"], FIELD);
        let (_, unchanged) = json_response(
            &app.router,
            publisher_request(Method::GET, &path, None, None),
        )
        .await;
        assert_eq!(unchanged, created);
    }
    let (status, updated) =
        write(&app, Method::PUT, &path, body, Some(CREDENTIAL), "allowed").await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["draft_version"], 2);
    assert_eq!(
        materializer.owners.lock().unwrap().as_slice(),
        vec![SubjectId::new(OWNER); 3]
    );
}

fn pinned_repository_body() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/trigger-inline/pinned-request.json"
    ))
    .unwrap()
}

#[test]
fn pinned_repository_wire_contract_uses_ref_and_rejects_unknown_fields() {
    use crate::rest::trigger_openapi_shapes::CreateTriggerRequest;
    let body = pinned_repository_body();
    let parsed: CreateTriggerRequest = serde_json::from_value(body.clone()).unwrap();
    let serialized = serde_json::to_value(parsed).unwrap();
    assert_eq!(
        serialized["content"]["target"]["inline"]["workspace"],
        body["content"]["target"]["inline"]["workspace"]
    );
    for field in ["reference", "unexpected"] {
        let mut invalid = body.clone();
        invalid["content"]["target"]["inline"]["workspace"]["repository"][field] =
            json!("a".repeat(40));
        assert!(serde_json::from_value::<CreateTriggerRequest>(invalid).is_err());
    }
}

#[tokio::test]
async fn pinned_repository_body_is_accepted_for_owner_and_refused_for_visitor() {
    let materializer = Arc::new(CredentialMaterializer::default());
    let app = TestApp::with_inline_materializer(materializer.clone()).await;
    let body = pinned_repository_body();
    let (status, created) = write(
        &app,
        Method::POST,
        "/api/v1/triggers",
        body.clone(),
        Some(CREDENTIAL),
        "pinned-owner",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        created["draft"]["target"]["inline"]["workspace"],
        body["content"]["target"]["inline"]["workspace"]
    );
    let (status, _) = json_response(
        &app.router,
        request(
            Method::POST,
            "/api/v1/triggers",
            Some(body),
            Some((OTHER, visitor_authority())),
            Some("pinned-visitor"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(materializer.owners.lock().unwrap().len(), 1);
}
