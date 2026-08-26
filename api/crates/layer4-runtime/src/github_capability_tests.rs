use super::*;

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};

const NOW: u64 = 1_000_000;
const SESSION: &str = "sesn_trg_123456781234123412341234567890ab";
const RUNTIME: &str = "sandbox-runtime-1";
const SERVICE: &str = "service-1";
const BRANCH: &str = "cma/launch-1234";
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Signer(Ed25519KeyPair);

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
        upstream: upstream(),
        fork: fork(),
        allowed_ref: BRANCH.into(),
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
    GithubCapabilityGate::from_configuration(
        true,
        SESSION,
        vec![
            "chrono-llm".into(),
            "ornn".into(),
            GITHUB_SERVICE_SLUG.into(),
        ],
        &signer.keyring(),
    )
    .expect("valid gate")
}

fn github_uri(path: &str) -> Uri {
    format!("/api/v1/proxy/s/api-github{path}?_nyxid_via={SERVICE}")
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
    let unknown: Uri = "/api/v1/proxy/s/some-other-service/data".parse().unwrap();
    assert_eq!(
        gate.evaluate(&Method::GET, &unknown, &[], NOW).await,
        Err(CapabilityRefusal::OperationDenied)
    );
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
        "/api/v1/proxy/s/api-github/repositories/30?_nyxid_via=service-1"
    );
    assert!(request.body.is_empty());
}

#[tokio::test]
async fn tree_reads_accept_only_the_fork_and_known_query_shape() {
    let (_, gate) = installed_gate().await;
    let uri: Uri = format!(
        "/api/v1/proxy/s/api-github/repos/contributor/project/git/trees/{SHA}?recursive=1&_nyxid_via={SERVICE}"
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
            "/api/v1/proxy/s/api-github/repos/contributor/project/git/trees/{SHA}?_nyxid_via={SERVICE}&recursive=1"
        )
    );

    let other = github_uri(&format!("/repos/someone-else/project/git/trees/{SHA}"));
    assert_eq!(
        gate.evaluate(&Method::GET, &other, &[], NOW + 1).await,
        Err(CapabilityRefusal::ResourceMismatch)
    );
    let extra: Uri = format!(
        "/api/v1/proxy/s/api-github/repos/contributor/project/git/trees/{SHA}?recursive=1&page=2&_nyxid_via={SERVICE}"
    )
    .parse()
    .unwrap();
    assert_eq!(
        gate.evaluate(&Method::GET, &extra, &[], NOW + 1).await,
        Err(CapabilityRefusal::MalformedRequest)
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
async fn raw_service_id_and_ambiguous_encodings_cannot_bypass_the_slug_filter() {
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
            Err(CapabilityRefusal::OperationDenied | CapabilityRefusal::MalformedRequest)
        ));
    }
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
    let wrong_service: Uri = "/api/v1/proxy/s/api-github/repositories/30?_nyxid_via=other"
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
        *allowed_ref = "cma/a-different-launch".into();
    }
    assert_eq!(
        gate.install(RUNTIME, &signer.sign(&conflicting), NOW).await,
        Err(CapabilityRefusal::StaleGeneration)
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
    let disabled = GithubCapabilityGate::disabled();
    assert!(!disabled.required());
    assert!(!disabled.supported());
}
