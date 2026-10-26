use super::*;
use serde_json::json;
use std::sync::Mutex;

fn origin() -> Url {
    Url::parse("https://nyx.example").unwrap()
}

fn document() -> Value {
    json!({
        "issuer": "https://nyx.example",
        "authorization_endpoint": "https://nyx.example/oauth/authorize",
        "token_endpoint": "https://nyx.example/oauth/token",
        "revocation_endpoint": "https://nyx.example/oauth/revoke",
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post", "none"]
    })
}

#[test]
fn a_complete_document_on_the_nyxid_origin_enables_the_console() {
    let mut post_only = document();
    post_only["token_endpoint_auth_methods_supported"] = json!(["client_secret_post"]);
    let endpoints = validate(&post_only, &origin()).unwrap();
    assert_eq!(
        endpoints.authorization.as_str(),
        "https://nyx.example/oauth/authorize"
    );
    assert_eq!(endpoints.token.as_str(), "https://nyx.example/oauth/token");
    assert_eq!(
        endpoints.revocation.as_str(),
        "https://nyx.example/oauth/revoke"
    );
}

#[test]
fn every_mismatch_disables_the_console() {
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "code_challenge_methods_supported",
            json!(["plain"]),
            "code_challenge_method",
        ),
        (
            "token_endpoint_auth_methods_supported",
            json!(["client_secret_basic", "none"]),
            "token_endpoint_auth_method",
        ),
        (
            "authorization_endpoint",
            json!("http://nyx.example/oauth/authorize"),
            "authorization_endpoint",
        ),
        (
            "token_endpoint",
            json!("https://evil.example/oauth/token"),
            "token_endpoint",
        ),
        (
            "token_endpoint",
            json!("https://nyx.example:8443/oauth/token"),
            "token_endpoint",
        ),
        (
            "revocation_endpoint",
            json!("https://user:pass@nyx.example/oauth/revoke"),
            "revocation_endpoint",
        ),
        (
            "authorization_endpoint",
            json!("https://nyx.example/oauth/authorize#fragment"),
            "authorization_endpoint",
        ),
        ("revocation_endpoint", Value::Null, "revocation_endpoint"),
        ("authorization_endpoint", json!(7), "authorization_endpoint"),
    ];
    for (field, value, reason) in cases {
        let mut broken = document();
        broken[field] = value.clone();
        assert_eq!(
            validate(&broken, &origin()),
            Err(DiscoveryError::Mismatch(reason)),
            "{field}={value}"
        );
    }
    let mut missing = document();
    missing
        .as_object_mut()
        .unwrap()
        .remove("code_challenge_methods_supported");
    assert_eq!(
        validate(&missing, &origin()),
        Err(DiscoveryError::Mismatch("code_challenge_method"))
    );
}

#[test]
fn plain_http_passes_only_on_a_loopback_nyxid_origin() {
    let local = Url::parse("http://127.0.0.1:4000").unwrap();
    let mut document = document();
    for field in [
        "authorization_endpoint",
        "token_endpoint",
        "revocation_endpoint",
    ] {
        document[field] = json!(format!("http://127.0.0.1:4000/oauth/{field}"));
    }
    assert!(validate(&document, &local).is_ok());
    let cluster = Url::parse("http://nyxid.chrono-bot.svc").unwrap();
    for field in [
        "authorization_endpoint",
        "token_endpoint",
        "revocation_endpoint",
    ] {
        document[field] = json!(format!("http://nyxid.chrono-bot.svc/oauth/{field}"));
    }
    assert_eq!(
        validate(&document, &cluster),
        Err(DiscoveryError::Mismatch("authorization_endpoint"))
    );
}

#[test]
fn retries_start_at_30_seconds_and_double_to_five_minutes() {
    let delays: Vec<u64> = (1..=8).map(|n| retry_delay(n).as_secs()).collect();
    assert_eq!(delays, [30, 60, 120, 240, 300, 300, 300, 300]);
    assert_eq!(retry_delay(u32::MAX), MAX_RETRY);
}

#[derive(Default)]
struct Recorder {
    published: Mutex<Vec<bool>>,
}

impl DiscoverySink for Recorder {
    fn publish(&self, endpoints: Option<Arc<OidcEndpoints>>) {
        self.published.lock().unwrap().push(endpoints.is_some());
    }
    fn current(&self) -> bool {
        self.published.lock().unwrap().last() == Some(&true)
    }
}

#[tokio::test(start_paused = true)]
async fn failures_back_off_and_a_success_is_kept_for_an_hour() {
    let started = tokio::time::Instant::now();
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::new(Recorder::default());
    let outcomes = Arc::new(Mutex::new(vec![
        Err(DiscoveryError::Unavailable),
        Err(DiscoveryError::Unavailable),
        Err(DiscoveryError::Mismatch("token_endpoint")),
        Ok(validate(&document(), &origin()).unwrap()),
        Err(DiscoveryError::Unavailable),
    ]));
    let task = tokio::spawn(run(recorder.clone(), {
        let attempts = attempts.clone();
        move || {
            let attempts = attempts.clone();
            let outcomes = outcomes.clone();
            async move {
                attempts.lock().unwrap().push(started.elapsed().as_secs());
                let mut outcomes = outcomes.lock().unwrap();
                if outcomes.is_empty() {
                    Err(DiscoveryError::Unavailable)
                } else {
                    outcomes.remove(0)
                }
            }
        }
    }));
    tokio::time::sleep(Duration::from_secs(3_600 + 210 + 15)).await;
    task.abort();
    assert_eq!(*attempts.lock().unwrap(), [0, 30, 90, 210, 3_810]);
    // The mismatch withdrew the endpoints, the success published them, and
    // the later outage kept them.
    assert_eq!(*recorder.published.lock().unwrap(), [false, true]);
}
