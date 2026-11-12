//! Route-level evidence for refusing custom image selection before admission.
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
        let _ = credential;
        if target
            .inline
            .as_ref()
            .is_some_and(|inline| inline.image.is_some())
        {
            Err(TriggerReferenceError::ImageSelectionUnsupported { field: FIELD })
        } else {
            Ok(target.clone())
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
async fn create_rejects_custom_image_selection_without_persisting_any_operation() {
    let materializer = Arc::new(CredentialMaterializer::default());
    let app = TestApp::with_inline_materializer(materializer.clone()).await;
    for (credential, key) in [
        (None, "missing"),
        (Some("fixture-denied"), "denied"),
        (Some(CREDENTIAL), "authorized"),
    ] {
        let (status, problem) = write(
            &app,
            Method::POST,
            "/api/v1/triggers",
            inline_body(),
            credential,
            key,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
        assert_eq!(problem["code"], "trigger_image_selection_unsupported");
        assert_eq!(problem["field"], FIELD);
    }
    assert_eq!(app.operation_reservations(), 0);
    assert!(materializer.owners.lock().unwrap().is_empty());
    let (_, empty) = json_response(
        &app.router,
        publisher_request(Method::GET, "/api/v1/triggers", None, None),
    )
    .await;
    assert_eq!(empty["items"], json!([]));
    let mut body = inline_body();
    body["content"]["target"]["inline"]
        .as_object_mut()
        .unwrap()
        .remove("image");
    let (status, created) = write(
        &app,
        Method::POST,
        "/api/v1/triggers",
        body,
        Some(CREDENTIAL),
        "default-image",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        materializer.owners.lock().unwrap().as_slice(),
        vec![SubjectId::new(OWNER)]
    );
}

#[tokio::test]
async fn update_rejects_custom_image_selection_without_persisting_or_changing_the_draft() {
    let materializer = Arc::new(CredentialMaterializer::default());
    let app = TestApp::with_inline_materializer(materializer.clone()).await;
    let created = create_trigger(&app, "always_new", "legacy-draft").await;
    let reservations = app.operation_reservations();
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
    for (credential, key) in [
        (None, "missing"),
        (Some("fixture-denied"), "denied"),
        (Some(CREDENTIAL), "authorized"),
    ] {
        let (status, problem) =
            write(&app, Method::PUT, &path, body.clone(), credential, key).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
        assert_eq!(problem["code"], "trigger_image_selection_unsupported");
        assert_eq!(problem["field"], FIELD);
        let (_, unchanged) = json_response(
            &app.router,
            publisher_request(Method::GET, &path, None, None),
        )
        .await;
        assert_eq!(unchanged, created);
    }
    assert_eq!(app.operation_reservations(), reservations);
    assert!(materializer.owners.lock().unwrap().is_empty());
    let mut body = body;
    body["content"]["target"]["inline"]
        .as_object_mut()
        .unwrap()
        .remove("image");
    let (status, updated) = write(
        &app,
        Method::PUT,
        &path,
        body,
        Some(CREDENTIAL),
        "default-image",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["draft_version"], 2);
}
