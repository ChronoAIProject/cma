//! The NyxID side of console sign-in: discovery, the authorization and token
//! endpoints, revocation, `users/me` for console access tokens, the
//! organization list and organization authorization. Every behaviour a test
//! needs is a knob on [`OAuthState`].
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Form, Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub const CLIENT_ID: &str = "console-fixture-client";
pub const CLIENT_SECRET: &str = "console-fixture-client-secret";

/// One token endpoint request, as the console sent it.
#[derive(Debug, Clone)]
pub struct TokenCall {
    pub grant_type: String,
    pub code_verifier: Option<String>,
    pub redirect_uri: Option<String>,
    pub refresh_token: Option<String>,
    pub client_authenticated: bool,
}

/// organization id -> (display name, subject -> role)
pub type Orgs = BTreeMap<String, (String, HashMap<String, String>)>;

struct CodeGrant {
    subject: String,
    challenge: String,
    redirect_uri: String,
}

pub struct OAuthState {
    pub base_url: Mutex<String>,
    /// Replaces the advertised discovery document when set.
    pub discovery: Mutex<Option<serde_json::Value>>,
    pub discovery_status: AtomicU16,
    /// The subject `/oauth/authorize` approves.
    pub login_subject: Mutex<String>,
    /// A status the token endpoint answers instead of issuing tokens, with
    /// `token_error` as its OAuth error code.
    pub token_status: AtomicU16,
    pub token_error: Mutex<String>,
    pub token_malformed: AtomicBool,
    pub token_delay_ms: AtomicU64,
    pub expires_in: AtomicU64,
    pub token_calls: Mutex<Vec<TokenCall>>,
    /// Token requests in flight, and the most seen at once.
    pub token_inflight: AtomicUsize,
    pub token_peak: AtomicUsize,
    pub revoked: Mutex<Vec<String>>,
    pub revoke_delay_ms: AtomicU64,
    /// `users/me`, organization reads and authorization answer 503.
    pub down: AtomicBool,
    /// Subjects whose `users/me` hangs for 30 s.
    pub hang: Mutex<HashSet<String>>,
    /// Subjects `users/me` answers 401 for.
    pub deactivated: Mutex<HashSet<String>>,
    pub orgs: Mutex<Orgs>,
    pub users_me_calls: AtomicUsize,
    pub org_list_calls: AtomicUsize,
    pub authorization_calls: AtomicUsize,
    codes: Mutex<HashMap<String, CodeGrant>>,
    access: Mutex<HashMap<String, String>>,
    refresh: Mutex<HashMap<String, String>>,
    sequence: AtomicU64,
}

impl Default for OAuthState {
    fn default() -> Self {
        Self {
            base_url: Mutex::new(String::new()),
            discovery: Mutex::new(None),
            discovery_status: AtomicU16::new(200),
            login_subject: Mutex::new("alice".into()),
            token_status: AtomicU16::new(200),
            token_error: Mutex::new("invalid_grant".into()),
            token_malformed: AtomicBool::new(false),
            token_delay_ms: AtomicU64::new(0),
            expires_in: AtomicU64::new(900),
            token_calls: Mutex::new(Vec::new()),
            token_inflight: AtomicUsize::new(0),
            token_peak: AtomicUsize::new(0),
            revoked: Mutex::new(Vec::new()),
            revoke_delay_ms: AtomicU64::new(0),
            down: AtomicBool::new(false),
            hang: Mutex::new(HashSet::new()),
            deactivated: Mutex::new(HashSet::new()),
            orgs: Mutex::new(BTreeMap::new()),
            users_me_calls: AtomicUsize::new(0),
            org_list_calls: AtomicUsize::new(0),
            authorization_calls: AtomicUsize::new(0),
            codes: Mutex::new(HashMap::new()),
            access: Mutex::new(HashMap::new()),
            refresh: Mutex::new(HashMap::new()),
            sequence: AtomicU64::new(0),
        }
    }
}

impl OAuthState {
    /// Make `subject` hold `role` in organization `id`.
    pub fn grant(&self, id: &str, name: &str, subject: &str, role: &str) {
        let mut orgs = self.orgs.lock().unwrap();
        let entry = orgs
            .entry(id.to_owned())
            .or_insert_with(|| (name.to_owned(), HashMap::new()));
        entry.1.insert(subject.to_owned(), role.to_owned());
    }

    pub fn remove_org(&self, id: &str) {
        self.orgs.lock().unwrap().remove(id);
    }

    /// Every token value the fake issued, for leak scans.
    pub fn issued_tokens(&self) -> Vec<String> {
        let mut tokens: Vec<String> = self.access.lock().unwrap().keys().cloned().collect();
        tokens.extend(self.refresh.lock().unwrap().keys().cloned());
        tokens.extend(self.codes.lock().unwrap().keys().cloned());
        tokens
    }

    /// An access and refresh token for `subject`, as a sign-in issues them.
    pub fn issue_tokens(&self, subject: &str) -> (String, String) {
        let issued = self.issue(subject);
        (
            issued["access_token"].as_str().unwrap().to_owned(),
            issued["refresh_token"].as_str().unwrap().to_owned(),
        )
    }

    /// How many refresh grants the token endpoint received.
    pub fn refresh_calls(&self) -> usize {
        self.token_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.grant_type == "refresh_token")
            .count()
    }

    /// The access tokens issued to `subject`.
    pub fn access_tokens_of(&self, subject: &str) -> Vec<String> {
        self.access
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, owner)| owner.as_str() == subject)
            .map(|(token, _)| token.clone())
            .collect()
    }

    pub fn subject_of(&self, access: &str) -> Option<String> {
        self.access.lock().unwrap().get(access).cloned()
    }

    fn next(&self, kind: &str) -> String {
        format!(
            "console-{kind}-{}",
            self.sequence.fetch_add(1, Ordering::SeqCst)
        )
    }

    fn issue(&self, subject: &str) -> serde_json::Value {
        let access = self.next("access");
        let refresh = self.next("refresh");
        self.access
            .lock()
            .unwrap()
            .insert(access.clone(), subject.to_owned());
        self.refresh
            .lock()
            .unwrap()
            .insert(refresh.clone(), subject.to_owned());
        serde_json::json!({"access_token": access, "token_type": "Bearer",
            "expires_in": self.expires_in.load(Ordering::SeqCst),
            "refresh_token": refresh, "id_token": "console-id-token-unread", "scope": "openid profile"})
    }

    fn bearer_subject(&self, headers: &HeaderMap) -> Option<String> {
        let token = headers
            .get("authorization")?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")?;
        self.subject_of(token)
    }
}

pub async fn discovery(State(state): State<Arc<OAuthState>>) -> Response {
    let status = StatusCode::from_u16(state.discovery_status.load(Ordering::SeqCst)).unwrap();
    if status != StatusCode::OK {
        return status.into_response();
    }
    if let Some(document) = state.discovery.lock().unwrap().clone() {
        return Json(document).into_response();
    }
    let base = state.base_url.lock().unwrap().clone();
    Json(serde_json::json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "revocation_endpoint": format!("{base}/oauth/revoke"),
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post", "none"]
    }))
    .into_response()
}

/// Approve at once for `login_subject` and send the browser back.
pub async fn authorize(
    State(state): State<Arc<OAuthState>>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["client_id"], CLIENT_ID);
    assert_eq!(query["scope"], "openid profile");
    assert_eq!(query["code_challenge_method"], "S256");
    assert!(!query.contains_key("resource"));
    let code = state.next("code");
    state.codes.lock().unwrap().insert(
        code.clone(),
        CodeGrant {
            subject: state.login_subject.lock().unwrap().clone(),
            challenge: query["code_challenge"].clone(),
            redirect_uri: query["redirect_uri"].clone(),
        },
    );
    let mut target = reqwest::Url::parse(&query["redirect_uri"]).unwrap();
    target
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", &query["state"]);
    Redirect::to(target.as_str()).into_response()
}

fn client_authenticated(headers: &HeaderMap) -> bool {
    let expected =
        base64::engine::general_purpose::STANDARD.encode(format!("{CLIENT_ID}:{CLIENT_SECRET}"));
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        == Some(format!("Basic {expected}").as_str())
}

/// The console grants of `/oauth/token`.
pub async fn token(
    state: Arc<OAuthState>,
    headers: HeaderMap,
    form: BTreeMap<String, String>,
) -> Response {
    let call = TokenCall {
        grant_type: form["grant_type"].clone(),
        code_verifier: form.get("code_verifier").cloned(),
        redirect_uri: form.get("redirect_uri").cloned(),
        refresh_token: form.get("refresh_token").cloned(),
        client_authenticated: client_authenticated(&headers),
    };
    state.token_calls.lock().unwrap().push(call.clone());
    let inflight = state.token_inflight.fetch_add(1, Ordering::SeqCst) + 1;
    state.token_peak.fetch_max(inflight, Ordering::SeqCst);
    let delay = state.token_delay_ms.load(Ordering::SeqCst);
    if delay > 0 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    state.token_inflight.fetch_sub(1, Ordering::SeqCst);
    let status = StatusCode::from_u16(state.token_status.load(Ordering::SeqCst)).unwrap();
    if status != StatusCode::OK {
        let error = state.token_error.lock().unwrap().clone();
        return (status, Json(serde_json::json!({"error": error}))).into_response();
    }
    if state.token_malformed.load(Ordering::SeqCst) {
        return (StatusCode::OK, "not json").into_response();
    }
    if !call.client_authenticated {
        return oauth_error(StatusCode::UNAUTHORIZED, "invalid_client");
    }
    let subject = match call.grant_type.as_str() {
        "authorization_code" => {
            let Some(grant) = state.codes.lock().unwrap().remove(&form["code"]) else {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant");
            };
            let challenge =
                URL_SAFE_NO_PAD.encode(Sha256::digest(call.code_verifier.unwrap_or_default()));
            if challenge != grant.challenge
                || call.redirect_uri.as_ref() != Some(&grant.redirect_uri)
            {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant");
            }
            grant.subject
        }
        "refresh_token" => {
            // Rotation: a refresh token works once.
            let Some(subject) = state.refresh.lock().unwrap().remove(&form["refresh_token"]) else {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant");
            };
            subject
        }
        _ => return oauth_error(StatusCode::BAD_REQUEST, "unsupported_grant_type"),
    };
    Json(state.issue(&subject)).into_response()
}

fn oauth_error(status: StatusCode, error: &str) -> Response {
    (status, Json(serde_json::json!({"error": error}))).into_response()
}

pub async fn revoke(
    State(state): State<Arc<OAuthState>>,
    headers: HeaderMap,
    Form(form): Form<BTreeMap<String, String>>,
) -> StatusCode {
    let delay = state.revoke_delay_ms.load(Ordering::SeqCst);
    if delay > 0 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    if client_authenticated(&headers) {
        let token = form["token"].clone();
        state.refresh.lock().unwrap().remove(&token);
        state.revoked.lock().unwrap().push(token);
    }
    StatusCode::OK
}

/// `users/me` for a console access token; `None` when the bearer is not one.
pub async fn users_me(state: &OAuthState, headers: &HeaderMap) -> Option<Response> {
    let subject = state.bearer_subject(headers)?;
    state.users_me_calls.fetch_add(1, Ordering::SeqCst);
    if state.hang.lock().unwrap().contains(&subject) {
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
    if state.down.load(Ordering::SeqCst) {
        return Some(StatusCode::SERVICE_UNAVAILABLE.into_response());
    }
    if state.deactivated.lock().unwrap().contains(&subject) {
        return Some(StatusCode::UNAUTHORIZED.into_response());
    }
    Some(Json(serde_json::json!({"id": subject, "user_type": "person"})).into_response())
}

pub async fn org_list(State(state): State<Arc<OAuthState>>, headers: HeaderMap) -> Response {
    state.org_list_calls.fetch_add(1, Ordering::SeqCst);
    if state.down.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    // Other fixtures' bearers keep the 404 this route answered before it
    // existed.
    let Some(subject) = state.bearer_subject(&headers) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let orgs: Vec<_> = state
        .orgs
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(id, (name, members))| {
            members.get(&subject).map(|role| {
                serde_json::json!({"id": id, "slug": id, "display_name": name, "your_role": role})
            })
        })
        .collect();
    Json(serde_json::json!({ "orgs": orgs })).into_response()
}

pub async fn org_authorization(
    State(state): State<Arc<OAuthState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    state.authorization_calls.fetch_add(1, Ordering::SeqCst);
    if state.down.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    // Other fixtures' bearers keep the 404 this route answered before it
    // existed: their subjects are then persons, not organizations.
    let Some(subject) = state.bearer_subject(&headers) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let role = state
        .orgs
        .lock()
        .unwrap()
        .get(&id)
        .and_then(|(_, members)| members.get(&subject).cloned());
    match role {
        Some(role) => Json(serde_json::json!({"id": id, "your_role": role})).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
