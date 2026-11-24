use super::*;
use axum::{http::StatusCode, routing::get, Router};
use cma_service_auth::signing_keys::{FetchFuture, Fetcher};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{RwLock, Semaphore};
use tokio::time::timeout;

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

fn forged_token(kid: &str) -> String {
    format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(json!({"alg":"RS256","typ":"JWT","kid":kid}).to_string()),
        URL_SAFE_NO_PAD.encode("{}"),
        URL_SAFE_NO_PAD.encode([0_u8; 256])
    )
}

struct Gate {
    held: AtomicBool,
    entered: Semaphore,
    released: Semaphore,
    completed: Semaphore,
}
impl Gate {
    fn new() -> Self {
        Self {
            held: AtomicBool::new(false),
            entered: Semaphore::new(0),
            released: Semaphore::new(0),
            completed: Semaphore::new(0),
        }
    }
    fn hold(&self) {
        self.held.store(true, Ordering::SeqCst);
    }
    async fn wait_for_entry(&self) {
        timeout(Duration::from_secs(1), self.entered.acquire())
            .await
            .expect("JWKS handler entered")
            .unwrap()
            .forget();
    }
    async fn wait_for_completion(&self) {
        timeout(Duration::from_secs(1), self.completed.acquire())
            .await
            .expect("JWKS fetch completed")
            .unwrap()
            .forget();
    }
    fn release(&self) {
        self.held.store(false, Ordering::SeqCst);
        self.released.add_permits(1);
    }
}

struct Fixture {
    verifier: Arc<Verifier>,
    body: Arc<RwLock<String>>,
    status: Arc<RwLock<StatusCode>>,
    calls: Arc<AtomicUsize>,
    redirected: Arc<AtomicUsize>,
    gate: Arc<Gate>,
    server: Option<tokio::task::JoinHandle<()>>,
}
impl Fixture {
    async fn new() -> Self {
        let body = Arc::new(RwLock::new(key_set().to_string()));
        let status = Arc::new(RwLock::new(StatusCode::OK));
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Gate::new());
        let fetcher = Arc::new(FakeFetcher {
            body: body.clone(),
            status: status.clone(),
            calls: calls.clone(),
            gate: gate.clone(),
        });
        let cache = Cache::with_fetcher(
            signing_keys::Config::new(CACHE_TTL, 1),
            KeySet::parse,
            transport_error,
            fetcher,
        );
        Self {
            verifier: Arc::new(Verifier {
                config: config("https://nyx.example/jwks".into()),
                cache,
            }),
            body,
            status,
            calls,
            gate,
            redirected: Arc::new(AtomicUsize::new(0)),
            server: None,
        }
    }
    async fn new_http() -> Self {
        let body = Arc::new(RwLock::new(key_set().to_string()));
        let status = Arc::new(RwLock::new(StatusCode::OK));
        let calls = Arc::new(AtomicUsize::new(0));
        let redirected = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Gate::new());
        let (returned, response_status, counter) = (body.clone(), status.clone(), calls.clone());
        let redirect_counter = redirected.clone();
        let request_gate = gate.clone();
        let app = Router::new()
            .route(
                "/jwks",
                get(move || {
                    let (body, status, counter) =
                        (returned.clone(), response_status.clone(), counter.clone());
                    let gate = request_gate.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        if gate.held.load(Ordering::SeqCst) {
                            gate.entered.add_permits(1);
                            timeout(Duration::from_secs(1), gate.released.acquire())
                                .await
                                .expect("JWKS handler released")
                                .unwrap()
                                .forget();
                        }
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
            verifier: Arc::new(Verifier::new(config(url)).unwrap()),
            body,
            status,
            calls,
            redirected,
            gate,
            server: Some(server),
        }
    }
    fn requests(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    async fn permit_refresh(&self) {
        if self.server.is_some() {
            tokio::time::sleep(Duration::from_secs(5)).await;
        } else {
            tokio::time::advance(Duration::from_secs(5)).await;
        }
    }
    async fn expire_cache(&self) {
        tokio::time::advance(CACHE_TTL).await;
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(server) = &self.server {
            server.abort();
        }
    }
}
struct FakeFetcher {
    body: Arc<RwLock<String>>,
    status: Arc<RwLock<StatusCode>>,
    calls: Arc<AtomicUsize>,
    gate: Arc<Gate>,
}
impl Fetcher for FakeFetcher {
    fn fetch<'a>(&'a self, _: &'a str, max_body_bytes: usize) -> FetchFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.gate.held.load(Ordering::SeqCst) {
                self.gate.entered.add_permits(1);
                self.gate.released.acquire().await.unwrap().forget();
            }
            if !self.status.read().await.is_success() {
                return Err(TransportFailure::Status);
            }
            let bytes = self.body.read().await.as_bytes().to_vec();
            if bytes.len() > max_body_bytes {
                return Err(TransportFailure::Bounds);
            }
            self.gate.completed.add_permits(1);
            Ok(bytes)
        })
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

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
async fn cached_kid_verifies_while_forged_unknown_kid_refresh_is_held() {
    let fixture = Fixture::new().await;
    let valid = Signer::primary().token(&claims(now()), "key-1");
    fixture.verifier.verify(&valid).await.unwrap();
    fixture.permit_refresh().await;
    fixture.gate.hold();
    let verifier = fixture.verifier.clone();
    let attack =
        tokio::spawn(async move { verifier.verify(&forged_token("attacker-unknown-kid")).await });
    fixture.gate.wait_for_entry().await;
    timeout(Duration::from_millis(100), fixture.verifier.verify(&valid))
        .await
        .expect("cached key must not wait for JWKS")
        .unwrap();
    assert!(!attack.is_finished());
    fixture.gate.release();
    assert_eq!(
        timeout(Duration::from_secs(1), attack)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err(),
        VerifyError::Rejected("signing_key_unknown")
    );
    assert_eq!(fixture.requests(), 2);
}

#[tokio::test(start_paused = true)]
async fn failed_refresh_and_cooldown_keep_cached_kids_available() {
    let fixture = Fixture::new().await;
    let valid = Signer::primary().token(&claims(now()), "key-1");
    fixture.verifier.verify(&valid).await.unwrap();
    fixture.permit_refresh().await;
    *fixture.status.write().await = StatusCode::SERVICE_UNAVAILABLE;
    fixture.gate.hold();
    let verifier = fixture.verifier.clone();
    let starter = tokio::spawn(async move { verifier.verify(&forged_token("unknown-1")).await });
    fixture.gate.wait_for_entry().await;
    timeout(Duration::from_millis(100), fixture.verifier.verify(&valid))
        .await
        .expect("cached key must remain available during a failing refresh")
        .unwrap();
    let unknown = forged_token("unknown-2");
    let mut joiner = std::pin::pin!(fixture.verifier.verify(&unknown));
    assert!(futures_util::poll!(&mut joiner).is_pending());
    fixture.gate.release();
    assert_eq!(
        timeout(Duration::from_secs(1), starter)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err(),
        VerifyError::Unavailable("jwks_status")
    );
    assert_eq!(
        timeout(Duration::from_secs(1), joiner)
            .await
            .unwrap()
            .unwrap_err(),
        VerifyError::Rejected("signing_key_unknown")
    );
    fixture.verifier.verify(&valid).await.unwrap();
    assert_eq!(
        fixture
            .verifier
            .verify(&forged_token("unknown-3"))
            .await
            .unwrap_err(),
        VerifyError::Rejected("signing_key_unknown")
    );
    fixture.verifier.verify(&valid).await.unwrap();
    assert_eq!(fixture.requests(), 2);
}

#[tokio::test(start_paused = true)]
async fn concurrent_unknown_kids_join_one_refresh_even_after_cooldown() {
    for past_cooldown in [false, true] {
        let fixture = Fixture::new().await;
        let valid = Signer::primary().token(&claims(now()), "key-1");
        fixture.verifier.verify(&valid).await.unwrap();
        fixture.permit_refresh().await;
        fixture.gate.hold();
        let verifier = fixture.verifier.clone();
        let starter = tokio::spawn(async move { verifier.verify(&forged_token("unknown")).await });
        fixture.gate.wait_for_entry().await;
        if past_cooldown {
            // Keep the flight held past the cooldown, then publish the joiners' key.
            fixture.permit_refresh().await;
            *fixture.body.write().await =
                json!({"keys":[Signer::primary().key("rotated")]}).to_string();
        }
        let tokens: Vec<_> = (0..24)
            .map(|index| {
                forged_token(&if past_cooldown {
                    "rotated".to_owned()
                } else {
                    format!("unknown-{index}")
                })
            })
            .collect();
        let mut joiners = std::pin::pin!(futures_util::future::join_all(
            tokens.iter().map(|token| fixture.verifier.verify(token))
        ));
        assert!(futures_util::poll!(&mut joiners).is_pending());
        assert_eq!(fixture.requests(), 2);
        fixture.gate.release();
        assert_eq!(
            timeout(Duration::from_secs(1), starter)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err(),
            VerifyError::Rejected("signing_key_unknown")
        );
        for result in timeout(Duration::from_secs(1), joiners).await.unwrap() {
            assert_eq!(
                result.unwrap_err(),
                if past_cooldown {
                    VerifyError::Rejected("token_signature")
                } else {
                    VerifyError::Rejected("signing_key_unknown")
                }
            );
        }
        assert_eq!(fixture.requests(), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn cancelling_refresh_starter_still_stores_rotated_keys() {
    let fixture = Fixture::new().await;
    let value = claims(now());
    let old = Signer::primary().token(&value, "key-1");
    let rotated = Signer::rotated().token(&value, "key-2");
    fixture.verifier.verify(&old).await.unwrap();
    fixture.gate.wait_for_completion().await;
    *fixture.body.write().await = json!({"keys":[Signer::rotated().key("key-2")]}).to_string();
    fixture.permit_refresh().await;
    fixture.gate.hold();
    let verifier = fixture.verifier.clone();
    let starter = tokio::spawn(async move { verifier.verify(&forged_token("unknown")).await });
    fixture.gate.wait_for_entry().await;
    starter.abort();
    assert!(starter.await.unwrap_err().is_cancelled());
    fixture.gate.release();
    fixture.gate.wait_for_completion().await;
    // On this current-thread runtime the refresh cannot yield between the
    // fetch completion signal and storing keys. Verify a hit on the first poll.
    let mut lookup = std::pin::pin!(fixture.verifier.verify(&rotated));
    assert!(matches!(
        futures_util::poll!(&mut lookup),
        std::task::Poll::Ready(Ok(_))
    ));
    assert_eq!(fixture.requests(), 2);
}

#[tokio::test(start_paused = true)]
async fn failed_cold_refresh_joiners_keep_throttled_outcome() {
    let fixture = Fixture::new().await;
    *fixture.status.write().await = StatusCode::SERVICE_UNAVAILABLE;
    fixture.gate.hold();
    let verifier = fixture.verifier.clone();
    let starter = tokio::spawn(async move { verifier.verify(&forged_token("unknown-1")).await });
    fixture.gate.wait_for_entry().await;
    let unknown = forged_token("unknown-2");
    let mut joiner = std::pin::pin!(fixture.verifier.verify(&unknown));
    assert!(futures_util::poll!(&mut joiner).is_pending());
    fixture.gate.release();
    assert_eq!(
        timeout(Duration::from_secs(1), starter)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err(),
        VerifyError::Unavailable("jwks_status")
    );
    assert_eq!(
        timeout(Duration::from_secs(1), joiner)
            .await
            .unwrap()
            .unwrap_err(),
        VerifyError::Unavailable("jwks_refresh_throttled")
    );
    assert_eq!(fixture.requests(), 1);
}

#[tokio::test(start_paused = true)]
async fn dropped_refresh_sender_maps_to_unavailable_and_can_recover() {
    struct PanicOnce(AtomicBool);
    impl Fetcher for PanicOnce {
        fn fetch<'a>(&'a self, _: &'a str, _: usize) -> FetchFuture<'a> {
            Box::pin(async move {
                assert!(
                    !self.0.swap(false, Ordering::SeqCst),
                    "simulate a lost refresh task"
                );
                Ok(key_set().to_string().into_bytes())
            })
        }
    }
    let verifier = Verifier {
        config: config("https://nyx.example/jwks".into()),
        cache: Cache::with_fetcher(
            signing_keys::Config::new(CACHE_TTL, 1),
            KeySet::parse,
            transport_error,
            Arc::new(PanicOnce(AtomicBool::new(true))),
        ),
    };
    let valid = Signer::primary().token(&claims(now()), "key-1");
    assert_eq!(
        verifier.verify(&valid).await.unwrap_err(),
        VerifyError::Unavailable("jwks_refresh")
    );
    assert_eq!(
        verifier.verify(&valid).await.unwrap_err(),
        VerifyError::Unavailable("jwks_refresh_throttled")
    );
    tokio::time::advance(Duration::from_secs(5)).await;
    verifier.verify(&valid).await.unwrap();
}

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
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
    let fixture = Fixture::new_http().await;
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

#[tokio::test(start_paused = true)]
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
