//! Verify NyxID's existing service identity assertion against configured trust.
//!
//! This is identity evidence, never an OAuth token or renewable owner custody.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::signature::{RsaPublicKeyComponents, RSA_PKCS1_2048_8192_SHA256};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::Mutex, time::Instant};

use crate::auth::profile::Profile;

pub(crate) const HEADER: &str = "x-nyxid-identity-token";
const MAX_TOKEN_BYTES: usize = 16_384;
const MAX_JWKS_BYTES: usize = 32_768;
const CACHE_TTL: Duration = Duration::from_secs(300);
const REFRESH_COOLDOWN: Duration = Duration::from_secs(5);
const MAX_CLOCK_SKEW: i64 = 5;

#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub issuer: String,
    pub jwks_url: String,
    pub audience: String,
    pub service_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum VerifyError {
    #[error("NyxID service identity rejected: {0}")]
    Rejected(&'static str),
    #[error("NyxID service identity verification unavailable: {0}")]
    Unavailable(&'static str),
}

#[derive(Debug, Clone)]
pub(crate) struct VerifiedIdentity {
    pub issuer: String,
    pub subject: String,
    pub permissions: Vec<String>,
    pub service_id: String,
    pub jti: String,
    pub profile: Profile,
    pub expires_at: SystemTime,
}

/// A repeated header must not allow different HTTP layers to select different
/// identities. Comma-joined values also fail compact-JWT parsing.
pub(crate) fn identity_token(headers: &axum::http::HeaderMap) -> Result<Option<&str>, VerifyError> {
    let mut values = headers.get_all(HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(VerifyError::Rejected("duplicate_identity_header"));
    }
    let value = value
        .to_str()
        .map_err(|_| VerifyError::Rejected("identity_header"))?;
    if value.is_empty() || value.len() > MAX_TOKEN_BYTES {
        return Err(VerifyError::Rejected("token_bounds"));
    }
    Ok(Some(value))
}

pub(crate) struct Verifier {
    config: Config,
    client: reqwest::Client,
    cache: Mutex<Cached>,
}

#[derive(Default)]
struct Cached {
    keys: Option<(Arc<KeySet>, Instant)>,
    next_refresh: Option<Instant>,
}

impl Verifier {
    pub(super) fn trusts(&self, issuer: &str, service_id: &str) -> bool {
        self.config.issuer == issuer && self.config.service_id == service_id
    }

    pub fn new(config: Config) -> Result<Self, VerifyError> {
        validate_endpoint(&config.jwks_url)?;
        validate_endpoint(&config.issuer)?;
        if !identifier(&config.service_id)
            || config.audience.trim().is_empty()
            || config.audience.len() > 2048
            || config.audience.chars().any(char::is_control)
        {
            return Err(VerifyError::Unavailable("trust_configuration"));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| VerifyError::Unavailable("http_client"))?;
        Ok(Self {
            config,
            client,
            cache: Mutex::new(Cached::default()),
        })
    }

    pub async fn verify(&self, raw: &str) -> Result<VerifiedIdentity, VerifyError> {
        let token = Token::parse(raw)?;
        let keys = self.keys_for(&token.kid).await?;
        // Never refresh on a bad signature for a known key identifier.
        let claims = keys.verify(&token)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| VerifyError::Unavailable("clock"))?
            .as_secs();
        let now = i64::try_from(now).map_err(|_| VerifyError::Unavailable("clock"))?;
        claims.validate(&self.config, now)
    }

    async fn keys_for(&self, kid: &str) -> Result<Arc<KeySet>, VerifyError> {
        // One endpoint and one bounded cache per configured verifier. Holding
        // this lock through refresh coalesces concurrent misses and failures.
        let mut cache = self.cache.lock().await;
        let now = Instant::now();
        let current = cache
            .keys
            .as_ref()
            .filter(|(_, until)| *until > now)
            .map(|(keys, _)| keys.clone());
        if let Some(keys) = &current {
            if keys.0.contains_key(kid) {
                return Ok(keys.clone());
            }
        }
        if cache.next_refresh.is_some_and(|after| after > now) {
            return Err(if current.is_some() {
                VerifyError::Rejected("signing_key_unknown")
            } else {
                VerifyError::Unavailable("jwks_refresh_throttled")
            });
        }
        cache.next_refresh = Some(now + REFRESH_COOLDOWN);
        // A failed unknown-key refresh must not evict still-current keys.
        // Expired keys, however, are never used after a fetch failure.
        let keys = Arc::new(self.fetch().await?);
        cache.keys = Some((keys.clone(), Instant::now() + CACHE_TTL));
        if !keys.0.contains_key(kid) {
            return Err(VerifyError::Rejected("signing_key_unknown"));
        }
        Ok(keys)
    }

    async fn fetch(&self) -> Result<KeySet, VerifyError> {
        let mut response = self
            .client
            .get(&self.config.jwks_url)
            .send()
            .await
            .map_err(|_| VerifyError::Unavailable("jwks_request"))?;
        if !response.status().is_success() {
            return Err(VerifyError::Unavailable("jwks_status"));
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_JWKS_BYTES as u64)
        {
            return Err(VerifyError::Unavailable("jwks_bounds"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| VerifyError::Unavailable("jwks_body"))?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_JWKS_BYTES {
                return Err(VerifyError::Unavailable("jwks_bounds"));
            }
            bytes.extend_from_slice(&chunk);
        }
        KeySet::parse(&bytes)
    }
}

fn validate_endpoint(raw: &str) -> Result<(), VerifyError> {
    let url =
        reqwest::Url::parse(raw).map_err(|_| VerifyError::Unavailable("trust_configuration"))?;
    // Plain HTTP is accepted only in explicitly configured local test servers.
    let loopback = url.host().is_some_and(|host| match host {
        url::Host::Domain(host) => host == "localhost",
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    });
    if raw.trim() != raw
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
    {
        return Err(VerifyError::Unavailable("trust_configuration"));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    alg: String,
    typ: String,
    kid: String,
}

struct Token<'a> {
    kid: String,
    signed: &'a [u8],
    signature: Vec<u8>,
    body: Vec<u8>,
}

impl<'a> Token<'a> {
    fn parse(raw: &'a str) -> Result<Self, VerifyError> {
        if raw.is_empty() || raw.len() > MAX_TOKEN_BYTES {
            return Err(VerifyError::Rejected("token_bounds"));
        }
        let mut parts = raw.split('.');
        let header = parts.next().unwrap_or_default();
        let body = parts.next().ok_or(VerifyError::Rejected("token_format"))?;
        let signature = parts.next().ok_or(VerifyError::Rejected("token_format"))?;
        if parts.next().is_some()
            || header.len() > 1024
            || header.is_empty()
            || body.is_empty()
            || signature.is_empty()
        {
            return Err(VerifyError::Rejected("token_format"));
        }
        let header: Header = serde_json::from_slice(&decode(header)?)
            .map_err(|_| VerifyError::Rejected("token_header"))?;
        if header.alg != "RS256" || header.typ != "JWT" || !identifier(&header.kid) {
            return Err(VerifyError::Rejected("token_header"));
        }
        let signature_bytes = decode(signature)?;
        if !(256..=1024).contains(&signature_bytes.len()) {
            return Err(VerifyError::Rejected("token_signature_bounds"));
        }
        Ok(Self {
            kid: header.kid,
            signed: &raw.as_bytes()[..raw.len() - signature.len() - 1],
            signature: signature_bytes,
            body: decode(body)?,
        })
    }
}

fn decode(value: &str) -> Result<Vec<u8>, VerifyError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| VerifyError::Rejected("token_encoding"))
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.chars().any(|c| c.is_control() || c.is_whitespace())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    sub: String,
    iss: String,
    aud: String,
    exp: i64,
    iat: i64,
    jti: String,
    nyx_service_id: String,
    permissions: Vec<String>,
    nbf: Option<i64>,
    email: Option<String>,
    name: Option<String>,
    // These fields belong to the existing NyxID identity contract. They do
    // not grant CMA capabilities; only the signed permissions are mapped.
    #[serde(rename = "roles")]
    _roles: Option<Vec<String>>,
    #[serde(rename = "groups")]
    _groups: Option<Vec<String>>,
}

impl Claims {
    fn validate(self, config: &Config, now: i64) -> Result<VerifiedIdentity, VerifyError> {
        if self.iss != config.issuer
            || self.aud != config.audience
            || self.nyx_service_id != config.service_id
        {
            return Err(VerifyError::Rejected("token_destination"));
        }
        if !identifier(&self.sub)
            || !identifier(&self.jti)
            || self.permissions.len() > 256
            || self.permissions.iter().any(|p| !identifier(p))
        {
            return Err(VerifyError::Rejected("token_claims"));
        }
        // No expiry grace: callers receive an expiry that is still in the
        // future. Up to five seconds of skew is allowed for iat/nbf only.
        if self.iat < 0
            || now < 0
            || self.exp <= self.iat
            || self.exp.saturating_sub(self.iat) > 60
            || self.exp <= now
            || self.iat > now.saturating_add(MAX_CLOCK_SKEW)
            || self.nbf.is_some_and(|nbf| {
                nbf < 0 || nbf >= self.exp || nbf > now.saturating_add(MAX_CLOCK_SKEW)
            })
        {
            return Err(VerifyError::Rejected("token_time"));
        }
        let clean =
            |value: Option<String>| value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        Ok(VerifiedIdentity {
            issuer: self.iss,
            subject: self.sub,
            permissions: self.permissions,
            service_id: self.nyx_service_id,
            jti: self.jti,
            profile: Profile {
                display_name: clean(self.name),
                email: clean(self.email),
                avatar_url: None,
            },
            expires_at: UNIX_EPOCH + Duration::from_secs(self.exp as u64),
        })
    }
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}
#[derive(Deserialize)]
struct Jwk {
    kty: String,
    kid: String,
    n: String,
    e: String,
    alg: Option<String>,
    #[serde(rename = "use")]
    purpose: Option<String>,
    key_ops: Option<Vec<String>>,
}
struct RsaKey {
    n: Vec<u8>,
    e: Vec<u8>,
}
struct KeySet(BTreeMap<String, RsaKey>);

impl KeySet {
    fn parse(bytes: &[u8]) -> Result<Self, VerifyError> {
        let invalid = || VerifyError::Unavailable("jwks_invalid");
        if bytes.len() > MAX_JWKS_BYTES {
            return Err(VerifyError::Unavailable("jwks_bounds"));
        }
        let jwks: Jwks = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        if jwks.keys.is_empty() || jwks.keys.len() > 16 {
            return Err(invalid());
        }
        let mut keys = BTreeMap::new();
        for key in jwks.keys {
            if key.kty != "RSA"
                || !identifier(&key.kid)
                || key.alg.as_deref().is_some_and(|alg| alg != "RS256")
                || key
                    .purpose
                    .as_deref()
                    .is_some_and(|purpose| purpose != "sig")
                || key
                    .key_ops
                    .as_ref()
                    .is_some_and(|ops| ops.len() != 1 || ops[0] != "verify")
            {
                return Err(invalid());
            }
            let n = decode(&key.n).map_err(|_| invalid())?;
            let e = decode(&key.e).map_err(|_| invalid())?;
            if !(256..=1024).contains(&n.len())
                || !(1..=8).contains(&e.len())
                || n[0] == 0
                || e[0] == 0
                || keys.insert(key.kid, RsaKey { n, e }).is_some()
            {
                return Err(invalid());
            }
        }
        Ok(Self(keys))
    }

    fn verify(&self, token: &Token<'_>) -> Result<Claims, VerifyError> {
        let key = self
            .0
            .get(&token.kid)
            .ok_or(VerifyError::Rejected("signing_key_unknown"))?;
        RsaPublicKeyComponents {
            n: &key.n,
            e: &key.e,
        }
        .verify(&RSA_PKCS1_2048_8192_SHA256, token.signed, &token.signature)
        .map_err(|_| VerifyError::Rejected("token_signature"))?;
        serde_json::from_slice(&token.body).map_err(|_| VerifyError::Rejected("token_claims"))
    }
}

#[cfg(test)]
#[path = "jwt_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) use tests::Signer;
