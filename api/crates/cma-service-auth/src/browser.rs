//! Server-held, encrypted sessions. Cookies contain only random opaque handles.
use crate::{AuthError, NyxIdAuth};
use axum::{
    extract::{Query, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, DateTime, Document},
    options::{Acknowledgment, CollectionOptions, IndexOptions, ReadConcern, WriteConcern},
    Collection, Database, IndexModel,
};
use ring::{
    aead, digest,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct BrowserAuth {
    auth: NyxIdAuth,
    rows: Collection<Document>,
    key: Arc<aead::LessSafeKey>,
    origin: String,
    prefix: String,
    landing: String,
    cookie: String,
}
#[derive(Serialize, Deserialize)]
struct Transaction {
    verifier: String,
}
#[derive(Serialize, Deserialize)]
struct Session {
    token: String,
}
#[derive(Deserialize)]
struct Callback {
    code: String,
    state: String,
}
#[derive(Deserialize)]
struct Token {
    access_token: String,
    token_type: String,
}

impl BrowserAuth {
    pub fn new(
        auth: NyxIdAuth,
        db: &Database,
        collection: &str,
        origin: &str,
        prefix: &str,
        landing: &str,
        encryption_key: &[u8],
    ) -> Result<Self, AuthError> {
        let url = url::Url::parse(origin).map_err(|_| AuthError::Configuration)?;
        if url.scheme() != "https"
            || url.origin().ascii_serialization() != origin
            || (collection, prefix, landing)
                != ("eg_auth_sessions", "/api/event-gateway", "/event-gateway")
        {
            return Err(AuthError::Configuration);
        }
        let key = aead::UnboundKey::new(&aead::AES_256_GCM, encryption_key)
            .map_err(|_| AuthError::Configuration)?;
        let options = CollectionOptions::builder()
            .read_concern(ReadConcern::majority())
            .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
            .build();
        Ok(Self {
            cookie: format!("__Host-{}", auth.service.slug()),
            auth,
            rows: db.collection_with_options(collection, options),
            key: Arc::new(aead::LessSafeKey::new(key)),
            origin: origin.into(),
            prefix: prefix.into(),
            landing: landing.into(),
        })
    }
    pub fn routes(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/auth/login", get(login))
            .route("/auth/callback", get(callback))
            .route("/auth/session", get(session))
            .route("/auth/logout", post(logout))
            .with_state(self.clone())
    }
    pub fn protect(self: &Arc<Self>, router: Router) -> Router {
        router.layer(middleware::from_fn_with_state(self.clone(), credentials))
    }
    fn cookie_header(&self, id: &str, transaction: bool, seconds: u64) -> String {
        format!(
            "{}{}={id}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={seconds}",
            self.cookie,
            if transaction { "-login" } else { "" }
        )
    }
    fn cookie_value(
        &self,
        headers: &HeaderMap,
        transaction: bool,
    ) -> Result<Option<String>, AuthError> {
        let name = format!("{}{}", self.cookie, if transaction { "-login" } else { "" });
        let values: Vec<_> = headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|s| s.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .filter(|(k, _)| *k == name)
            .collect();
        if values.len() > 1 {
            return Err(AuthError::Unauthenticated);
        }
        Ok(values.first().map(|(_, v)| v.to_string()).filter(|v| {
            v.len() == 43
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        }))
    }
    fn csrf(&self, headers: &HeaderMap) -> Result<(), AuthError> {
        if headers.get_all(header::ORIGIN).iter().count() != 1
            || headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(&self.origin)
        {
            return Err(AuthError::Forbidden);
        }
        Ok(())
    }
    async fn save(
        &self,
        kind: &str,
        value: &impl Serialize,
        seconds: u64,
    ) -> Result<String, AuthError> {
        let id = random()?;
        let hash = hash(&id);
        let mut bytes = serde_json::to_vec(value).map_err(|_| AuthError::Unavailable)?;
        let mut nonce = [0u8; 12];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| AuthError::Unavailable)?;
        self.key
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(format!("{}:{kind}:{hash}", self.cookie).as_bytes()),
                &mut bytes,
            )
            .map_err(|_| AuthError::Unavailable)?;
        let payload = [nonce.to_vec(), bytes].concat();
        self.rows.insert_one(doc!{"_id":hash,"kind":kind,"payload":Binary{subtype:BinarySubtype::Generic,bytes:payload},"expires_at":DateTime::from_system_time(SystemTime::now()+Duration::from_secs(seconds))}).await.map_err(|_|AuthError::Unavailable)?;
        Ok(id)
    }
    async fn load<T: serde::de::DeserializeOwned>(
        &self,
        id: &str,
        kind: &str,
        consume: bool,
    ) -> Result<T, AuthError> {
        let hash = hash(id);
        let filter = doc! {"_id":&hash,"kind":kind,"expires_at":{"$gt":DateTime::now()}};
        let row = if consume {
            self.rows.find_one_and_delete(filter).await
        } else {
            self.rows.find_one(filter).await
        }
        .map_err(|_| AuthError::Unavailable)?
        .ok_or(AuthError::Unauthenticated)?;
        let payload = row
            .get_binary_generic("payload")
            .map_err(|_| AuthError::Unavailable)?;
        if payload.len() < 28 {
            return Err(AuthError::Unavailable);
        }
        let nonce: [u8; 12] = payload[..12]
            .try_into()
            .map_err(|_| AuthError::Unavailable)?;
        let mut bytes = payload[12..].to_vec();
        let plain = self
            .key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(format!("{}:{kind}:{hash}", self.cookie).as_bytes()),
                &mut bytes,
            )
            .map_err(|_| AuthError::Unauthenticated)?;
        serde_json::from_slice(plain).map_err(|_| AuthError::Unavailable)
    }
    async fn headers(&self, headers: &HeaderMap, method: &Method) -> Result<HeaderMap, AuthError> {
        if headers.contains_key(header::AUTHORIZATION) {
            return Ok(headers.clone());
        }
        let Some(id) = self.cookie_value(headers, false)? else {
            return Ok(headers.clone());
        };
        if !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
            self.csrf(headers)?;
        }
        let value: Session = self.load(&id, "session", false).await?;
        let mut next = headers.clone();
        next.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", value.token))
                .map_err(|_| AuthError::Unauthenticated)?,
        );
        Ok(next)
    }
}
async fn credentials(
    State(auth): State<Arc<BrowserAuth>>,
    mut request: Request,
    next: Next,
) -> Response {
    // OAuth and health routes are independent of an expired browser session.
    if request.uri().path().starts_with("/auth/")
        || matches!(request.uri().path(), "/healthz" | "/readyz")
    {
        return next.run(request).await;
    }
    match auth.headers(request.headers(), request.method()).await {
        Ok(headers) => {
            *request.headers_mut() = headers;
            next.run(request).await
        }
        Err(e) => e.into_response(),
    }
}
async fn login(
    State(auth): State<Arc<BrowserAuth>>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    if headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) == Some("cross-site") {
        return Err(AuthError::Forbidden);
    }
    let verifier = random()?;
    let challenge = URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, verifier.as_bytes()));
    let id = auth
        .save("transaction", &Transaction { verifier }, 300)
        .await?;
    let mut url = url::Url::parse(&format!("{}/oauth/authorize", auth.auth.issuer))
        .map_err(|_| AuthError::Configuration)?;
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", &auth.auth.client_id),
        (
            "redirect_uri",
            &format!("{}{}{}", auth.origin, auth.prefix, "/auth/callback"),
        ),
        ("scope", "openid profile roles"),
        ("resource", &auth.auth.resource),
        ("state", &id),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ]);
    Ok((
        [
            (header::SET_COOKIE, auth.cookie_header(&id, true, 300)),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
        Redirect::to(url.as_str()),
    )
        .into_response())
}
async fn callback(
    State(auth): State<Arc<BrowserAuth>>,
    headers: HeaderMap,
    Query(query): Query<Callback>,
) -> Result<Response, AuthError> {
    if query.code.is_empty()
        || query.code.len() > 4096
        || auth.cookie_value(&headers, true)?.as_deref() != Some(&query.state)
    {
        return Err(AuthError::Unauthenticated);
    }
    let tx: Transaction = auth.load(&query.state, "transaction", true).await?;
    let response = auth
        .auth
        .http
        .post(format!("{}/oauth/token", auth.auth.issuer))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &query.code),
            ("client_id", &auth.auth.client_id),
            ("client_secret", auth.auth.client_secret.as_ref()),
            (
                "redirect_uri",
                &format!("{}{}{}", auth.origin, auth.prefix, "/auth/callback"),
            ),
            ("code_verifier", &tx.verifier),
            ("resource", &auth.auth.resource),
        ])
        .send()
        .await
        .map_err(|_| AuthError::Unavailable)?;
    let token: Token = read_response(response).await?;
    if !token.token_type.eq_ignore_ascii_case("bearer") || token.access_token.len() > 16384 {
        return Err(AuthError::Unauthenticated);
    }
    let mut authorization = HeaderMap::new();
    authorization.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", token.access_token))
            .map_err(|_| AuthError::Unauthenticated)?,
    );
    let principal = auth.auth.authenticate(&authorization).await?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AuthError::Unavailable)?
        .as_secs();
    let ttl = principal.expires_at.saturating_sub(now).min(3600);
    if ttl == 0 {
        return Err(AuthError::Unauthenticated);
    }
    // Rotate and revoke the previous local session, even for an account switch.
    if let Some(old) = auth.cookie_value(&headers, false)? {
        auth.rows
            .delete_one(doc! {"_id":hash(&old),"kind":"session"})
            .await
            .map_err(|_| AuthError::Unavailable)?;
    }
    let id = auth
        .save(
            "session",
            &Session {
                token: token.access_token,
            },
            ttl,
        )
        .await?;
    let mut response = Redirect::to(&auth.landing).into_response();
    for value in [
        auth.cookie_header(&id, false, ttl),
        auth.cookie_header("", true, 0),
    ] {
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&value).map_err(|_| AuthError::Unavailable)?,
        );
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    Ok(response)
}
async fn session(
    State(auth): State<Arc<BrowserAuth>>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    let headers = auth.headers(&headers, &Method::GET).await?;
    let principal = auth.auth.authenticate(&headers).await?;
    Ok(([(header::CACHE_CONTROL,"no-store")],Json(json!({"authenticated":true,"subject":principal.subject,"permissions":principal.permissions,"expires_at":principal.expires_at}))).into_response())
}
async fn logout(
    State(auth): State<Arc<BrowserAuth>>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    auth.csrf(&headers)?;
    if let Some(id) = auth.cookie_value(&headers, false)? {
        auth.rows
            .delete_one(doc! {"_id":hash(&id),"kind":"session"})
            .await
            .map_err(|_| AuthError::Unavailable)?;
    }
    Ok((
        [
            (header::SET_COOKIE, auth.cookie_header("", false, 0)),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
        StatusCode::NO_CONTENT,
    )
        .into_response())
}
async fn read_response<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, AuthError> {
    if !response.status().is_success() {
        return Err(AuthError::Unavailable);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| AuthError::Unavailable)? {
        if bytes.len() + chunk.len() > 65536 {
            return Err(AuthError::Unavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| AuthError::Unavailable)
}
fn random() -> Result<String, AuthError> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| AuthError::Unavailable)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn hash(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, value.as_bytes()))
}
impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        };
        (
            status,
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({"status":status.as_u16(),"detail":self.to_string()})),
        )
            .into_response()
    }
}

pub fn validator() -> Document {
    doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","kind","payload","expires_at"],"properties":{"_id":{"bsonType":"string"},"kind":{"enum":["transaction","session"]},"payload":{"bsonType":"binData"},"expires_at":{"bsonType":"date"}}}}
}
pub async fn migrate(db: &Database, collection: &str) -> Result<(), mongodb::error::Error> {
    cma_service_migrations::collection(db, collection, validator(), true).await?;
    cma_service_migrations::indexes(
        db,
        collection,
        vec![IndexModel::builder()
            .keys(doc! {"expires_at":1})
            .options(IndexOptions::builder().expire_after(Duration::ZERO).build())
            .build()],
    )
    .await?;
    Ok(())
}
