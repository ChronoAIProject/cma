use super::*;

use std::collections::BTreeMap as CapturedFields;
use std::sync::{Arc as StdArc, Mutex as StdMutex};

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use tracing::field::{Field, Visit};
use tracing_subscriber::prelude::*;

const NOW: u64 = 1_000_000;
const SESSION: &str = "sesn_trg_123456781234123412341234567890ab";
const SESSION_B: &str = "sesn_trg_123456781234123412341234567890ac";
const RUNTIME: &str = "sandbox-runtime-1";
const RUNTIME_B: &str = "sandbox-runtime-2";
const SERVICE: &str = "service-1";
const CATALOG_SERVICE: &str = "catalog-service-github";
const USER_SERVICE_SLUG: &str = "api-github-2";
const BRANCH: &str = "cma/launch-1234";
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Signer(Ed25519KeyPair);

#[derive(Clone, Default)]
struct AuditCapture(StdArc<StdMutex<Vec<CapturedFields<String, String>>>>);

impl AuditCapture {
    fn take(&self) -> Vec<CapturedFields<String, String>> {
        std::mem::take(&mut *self.0.lock().expect("audit capture lock"))
    }
}

struct AuditVisitor<'a>(&'a mut CapturedFields<String, String>);

impl Visit for AuditVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

impl<S> tracing_subscriber::Layer<S> for AuditCapture
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut fields = CapturedFields::new();
        fields.insert("_target".into(), event.metadata().target().into());
        event.record(&mut AuditVisitor(&mut fields));
        if event.metadata().target() == "cma_trigger::github_capability_audit" {
            self.0.lock().expect("audit capture lock").push(fields);
        }
    }
}

impl Signer {
    fn new() -> Self {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("generate key");
        Self(Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("read generated key"))
    }

    fn keyring(&self) -> String {
        serde_json::json!({
            "key-1": base64::engine::general_purpose::STANDARD.encode(self.0.public_key().as_ref())
        })
        .to_string()
    }

    fn sign(&self, document: &CapabilityDocument) -> String {
        self.sign_payload(&serde_json::to_vec(document).expect("serialize document"))
    }

    fn sign_payload(&self, payload: &[u8]) -> String {
        let mut message = SIGNING_DOMAIN.to_vec();
        message.extend_from_slice(payload);
        serde_json::to_string(&SignedEnvelope {
            version: POLICY_VERSION,
            key_id: "key-1".into(),
            payload: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload),
            signature: base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(self.0.sign(&message).as_ref()),
        })
        .expect("serialize envelope")
    }
}

fn upstream() -> RepositoryBinding {
    RepositoryBinding {
        repository_id: 10,
        repository_node_id: "R_upstream".into(),
        owner_id: 11,
        owner_node_id: "U_upstream".into(),
        canonical_owner: "upstream-owner".into(),
        canonical_name: "project".into(),
        fork_parent_id: None,
        fork_source_id: None,
    }
}

fn fork() -> RepositoryBinding {
    RepositoryBinding {
        repository_id: 30,
        repository_node_id: "R_fork".into(),
        owner_id: 31,
        owner_node_id: "U_fork".into(),
        canonical_owner: "contributor".into(),
        canonical_name: "project".into(),
        fork_parent_id: Some(10),
        fork_source_id: Some(10),
    }
}

fn policy(generation: u64, fence: u64) -> CapabilityDocument {
    CapabilityDocument::Policy {
        version: POLICY_VERSION,
        launch_id: "12345678-1234-1234-1234-1234567890ab".into(),
        session_id: SESSION.into(),
        runtime_audience: RUNTIME.into(),
        service_id: SERVICE.into(),
        catalog_service_id: CATALOG_SERVICE.into(),
        user_service_slug: USER_SERVICE_SLUG.into(),
        upstream: Box::new(upstream()),
        fork: Box::new(fork()),
        allowed_ref: format!("refs/heads/{BRANCH}"),
        github_operation_classes: GithubOperationClass::ALL.to_vec(),
        generation,
        fence,
        not_before_epoch_seconds: NOW,
        expires_at_epoch_seconds: NOW + MAX_LIFETIME_SECONDS,
        issuer: "cma".into(),
        audience: POLICY_AUDIENCE.into(),
        signature_key_id: "key-1".into(),
        numeric_binding_observed_at: "2026-08-27T00:00:00Z".into(),
    }
}

fn tombstone(generation: u64, fence: u64) -> CapabilityDocument {
    CapabilityDocument::Tombstone {
        version: POLICY_VERSION,
        launch_id: "12345678-1234-1234-1234-1234567890ab".into(),
        session_id: SESSION.into(),
        runtime_audience: RUNTIME.into(),
        generation,
        fence,
        issued_at_epoch_seconds: NOW,
        issuer: "cma".into(),
        audience: POLICY_AUDIENCE.into(),
        signature_key_id: "key-1".into(),
    }
}

fn gate(signer: &Signer) -> GithubCapabilityGate {
    gate_for(signer, SESSION)
}

fn gate_for(signer: &Signer, session_id: &str) -> GithubCapabilityGate {
    GithubCapabilityGate::from_configuration(
        true,
        session_id,
        vec![
            "chrono-llm".into(),
            "ornn".into(),
            GITHUB_SERVICE_SLUG.into(),
            USER_SERVICE_SLUG.into(),
            "api-github-pat".into(),
            SERVICE.into(),
            CATALOG_SERVICE.into(),
            "unexpected-baseline".into(),
        ],
        &signer.keyring(),
    )
    .expect("valid gate")
}

fn github_uri(path: &str) -> Uri {
    format!("/api/v1/proxy/{CATALOG_SERVICE}{path}?_nyxid_via={SERVICE}")
        .parse()
        .expect("URI")
}

async fn installed_gate() -> (Signer, GithubCapabilityGate) {
    let signer = Signer::new();
    let gate = gate(&signer);
    gate.install(RUNTIME, &signer.sign(&policy(1, 1)), NOW)
        .await
        .expect("install policy");
    (signer, gate)
}

#[tokio::test]
async fn missing_policy_fails_closed_for_github_but_not_an_entitled_non_github_service() {
    let signer = Signer::new();
    let gate = gate(&signer);
    let denied = gate
        .evaluate(&Method::GET, &github_uri("/repositories/30"), &[], NOW)
        .await;
    assert_eq!(denied, Err(CapabilityRefusal::MissingPolicy));

    let passthrough: Uri = "/api/v1/proxy/s/ornn/skills?_nyxid_via=ornn-1"
        .parse()
        .unwrap();
    assert_eq!(
        gate.evaluate(&Method::GET, &passthrough, &[], NOW).await,
        Ok(None)
    );
    let unknown: Uri = "/api/v1/proxy/s/unexpected-baseline/data".parse().unwrap();
    assert_eq!(
        gate.evaluate(&Method::GET, &unknown, &[], NOW).await,
        Err(CapabilityRefusal::OperationDenied),
        "capability-required runtimes deny configured reaches outside the MVP baseline"
    );

    for slug in [
        GITHUB_SERVICE_SLUG,
        USER_SERVICE_SLUG,
        "api-github-pat",
        SERVICE,
        CATALOG_SERVICE,
    ] {
        let alternate: Uri = format!("/api/v1/proxy/s/{slug}/repositories/30?_nyxid_via={SERVICE}")
            .parse()
            .unwrap();
        assert_eq!(
            gate.evaluate(&Method::GET, &alternate, &[], NOW).await,
            Err(CapabilityRefusal::OperationDenied),
            "{slug} must never enter generic slug passthrough"
        );
    }
}

#[tokio::test]
async fn exact_numeric_repository_read_is_canonicalized_with_the_service_selector() {
    let (_, gate) = installed_gate().await;
    let request = gate
        .evaluate(&Method::GET, &github_uri("/repositories/30"), &[], NOW + 1)
        .await
        .expect("allowed")
        .expect("GitHub request");
    assert_eq!(request.operation, GithubOperationClass::RepositoryRead);
    assert_eq!(
        request.path_and_query,
        "/api/v1/proxy/catalog-service-github/repositories/30?_nyxid_via=service-1"
    );
    assert!(request.body.is_empty());
}

#[tokio::test]
async fn tree_reads_accept_only_the_fork_and_known_query_shape() {
    let (_, gate) = installed_gate().await;
    let uri: Uri = format!(
        "/api/v1/proxy/{CATALOG_SERVICE}/repos/contributor/project/git/trees/{SHA}?recursive=1&_nyxid_via={SERVICE}"
    )
    .parse()
    .unwrap();
    let request = gate
        .evaluate(&Method::GET, &uri, &[], NOW + 1)
        .await
        .expect("allowed")
        .unwrap();
    assert_eq!(request.operation, GithubOperationClass::TreeRead);
    assert_eq!(
        request.path_and_query,
        format!(
            "/api/v1/proxy/{CATALOG_SERVICE}/repos/contributor/project/git/trees/{SHA}?_nyxid_via={SERVICE}&recursive=1"
        )
    );

    let other = github_uri(&format!("/repos/someone-else/project/git/trees/{SHA}"));
    assert_eq!(
        gate.evaluate(&Method::GET, &other, &[], NOW + 1).await,
        Err(CapabilityRefusal::ResourceMismatch)
    );
    let extra: Uri = format!(
        "/api/v1/proxy/{CATALOG_SERVICE}/repos/contributor/project/git/trees/{SHA}?recursive=1&page=2&_nyxid_via={SERVICE}"
    )
    .parse()
    .unwrap();
    assert_eq!(
        gate.evaluate(&Method::GET, &extra, &[], NOW + 1).await,
        Err(CapabilityRefusal::MalformedRequest)
    );
}

#[tokio::test]
async fn object_creation_parsers_validate_and_canonicalize_each_git_data_family() {
    let (_, gate) = installed_gate().await;
    let cases = [
        (
            "/repos/contributor/project/git/blobs",
            serde_json::json!({ "content": "hello", "encoding": "utf8" }),
        ),
        (
            "/repos/contributor/project/git/trees",
            serde_json::json!({
                "base_tree": SHA,
                "tree": [{
                    "path": "src/lib.rs",
                    "mode": "100644",
                    "type": "blob",
                    "content": "pub fn ready() {}"
                }]
            }),
        ),
        (
            "/repos/contributor/project/git/commits",
            serde_json::json!({ "message": "CMA contribution", "tree": SHA, "parents": [SHA] }),
        ),
    ];

    for (path, value) in cases {
        let body = serde_json::to_vec(&value).unwrap();
        let request = gate
            .evaluate(&Method::POST, &github_uri(path), &body, NOW + 1)
            .await
            .expect("allowed")
            .expect("GitHub request");
        assert_eq!(request.operation, GithubOperationClass::ObjectCreate);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
            value
        );
    }

    let unknown_field = serde_json::json!({
        "content": "hello",
        "encoding": "utf8",
        "owner_supplied_authority": true
    })
    .to_string()
    .into_bytes();
    assert_eq!(
        gate.evaluate(
            &Method::POST,
            &github_uri("/repos/contributor/project/git/blobs"),
            &unknown_field,
            NOW + 1,
        )
        .await,
        Err(CapabilityRefusal::MalformedRequest)
    );

    let traversal = serde_json::json!({
        "base_tree": SHA,
        "tree": [{
            "path": "../outside",
            "mode": "100644",
            "type": "blob",
            "content": "escape"
        }]
    })
    .to_string()
    .into_bytes();
    assert_eq!(
        gate.evaluate(
            &Method::POST,
            &github_uri("/repos/contributor/project/git/trees"),
            &traversal,
            NOW + 1,
        )
        .await,
        Err(CapabilityRefusal::MalformedRequest)
    );
}

#[tokio::test]
async fn status_reads_are_bound_to_the_launch_branch_and_commit_shape() {
    let (_, gate) = installed_gate().await;
    for path in [
        format!("/repos/contributor/project/git/ref/heads/{BRANCH}"),
        format!("/repos/contributor/project/branches/{BRANCH}"),
        format!("/repos/contributor/project/commits/{SHA}/status"),
        format!("/repos/contributor/project/commits/{SHA}/check-runs"),
        format!("/repos/contributor/project/commits/{SHA}/check-suites"),
    ] {
        let request = gate
            .evaluate(&Method::GET, &github_uri(&path), &[], NOW + 1)
            .await
            .expect("allowed")
            .expect("GitHub request");
        assert_eq!(request.operation, GithubOperationClass::StatusRead);
    }

    assert_eq!(
        gate.evaluate(
            &Method::GET,
            &github_uri("/repos/contributor/project/branches/main"),
            &[],
            NOW + 1,
        )
        .await,
        Err(CapabilityRefusal::OperationDenied)
    );
    assert_eq!(
        gate.evaluate(
            &Method::GET,
            &github_uri("/repos/contributor/project/commits/not-a-sha/status"),
            &[],
            NOW + 1,
        )
        .await,
        Err(CapabilityRefusal::MalformedRequest)
    );
}

#[tokio::test]
async fn pull_request_admin_upstream_and_cross_repository_routes_are_never_session_capabilities() {
    let (_, gate) = installed_gate().await;
    let attempts = [
        (
            Method::POST,
            "/repos/contributor/project/pulls",
            serde_json::json!({ "head": BRANCH, "base": "main", "title": "escape" }),
        ),
        (
            Method::PUT,
            "/repos/contributor/project/collaborators/attacker",
            serde_json::json!({ "permission": "admin" }),
        ),
        (
            Method::DELETE,
            "/repos/contributor/project/branches/main/protection",
            serde_json::Value::Null,
        ),
        (
            Method::POST,
            "/repos/upstream-owner/project/git/blobs",
            serde_json::json!({ "content": "write upstream", "encoding": "utf8" }),
        ),
        (
            Method::POST,
            "/repos/contributor/another-project/git/blobs",
            serde_json::json!({ "content": "write elsewhere", "encoding": "utf8" }),
        ),
    ];

    for (method, path, value) in attempts {
        let body = if value.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(&value).unwrap()
        };
        assert!(matches!(
            gate.evaluate(&method, &github_uri(path), &body, NOW + 1)
                .await,
            Err(CapabilityRefusal::OperationDenied | CapabilityRefusal::ResourceMismatch)
        ));
    }
}

#[tokio::test]
async fn launch_ref_creation_uses_the_exact_full_ref_from_the_signed_policy() {
    let (_, gate) = installed_gate().await;
    let uri = github_uri("/repos/contributor/project/git/refs");
    let body = serde_json::json!({ "ref": format!("refs/heads/{BRANCH}"), "sha": SHA })
        .to_string()
        .into_bytes();
    let request = gate
        .evaluate(&Method::POST, &uri, &body, NOW + 1)
        .await
        .expect("allowed")
        .unwrap();
    assert_eq!(request.operation, GithubOperationClass::LaunchRefCreate);

    let double_prefixed =
        serde_json::json!({ "ref": format!("refs/heads/refs/heads/{BRANCH}"), "sha": SHA })
            .to_string()
            .into_bytes();
    assert_eq!(
        gate.evaluate(&Method::POST, &uri, &double_prefixed, NOW + 1)
            .await,
        Err(CapabilityRefusal::ResourceMismatch)
    );
}

#[tokio::test]
async fn launch_ref_updates_require_the_exact_ref_and_force_false() {
    let (_, gate) = installed_gate().await;
    let uri = github_uri("/repos/contributor/project/git/refs/heads/cma/launch-1234");
    let body = serde_json::json!({ "sha": SHA, "force": false })
        .to_string()
        .into_bytes();
    let request = gate
        .evaluate(&Method::PATCH, &uri, &body, NOW + 1)
        .await
        .expect("allowed")
        .unwrap();
    assert_eq!(request.operation, GithubOperationClass::LaunchRefUpdate);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
        serde_json::json!({ "sha": SHA, "force": false })
    );

    let forced = serde_json::json!({ "sha": SHA, "force": true })
        .to_string()
        .into_bytes();
    assert_eq!(
        gate.evaluate(&Method::PATCH, &uri, &forced, NOW + 1).await,
        Err(CapabilityRefusal::OperationDenied)
    );
    let unknown = serde_json::json!({ "sha": SHA, "force": false, "extra": "ignored?" })
        .to_string()
        .into_bytes();
    assert_eq!(
        gate.evaluate(&Method::PATCH, &uri, &unknown, NOW + 1).await,
        Err(CapabilityRefusal::MalformedRequest)
    );
}

#[tokio::test]
async fn wrong_catalog_slug_routes_and_ambiguous_encodings_cannot_bypass_the_catalog_filter() {
    let (_, gate) = installed_gate().await;
    for raw in [
        "/api/v1/proxy/service-internal/repos/contributor/project",
        "/api/v1/proxy/s/api%2dgithub/repos/contributor/project",
        "/api/v1/proxy/s/api-github/repos/contributor/../project",
        "/api/v1/proxy/s/api-github//repos/contributor/project",
    ] {
        let uri: Uri = raw.parse().unwrap();
        assert!(matches!(
            gate.evaluate(&Method::GET, &uri, &[], NOW + 1).await,
            Err(CapabilityRefusal::OperationDenied
                | CapabilityRefusal::MalformedRequest
                | CapabilityRefusal::ResourceMismatch)
        ));
    }
}

#[tokio::test]
async fn signed_noncanonical_user_slug_is_evidence_only_and_never_an_authority_route() {
    let (_, gate) = installed_gate().await;
    for slug in [
        USER_SERVICE_SLUG,
        "api-github-pat",
        SERVICE,
        CATALOG_SERVICE,
    ] {
        let uri: Uri = format!("/api/v1/proxy/s/{slug}/repositories/30?_nyxid_via={SERVICE}")
            .parse()
            .unwrap();
        assert_eq!(
            gate.evaluate(&Method::GET, &uri, &[], NOW + 1).await,
            Err(CapabilityRefusal::OperationDenied)
        );
    }

    assert!(gate
        .evaluate(&Method::GET, &github_uri("/repositories/30"), &[], NOW + 1)
        .await
        .expect("the exact catalog-ID route is allowed")
        .is_some());
}

#[tokio::test]
async fn gateway_probe_binding_matches_the_full_live_signed_tuple() {
    let (signer, gate) = installed_gate().await;
    assert!(
        gate.matches_probe_binding(SERVICE, CATALOG_SERVICE, USER_SERVICE_SLUG, 30, NOW + 1)
            .await
    );
    for (service_id, catalog_service_id, user_service_slug, repository_id) in [
        ("other-service", CATALOG_SERVICE, USER_SERVICE_SLUG, 30),
        (SERVICE, "other-catalog", USER_SERVICE_SLUG, 30),
        (SERVICE, CATALOG_SERVICE, "api-github-3", 30),
        (SERVICE, CATALOG_SERVICE, USER_SERVICE_SLUG, 31),
    ] {
        assert!(
            !gate
                .matches_probe_binding(
                    service_id,
                    catalog_service_id,
                    user_service_slug,
                    repository_id,
                    NOW + 1,
                )
                .await
        );
    }
    assert!(
        !gate
            .matches_probe_binding(
                SERVICE,
                CATALOG_SERVICE,
                USER_SERVICE_SLUG,
                30,
                NOW + MAX_LIFETIME_SECONDS + MAX_CLOCK_SKEW_SECONDS + 1,
            )
            .await
    );

    gate.install(RUNTIME, &signer.sign(&tombstone(2, 2)), NOW + 1)
        .await
        .unwrap();
    assert!(
        !gate
            .matches_probe_binding(SERVICE, CATALOG_SERVICE, USER_SERVICE_SLUG, 30, NOW + 1)
            .await
    );
}

#[tokio::test]
async fn wrong_service_and_an_ungranted_operation_are_typed_denials() {
    let signer = Signer::new();
    let gate = gate(&signer);
    let mut read_only = policy(1, 1);
    if let CapabilityDocument::Policy {
        github_operation_classes,
        ..
    } = &mut read_only
    {
        *github_operation_classes = vec![GithubOperationClass::RepositoryRead];
    }
    gate.install(RUNTIME, &signer.sign(&read_only), NOW)
        .await
        .unwrap();
    let wrong_service: Uri =
        "/api/v1/proxy/catalog-service-github/repositories/30?_nyxid_via=other"
            .parse()
            .unwrap();
    assert_eq!(
        gate.evaluate(&Method::GET, &wrong_service, &[], NOW + 1)
            .await,
        Err(CapabilityRefusal::ResourceMismatch)
    );
    let update = github_uri("/repos/contributor/project/git/refs/heads/cma/launch-1234");
    let body = serde_json::json!({ "sha": SHA, "force": false })
        .to_string()
        .into_bytes();
    assert_eq!(
        gate.evaluate(&Method::PATCH, &update, &body, NOW + 1).await,
        Err(CapabilityRefusal::OperationDenied)
    );
}

#[tokio::test]
async fn signature_audience_session_and_lifetime_are_enforced() {
    let signer = Signer::new();
    let gate = gate(&signer);
    let mut envelope: serde_json::Value =
        serde_json::from_str(&signer.sign(&policy(1, 1))).unwrap();
    envelope["signature"] = serde_json::Value::String(
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0_u8; 64]),
    );
    assert_eq!(
        gate.install(RUNTIME, &envelope.to_string(), NOW).await,
        Err(CapabilityRefusal::SignatureInvalid)
    );
    assert_eq!(
        gate.install("another-runtime", &signer.sign(&policy(1, 1)), NOW)
            .await,
        Err(CapabilityRefusal::AudienceMismatch)
    );

    let mut other_session = policy(1, 1);
    if let CapabilityDocument::Policy { session_id, .. } = &mut other_session {
        *session_id = "sesn_someone_else".into();
    }
    assert_eq!(
        gate.install(RUNTIME, &signer.sign(&other_session), NOW)
            .await,
        Err(CapabilityRefusal::AudienceMismatch)
    );

    let mut other_launch = policy(1, 1);
    if let CapabilityDocument::Policy { launch_id, .. } = &mut other_launch {
        *launch_id = "12345678-1234-1234-1234-1234567890ac".into();
    }
    assert_eq!(
        gate.install(RUNTIME, &signer.sign(&other_launch), NOW)
            .await,
        Err(CapabilityRefusal::AudienceMismatch)
    );

    assert_eq!(
        gate.install(
            RUNTIME,
            &signer.sign(&policy(1, 1)),
            NOW + MAX_LIFETIME_SECONDS + MAX_CLOCK_SKEW_SECONDS + 1,
        )
        .await,
        Err(CapabilityRefusal::Expired)
    );
}

#[tokio::test]
async fn bare_branch_policy_is_rejected_as_a_resource_mismatch() {
    let signer = Signer::new();
    let gate = gate(&signer);
    let mut bare = policy(1, 1);
    if let CapabilityDocument::Policy { allowed_ref, .. } = &mut bare {
        *allowed_ref = BRANCH.into();
    }
    assert_eq!(
        gate.install(RUNTIME, &signer.sign(&bare), NOW).await,
        Err(CapabilityRefusal::ResourceMismatch)
    );
}

#[tokio::test]
async fn highest_generation_and_fence_win_and_a_tombstone_is_sticky() {
    let signer = Signer::new();
    let gate = gate(&signer);
    let first = signer.sign(&policy(1, 10));
    gate.install(RUNTIME, &first, NOW).await.unwrap();
    // Exact replay is idempotent.
    gate.install(RUNTIME, &first, NOW).await.unwrap();
    assert_eq!(
        gate.install(RUNTIME, &signer.sign(&policy(1, 9)), NOW)
            .await,
        Err(CapabilityRefusal::StaleGeneration)
    );
    let receipt = gate
        .install(RUNTIME, &signer.sign(&tombstone(2, 1)), NOW)
        .await
        .unwrap();
    assert!(receipt.revoked);
    assert_eq!(
        gate.evaluate(&Method::GET, &github_uri("/repositories/30"), &[], NOW + 1)
            .await,
        Err(CapabilityRefusal::Revoked)
    );
    assert_eq!(
        gate.install(RUNTIME, &signer.sign(&policy(1, 99)), NOW)
            .await,
        Err(CapabilityRefusal::StaleGeneration)
    );
    let snapshot = gate.snapshot().await;
    assert!(snapshot.revoked);
    assert_eq!(snapshot.generation, Some(2));
    assert_eq!(snapshot.last_refusal, Some("stale_generation"));
}

#[tokio::test]
async fn same_generation_and_fence_cannot_be_equivocated() {
    let signer = Signer::new();
    let gate = gate(&signer);
    gate.install(RUNTIME, &signer.sign(&policy(1, 1)), NOW)
        .await
        .unwrap();
    let mut conflicting = policy(1, 1);
    if let CapabilityDocument::Policy { allowed_ref, .. } = &mut conflicting {
        *allowed_ref = "refs/heads/cma/a-different-launch".into();
    }
    assert_eq!(
        gate.install(RUNTIME, &signer.sign(&conflicting), NOW).await,
        Err(CapabilityRefusal::StaleGeneration)
    );
}

#[tokio::test]
async fn github_decisions_emit_one_closed_redacted_audit_event_per_request() {
    let (_, installed) = installed_gate().await;
    let capture = AuditCapture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let allowed = installed
        .evaluate(&Method::GET, &github_uri("/repositories/30"), &[], NOW + 1)
        .await
        .expect("the approved repository read is evaluated");
    assert!(allowed.is_some());
    let events = capture.take();
    assert_eq!(events.len(), 1, "one request has one audit decision");
    let event = &events[0];
    assert_eq!(
        event.get("_target").map(String::as_str),
        Some("cma_trigger::github_capability_audit")
    );
    assert_eq!(event.get("schema_version").map(String::as_str), Some("1"));
    assert_eq!(event.get("session_id").map(String::as_str), Some(SESSION));
    assert_eq!(
        event.get("launch_id").map(String::as_str),
        Some("12345678-1234-1234-1234-1234567890ab")
    );
    assert_eq!(
        event.get("operation_class").map(String::as_str),
        Some("repository_read")
    );
    assert_eq!(event.get("decision").map(String::as_str), Some("allow"));
    assert_eq!(
        event.get("decision_code").map(String::as_str),
        Some(AUDIT_ALLOWED_CODE)
    );
    assert_eq!(
        event.get("upstream_repository_id").map(String::as_str),
        Some("10")
    );
    assert_eq!(
        event.get("fork_repository_id").map(String::as_str),
        Some("30")
    );

    let denied_body = br#"{"content":"audit-secret-must-not-be-recorded","encoding":"utf8"}"#;
    assert_eq!(
        installed
            .evaluate(
                &Method::POST,
                &github_uri("/repos/another-owner/private-repository/git/blobs"),
                denied_body,
                NOW + 1,
            )
            .await,
        Err(CapabilityRefusal::ResourceMismatch)
    );
    let events = capture.take();
    assert_eq!(events.len(), 1, "one denial has one audit decision");
    let event = &events[0];
    assert_eq!(
        event.get("operation_class").map(String::as_str),
        Some(AUDIT_UNCLASSIFIED_OPERATION)
    );
    assert_eq!(event.get("decision").map(String::as_str), Some("deny"));
    assert_eq!(
        event.get("decision_code").map(String::as_str),
        Some("resource_mismatch")
    );
    assert_eq!(event.get("fork_owner_id").map(String::as_str), Some("31"));
    let rendered = event
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(" ");
    for forbidden in [
        "another-owner",
        "private-repository",
        "contributor",
        "project",
        SERVICE,
        "audit-secret-must-not-be-recorded",
        "/repos/",
        "credential",
        "body",
        "path",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "audit admitted forbidden value {forbidden}: {rendered}"
        );
    }

    let signer = Signer::new();
    let missing = gate(&signer);
    assert_eq!(
        missing
            .evaluate(&Method::GET, &github_uri("/repositories/30"), &[], NOW)
            .await,
        Err(CapabilityRefusal::MissingPolicy)
    );
    let events = capture.take();
    assert_eq!(events.len(), 1, "missing policy has one audit decision");
    let event = &events[0];
    assert_eq!(event.get("session_id").map(String::as_str), Some(SESSION));
    assert_eq!(
        event.get("launch_id").map(String::as_str),
        Some("12345678-1234-1234-1234-1234567890ab")
    );
    assert_eq!(
        event.get("decision_code").map(String::as_str),
        Some("missing_policy")
    );
    assert!(!event.contains_key("fork_repository_id"));

    assert_eq!(
        missing
            .refuse_unreadable_body(&github_uri("/repos/contributor/project/git/blobs"))
            .await,
        Some(CapabilityRefusal::MalformedRequest)
    );
    let events = capture.take();
    assert_eq!(
        events.len(),
        1,
        "a pre-evaluation body refusal has one audit decision"
    );
    assert_eq!(
        events[0].get("decision_code").map(String::as_str),
        Some("malformed_request")
    );

    let passthrough: Uri = "/api/v1/proxy/s/ornn/skills?_nyxid_via=ornn-1"
        .parse()
        .unwrap();
    assert_eq!(missing.refuse_unreadable_body(&passthrough).await, None);
    assert!(capture.take().is_empty());
}

#[tokio::test]
async fn unknown_wire_versions_and_operation_classes_are_typed_denials() {
    let signer = Signer::new();
    let gate = gate(&signer);

    let mut envelope: serde_json::Value =
        serde_json::from_str(&signer.sign(&policy(1, 1))).unwrap();
    envelope["version"] = serde_json::json!(POLICY_VERSION + 1);
    assert_eq!(
        gate.install(RUNTIME, &envelope.to_string(), NOW).await,
        Err(CapabilityRefusal::UnknownVersion)
    );

    let mut future_document = policy(1, 1);
    if let CapabilityDocument::Policy { version, .. } = &mut future_document {
        *version = POLICY_VERSION + 1;
    }
    assert_eq!(
        gate.install(RUNTIME, &signer.sign(&future_document), NOW)
            .await,
        Err(CapabilityRefusal::UnknownVersion)
    );

    let mut future_operation = serde_json::to_value(policy(1, 1)).unwrap();
    future_operation["github_operation_classes"] =
        serde_json::json!(["repository_read", "future_admin_write"]);
    let payload = serde_json::to_vec(&future_operation).unwrap();
    assert_eq!(
        gate.install(RUNTIME, &signer.sign_payload(&payload), NOW)
            .await,
        Err(CapabilityRefusal::UnknownVersion)
    );
}

#[tokio::test]
async fn lifetime_and_clock_skew_boundaries_are_exact() {
    let signer = Signer::new();
    let at_boundary = gate(&signer);
    at_boundary
        .install(
            RUNTIME,
            &signer.sign(&policy(1, 1)),
            NOW - MAX_CLOCK_SKEW_SECONDS,
        )
        .await
        .expect("not-before minus five seconds is accepted");
    assert!(at_boundary
        .evaluate(
            &Method::GET,
            &github_uri("/repositories/30"),
            &[],
            NOW - MAX_CLOCK_SKEW_SECONDS,
        )
        .await
        .is_ok());
    assert!(at_boundary
        .evaluate(
            &Method::GET,
            &github_uri("/repositories/30"),
            &[],
            NOW + MAX_LIFETIME_SECONDS + MAX_CLOCK_SKEW_SECONDS,
        )
        .await
        .is_ok());
    assert_eq!(
        at_boundary
            .evaluate(
                &Method::GET,
                &github_uri("/repositories/30"),
                &[],
                NOW + MAX_LIFETIME_SECONDS + MAX_CLOCK_SKEW_SECONDS + 1,
            )
            .await,
        Err(CapabilityRefusal::Expired)
    );

    let too_early = gate(&signer);
    assert_eq!(
        too_early
            .install(
                RUNTIME,
                &signer.sign(&policy(1, 1)),
                NOW - MAX_CLOCK_SKEW_SECONDS - 1,
            )
            .await,
        Err(CapabilityRefusal::Expired)
    );

    let mut overlong = policy(1, 1);
    if let CapabilityDocument::Policy {
        expires_at_epoch_seconds,
        ..
    } = &mut overlong
    {
        *expires_at_epoch_seconds = NOW + MAX_LIFETIME_SECONDS + 1;
    }
    assert_eq!(
        gate(&signer)
            .install(RUNTIME, &signer.sign(&overlong), NOW)
            .await,
        Err(CapabilityRefusal::Expired)
    );
}

#[tokio::test]
async fn one_session_cannot_replace_or_use_another_sessions_policy() {
    let signer = Signer::new();
    let gate_a = gate(&signer);
    gate_a
        .install(RUNTIME, &signer.sign(&policy(1, 1)), NOW)
        .await
        .unwrap();

    let mut policy_b = policy(1, 1);
    if let CapabilityDocument::Policy {
        launch_id,
        session_id,
        runtime_audience,
        fork,
        ..
    } = &mut policy_b
    {
        *launch_id = "12345678-1234-1234-1234-1234567890ac".into();
        *session_id = SESSION_B.into();
        *runtime_audience = RUNTIME_B.into();
        fork.repository_id = 40;
        fork.repository_node_id = "R_fork_b".into();
        fork.owner_id = 41;
        fork.owner_node_id = "U_fork_b".into();
        fork.canonical_owner = "second-contributor".into();
        fork.canonical_name = "second-project".into();
    }
    let gate_b = gate_for(&signer, SESSION_B);
    gate_b
        .install(RUNTIME_B, &signer.sign(&policy_b), NOW)
        .await
        .unwrap();

    assert_eq!(
        gate_b
            .install(RUNTIME_B, &signer.sign(&policy(2, 2)), NOW)
            .await,
        Err(CapabilityRefusal::AudienceMismatch)
    );
    let snapshot = gate_b.snapshot().await;
    assert!(snapshot.installed);
    assert_eq!(
        snapshot.launch_id.as_deref(),
        Some("12345678-1234-1234-1234-1234567890ac")
    );
    assert_eq!(snapshot.generation, Some(1));
    assert_eq!(snapshot.fence, Some(1));

    assert_eq!(
        gate_b
            .evaluate(&Method::GET, &github_uri("/repositories/30"), &[], NOW + 1)
            .await,
        Err(CapabilityRefusal::ResourceMismatch)
    );
    let own_b_uri: Uri =
        "/api/v1/proxy/catalog-service-github/repositories/40?_nyxid_via=service-1"
            .parse()
            .unwrap();
    assert!(gate_b
        .evaluate(&Method::GET, &own_b_uri, &[], NOW + 1)
        .await
        .expect("session B retains its own policy")
        .is_some());
    assert_eq!(
        gate_a
            .evaluate(&Method::GET, &own_b_uri, &[], NOW + 1)
            .await,
        Err(CapabilityRefusal::ResourceMismatch)
    );
}

#[test]
fn capability_configuration_fails_closed() {
    assert!(GithubCapabilityGate::from_configuration(true, SESSION, Vec::new(), "").is_err());
    assert!(GithubCapabilityGate::from_configuration(
        true,
        SESSION,
        Vec::new(),
        r#"{"key-1":"not base64"}"#
    )
    .is_err());
    assert!(GithubCapabilityGate::from_configuration(
        true,
        "sesn_not_a_deterministic_trigger_session",
        vec![GITHUB_SERVICE_SLUG.into()],
        &Signer::new().keyring(),
    )
    .is_err());
    let disabled = GithubCapabilityGate::disabled();
    assert!(!disabled.required());
    assert!(!disabled.supported());
}
