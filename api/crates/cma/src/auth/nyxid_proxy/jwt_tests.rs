use super::*;
use axum::{http::StatusCode, routing::get, Router};
use ring::{
    rand::SystemRandom,
    signature::{KeyPair, RsaKeyPair, RSA_PKCS1_SHA256},
};
use serde_json::{json, Value};
use std::{
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    },
};
use tokio::sync::RwLock;

// Test-only RSA keys are generated in memory. No private fixture is committed.
pub(crate) struct Signer(RsaKeyPair);
impl Signer {
    fn generate() -> Self {
        let output = Command::new("openssl")
            .args(["genrsa", "2048"])
            .output()
            .expect("openssl generates an ephemeral test RSA key");
        assert!(output.status.success(), "test RSA generation failed");
        let pem = String::from_utf8(output.stdout).unwrap();
        let encoded: String = pem.lines().filter(|line| !line.starts_with('-')).collect();
        let der = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let key = if pem.starts_with("-----BEGIN PRIVATE KEY-----") {
            RsaKeyPair::from_pkcs8(&der)
        } else {
            RsaKeyPair::from_der(&der)
        };
        Self(key.unwrap())
    }
    pub(crate) fn primary() -> &'static Self {
        static SIGNER: OnceLock<Signer> = OnceLock::new();
        SIGNER.get_or_init(Self::generate)
    }
    pub(crate) fn rotated() -> &'static Self {
        static SIGNER: OnceLock<Signer> = OnceLock::new();
        SIGNER.get_or_init(Self::generate)
    }
    pub(crate) fn token(&self, claims: &Value, kid: &str) -> String {
        self.raw(
            &json!({"alg":"RS256","typ":"JWT","kid":kid}).to_string(),
            &claims.to_string(),
        )
    }
    fn raw(&self, header: &str, body: &str) -> String {
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header),
            URL_SAFE_NO_PAD.encode(body)
        );
        let mut signature = vec![0; self.0.public().modulus_len()];
        self.0
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                signed.as_bytes(),
                &mut signature,
            )
            .unwrap();
        format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature))
    }
    pub(crate) fn key(&self, kid: &str) -> Value {
        let mut der = self.0.public_key().as_ref();
        let mut sequence = tlv(&mut der, 0x30);
        let n = tlv(&mut sequence, 0x02);
        let e = tlv(&mut sequence, 0x02);
        json!({"kty":"RSA","kid":kid,"alg":"RS256","use":"sig","key_ops":["verify"],
            "n":URL_SAFE_NO_PAD.encode(n.strip_prefix(&[0]).unwrap_or(n)),"e":URL_SAFE_NO_PAD.encode(e)})
    }
}

fn tlv<'a>(input: &mut &'a [u8], tag: u8) -> &'a [u8] {
    assert_eq!(input[0], tag);
    let head = input[1];
    *input = &input[2..];
    let size = if head < 128 {
        head as usize
    } else {
        let bytes = (head & 127) as usize;
        let size = input[..bytes]
            .iter()
            .fold(0_usize, |n, b| (n << 8) | usize::from(*b));
        *input = &input[bytes..];
        size
    };
    let value = &input[..size];
    *input = &input[size..];
    value
}

fn config(jwks_url: String) -> Config {
    Config {
        issuer: "https://nyx.example".into(),
        jwks_url,
        audience: "https://cma.example/agent-manager".into(),
        service_id: "cma-service".into(),
    }
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
fn claims(at: i64) -> Value {
    json!({"sub":"owner","iss":"https://nyx.example","aud":"https://cma.example/agent-manager",
        "iat":at,"exp":at+60,"jti":"assertion-1","nyx_service_id":"cma-service",
        "permissions":["cma:sandbox:use","cma:agent:write"],"roles":[],"groups":[],
        "name":" Owner ","email":"owner@example.test"})
}
fn key_set() -> Value {
    json!({"keys":[Signer::primary().key("key-1")]})
}
fn verify_locally(raw: &str, at: i64) -> Result<VerifiedIdentity, VerifyError> {
    KeySet::parse(key_set().to_string().as_bytes())?
        .verify(&Token::parse(raw)?)?
        .validate(&config("https://nyx.example/jwks".into()), at)
}

struct Fixture {
    verifier: Verifier,
    body: Arc<RwLock<String>>,
    status: Arc<RwLock<StatusCode>>,
    calls: Arc<AtomicUsize>,
    redirected: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let body = Arc::new(RwLock::new(key_set().to_string()));
        let status = Arc::new(RwLock::new(StatusCode::OK));
        let calls = Arc::new(AtomicUsize::new(0));
        let redirected = Arc::new(AtomicUsize::new(0));
        let (returned, response_status, counter) = (body.clone(), status.clone(), calls.clone());
        let redirect_counter = redirected.clone();
        let app = Router::new()
            .route(
                "/jwks",
                get(move || {
                    let (body, status, counter) =
                        (returned.clone(), response_status.clone(), counter.clone());
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        (
                            *status.read().await,
                            [("location", "/redirect-target")],
                            body.read().await.clone(),
                        )
                    }
                }),
            )
            .route(
                "/redirect-target",
                get(move || {
                    let counter = redirect_counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        "redirected"
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/jwks", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            verifier: Verifier::new(config(url)).unwrap(),
            body,
            status,
            calls,
            redirected,
            server,
        }
    }
    fn requests(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    async fn permit_refresh(&self) {
        self.verifier.cache.lock().await.next_refresh = None;
    }
    async fn expire_cache(&self) {
        let mut cache = self.verifier.cache.lock().await;
        cache.next_refresh = None;
        if let Some((_, expiry)) = cache.keys.as_mut() {
            *expiry = Instant::now();
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[test]
fn verifies_existing_identity_contract_and_returns_bounded_evidence() {
    let at = now();
    let identity = verify_locally(&Signer::primary().token(&claims(at), "key-1"), at).unwrap();
    assert_eq!(identity.subject, "owner");
    assert_eq!(identity.service_id, "cma-service");
    assert_eq!(identity.jti, "assertion-1");
    assert_eq!(identity.permissions, ["cma:sandbox:use", "cma:agent:write"]);
    assert_eq!(identity.profile.display_name.as_deref(), Some("Owner"));
    assert_eq!(
        identity.profile.email.as_deref(),
        Some("owner@example.test")
    );
    assert_eq!(
        identity
            .expires_at
            .duration_since(UNIX_EPOCH + Duration::from_secs(at as u64))
            .unwrap(),
        Duration::from_secs(60)
    );
}

#[test]
fn destination_identity_and_permissions_are_required_and_strictly_typed() {
    let at = now();
    for (field, replacement) in [
        ("iss", json!("https://wrong.example")),
        ("aud", json!("other-service")),
        ("aud", json!(["https://cma.example/agent-manager"])),
        ("nyx_service_id", json!("other-service")),
        ("sub", json!("")),
        ("sub", json!("owner\n")),
        ("jti", json!("")),
        ("permissions", json!("cma:*")),
        ("permissions", json!([null])),
        ("permissions", json!(["cma:sandbox:use\n"])),
    ] {
        let mut value = claims(at);
        value[field] = replacement;
        assert!(
            matches!(
                verify_locally(&Signer::primary().token(&value, "key-1"), at),
                Err(VerifyError::Rejected(_))
            ),
            "{field}"
        );
    }
    for field in [
        "sub",
        "iss",
        "aud",
        "nyx_service_id",
        "iat",
        "exp",
        "jti",
        "permissions",
    ] {
        let mut value = claims(at);
        value.as_object_mut().unwrap().remove(field);
        assert!(
            verify_locally(&Signer::primary().token(&value, "key-1"), at).is_err(),
            "missing {field}"
        );
    }
    let mut no_permissions = claims(at);
    no_permissions["permissions"] = json!([]);
    assert!(
        verify_locally(&Signer::primary().token(&no_permissions, "key-1"), at)
            .unwrap()
            .permissions
            .is_empty()
    );
}

#[test]
fn expiry_future_issue_time_not_before_and_maximum_lifetime_are_enforced() {
    let at = now();
    for (iat, exp, nbf) in [
        (at - 60, at, None),
        (at - 61, at - 1, None),
        (at, at + 61, None),
        (at, at, None),
        (at + 6, at + 66, None),
        (-1, at + 60, None),
        (at, at + 60, Some(at + 6)),
        (at, at + 60, Some(at + 60)),
        (at, at + 60, Some(-1)),
    ] {
        let mut value = claims(at);
        value["iat"] = json!(iat);
        value["exp"] = json!(exp);
        if let Some(nbf) = nbf {
            value["nbf"] = json!(nbf);
        }
        assert_eq!(
            verify_locally(&Signer::primary().token(&value, "key-1"), at).unwrap_err(),
            VerifyError::Rejected("token_time")
        );
    }
    let mut value = claims(at + 5);
    value["nbf"] = json!(at + 5);
    assert!(verify_locally(&Signer::primary().token(&value, "key-1"), at).is_ok());
}

#[test]
fn algorithm_type_header_key_source_and_duplicate_header_fields_are_rejected() {
    let at = now();
    for header in [
        json!({"alg":"none","typ":"JWT","kid":"key-1"}),
        json!({"alg":"HS256","typ":"JWT","kid":"key-1"}),
        json!({"alg":"RS256","typ":"at+jwt","kid":"key-1"}),
        json!({"alg":"RS256","typ":"JWT"}),
        json!({"alg":"RS256","typ":"JWT","kid":""}),
        json!({"alg":"RS256","typ":"JWT","kid":"key-1","jku":"https://attacker.example/jwks"}),
        json!({"alg":"RS256","typ":"JWT","kid":"key-1","b64":false}),
        json!({"alg":"RS256","typ":"JWT","kid":"key-1","crit":["custom"]}),
    ] {
        let raw = Signer::primary().raw(&header.to_string(), &claims(at).to_string());
        assert_eq!(
            verify_locally(&raw, at).unwrap_err(),
            VerifyError::Rejected("token_header")
        );
    }
    let duplicated = r#"{"alg":"RS256","alg":"RS256","typ":"JWT","kid":"key-1"}"#;
    assert!(Token::parse(&Signer::primary().raw(duplicated, &claims(at).to_string())).is_err());
}

#[test]
fn duplicate_claims_and_other_nyxid_token_families_do_not_authenticate() {
    let at = now();
    let duplicate = format!("{{\"sub\":\"other-owner\",{}", &claims(at).to_string()[1..]);
    let header = json!({"alg":"RS256","typ":"JWT","kid":"key-1"}).to_string();
    assert_eq!(
        verify_locally(&Signer::primary().raw(&header, &duplicate), at).unwrap_err(),
        VerifyError::Rejected("token_claims")
    );
    for (key, value) in [
        ("token_type", json!("access")),
        ("scope", json!("proxy")),
        ("act", json!({"sub":"cma"})),
        ("resource_request", json!({"method":"GET"})),
    ] {
        let mut other = claims(at);
        other[key] = value;
        assert_eq!(
            verify_locally(&Signer::primary().token(&other, "key-1"), at).unwrap_err(),
            VerifyError::Rejected("token_claims")
        );
    }
}

#[test]
fn jwks_rejects_duplicate_ids_fields_and_invalid_key_purpose() {
    let key = Signer::primary().key("key-1");
    assert!(KeySet::parse(
        json!({"keys":[key.clone(),key.clone()]})
            .to_string()
            .as_bytes()
    )
    .is_err());
    let duplicate_kid = format!(
        "{{\"keys\":[{{\"kid\":\"key-1\",{}]}}",
        &key.to_string()[1..]
    );
    assert!(KeySet::parse(duplicate_kid.as_bytes()).is_err());
    for (field, value) in [
        ("kty", json!("EC")),
        ("alg", json!("HS256")),
        ("use", json!("enc")),
        ("key_ops", json!(["sign"])),
        ("key_ops", json!(["verify", "sign"])),
        ("n", json!("AA")),
        ("e", json!("AA")),
    ] {
        let mut key = key.clone();
        key[field] = value;
        assert!(
            KeySet::parse(json!({"keys":[key]}).to_string().as_bytes()).is_err(),
            "{field}"
        );
    }
    assert!(KeySet::parse(json!({"keys":[]}).to_string().as_bytes()).is_err());
    assert!(KeySet::parse(&vec![b' '; MAX_JWKS_BYTES + 1]).is_err());
}

#[test]
fn endpoint_configuration_and_http_header_multiplicity_are_validated() {
    for url in [
        "http://nyx.example/jwks",
        "https://user:pass@nyx.example/jwks",
        "https://nyx.example/jwks?query=1",
        "https://nyx.example/jwks#fragment",
        "file:///tmp/jwks",
    ] {
        assert!(Verifier::new(config(url.into())).is_err(), "{url}");
    }
    assert!(Verifier::new(config("http://127.0.0.1:10000/jwks".into())).is_ok());
    let mut headers = axum::http::HeaderMap::new();
    assert!(identity_token(&headers).unwrap().is_none());
    headers.append(HEADER, "token-one".parse().unwrap());
    assert_eq!(identity_token(&headers).unwrap(), Some("token-one"));
    headers.append(HEADER, "token-two".parse().unwrap());
    assert_eq!(
        identity_token(&headers).unwrap_err(),
        VerifyError::Rejected("duplicate_identity_header")
    );
}

#[tokio::test]
async fn concurrent_requests_coalesce_and_fresh_cache_avoids_provider_io() {
    let fixture = Fixture::new().await;
    let token = Signer::primary().token(&claims(now()), "key-1");
    let results =
        futures_util::future::join_all((0..24).map(|_| fixture.verifier.verify(&token))).await;
    assert!(results.into_iter().all(|result| result.is_ok()));
    assert_eq!(fixture.requests(), 1);
    fixture.verifier.verify(&token).await.unwrap();
    assert_eq!(fixture.requests(), 1);
    fixture.expire_cache().await;
    fixture.verifier.verify(&token).await.unwrap();
    assert_eq!(fixture.requests(), 2);
}

#[tokio::test]
async fn bad_signature_for_known_kid_never_refreshes_or_accepts_replacement_key() {
    let fixture = Fixture::new().await;
    let value = claims(now());
    fixture
        .verifier
        .verify(&Signer::primary().token(&value, "key-1"))
        .await
        .unwrap();
    fixture.permit_refresh().await;
    let forged = Signer::rotated().token(&value, "key-1");
    assert_eq!(
        fixture.verifier.verify(&forged).await.unwrap_err(),
        VerifyError::Rejected("token_signature")
    );
    assert_eq!(fixture.requests(), 1);
    let valid = Signer::primary().token(&value, "key-1");
    let mut parts: Vec<_> = valid.split('.').map(str::to_owned).collect();
    let mut altered = value;
    altered["sub"] = json!("another-owner");
    parts[1] = URL_SAFE_NO_PAD.encode(altered.to_string());
    assert_eq!(
        fixture.verifier.verify(&parts.join(".")).await.unwrap_err(),
        VerifyError::Rejected("token_signature")
    );
    assert_eq!(fixture.requests(), 1);
}

#[tokio::test]
async fn unknown_key_refresh_is_bounded_even_for_distinct_attacker_kids() {
    let fixture = Fixture::new().await;
    let value = claims(now());
    fixture
        .verifier
        .verify(&Signer::primary().token(&value, "key-1"))
        .await
        .unwrap();
    fixture.permit_refresh().await;
    for kid in ["unknown-1", "unknown-2", "unknown-3", "unknown-1"] {
        assert_eq!(
            fixture
                .verifier
                .verify(&Signer::primary().token(&value, kid))
                .await
                .unwrap_err(),
            VerifyError::Rejected("signing_key_unknown")
        );
    }
    assert_eq!(fixture.requests(), 2);
}

#[tokio::test]
async fn rotated_and_retained_keys_work_then_retired_key_stops_working() {
    let fixture = Fixture::new().await;
    let value = claims(now());
    let old = Signer::primary().token(&value, "key-1");
    let new = Signer::rotated().token(&value, "key-2");
    fixture.verifier.verify(&old).await.unwrap();
    *fixture.body.write().await =
        json!({"keys":[Signer::primary().key("key-1"),Signer::rotated().key("key-2")]}).to_string();
    fixture.permit_refresh().await;
    fixture.verifier.verify(&new).await.unwrap();
    fixture.verifier.verify(&old).await.unwrap();
    assert_eq!(fixture.requests(), 2);
    *fixture.body.write().await = json!({"keys":[Signer::rotated().key("key-2")]}).to_string();
    fixture.expire_cache().await;
    fixture.verifier.verify(&new).await.unwrap();
    assert_eq!(
        fixture.verifier.verify(&old).await.unwrap_err(),
        VerifyError::Rejected("signing_key_unknown")
    );
    assert_eq!(fixture.requests(), 3);
}

#[tokio::test]
async fn failed_unknown_key_refresh_keeps_fresh_keys_but_never_uses_expired_keys() {
    let fixture = Fixture::new().await;
    let value = claims(now());
    let valid = Signer::primary().token(&value, "key-1");
    fixture.verifier.verify(&valid).await.unwrap();
    *fixture.status.write().await = StatusCode::SERVICE_UNAVAILABLE;
    fixture.permit_refresh().await;
    assert!(matches!(
        fixture
            .verifier
            .verify(&Signer::primary().token(&value, "unknown"))
            .await,
        Err(VerifyError::Unavailable(_))
    ));
    fixture.verifier.verify(&valid).await.unwrap();
    assert_eq!(fixture.requests(), 2);
    fixture.expire_cache().await;
    assert!(matches!(
        fixture.verifier.verify(&valid).await,
        Err(VerifyError::Unavailable(_))
    ));
    assert!(matches!(
        fixture.verifier.verify(&valid).await,
        Err(VerifyError::Unavailable(_))
    ));
    assert_eq!(fixture.requests(), 3);
}

#[tokio::test]
async fn redirects_oversized_responses_and_invalid_provider_keys_fail_closed() {
    let fixture = Fixture::new().await;
    let token = Signer::primary().token(&claims(now()), "key-1");
    *fixture.status.write().await = StatusCode::FOUND;
    assert_eq!(
        fixture.verifier.verify(&token).await.unwrap_err(),
        VerifyError::Unavailable("jwks_status")
    );
    assert_eq!(fixture.redirected.load(Ordering::SeqCst), 0);
    *fixture.status.write().await = StatusCode::OK;
    *fixture.body.write().await = " ".repeat(MAX_JWKS_BYTES + 1);
    fixture.permit_refresh().await;
    assert_eq!(
        fixture.verifier.verify(&token).await.unwrap_err(),
        VerifyError::Unavailable("jwks_bounds")
    );
    *fixture.body.write().await =
        json!({"keys":[Signer::primary().key("key-1"),Signer::primary().key("key-1")]}).to_string();
    fixture.permit_refresh().await;
    assert_eq!(
        fixture.verifier.verify(&token).await.unwrap_err(),
        VerifyError::Unavailable("jwks_invalid")
    );
}

#[tokio::test]
async fn malformed_and_oversized_tokens_never_fetch_keys() {
    let fixture = Fixture::new().await;
    for token in [
        "".into(),
        "a.b.c.d".into(),
        "*.*.*".into(),
        "a".repeat(MAX_TOKEN_BYTES + 1),
    ] {
        assert!(matches!(
            fixture.verifier.verify(&token).await,
            Err(VerifyError::Rejected(_))
        ));
    }
    assert_eq!(fixture.requests(), 0);
}
