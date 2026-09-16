use std::{collections::BTreeMap, time::Duration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::signature::{RsaPublicKeyComponents, RSA_PKCS1_2048_8192_SHA256};
use serde::{de::DeserializeOwned, Deserialize};

use super::CallbackError;

/// Deployment-pinned issuer and relay audience. No callback can change these
/// values or provide a JWKS URL. TTL values must match the pinned NyxID policy.
#[derive(Clone)]
pub struct CallbackTokenPolicy {
    pub issuer: String,
    pub relay_audience: String,
    pub callback_ttl_seconds: u32,
    pub relay_ttl_seconds: u32,
    pub reply_ttl_seconds: u32,
    pub clock_skew_seconds: u32,
}

impl CallbackTokenPolicy {
    pub(super) fn validate(&self) -> Result<(), CallbackError> {
        for address in [&self.issuer, &self.relay_audience] {
            let url = url::Url::parse(address).map_err(|_| CallbackError::KeysUnavailable)?;
            if address.len() > 2048
                || url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(CallbackError::KeysUnavailable);
            }
        }
        if self.callback_ttl_seconds == 0
            || self.callback_ttl_seconds > 300
            || self.relay_ttl_seconds == 0
            || self.relay_ttl_seconds > 3600
            || self.reply_ttl_seconds == 0
            || self.reply_ttl_seconds > 1800
            || self.clock_skew_seconds > 60
        {
            return Err(CallbackError::KeysUnavailable);
        }
        Ok(())
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
    alg: String,
    #[serde(rename = "use")]
    purpose: String,
    n: String,
    e: String,
}
struct RsaKey {
    modulus: Vec<u8>,
    exponent: Vec<u8>,
}

/// Keys fetched only from the deployment-pinned HTTPS endpoint. Cache expiry
/// fails closed; a missing key never falls back to another key or algorithm.
pub struct CallbackKeySet {
    issuer: String,
    keys: BTreeMap<String, RsaKey>,
    deadline: tokio::time::Instant,
}

impl CallbackKeySet {
    pub fn from_pinned_jwks(
        issuer: String,
        bytes: &[u8],
        deadline: tokio::time::Instant,
    ) -> Result<Self, CallbackError> {
        let now = tokio::time::Instant::now();
        if bytes.len() > 32768 || deadline <= now || deadline > now + Duration::from_secs(300) {
            return Err(CallbackError::KeysUnavailable);
        }
        let jwks: Jwks =
            serde_json::from_slice(bytes).map_err(|_| CallbackError::KeysUnavailable)?;
        if jwks.keys.is_empty() || jwks.keys.len() > 16 {
            return Err(CallbackError::KeysUnavailable);
        }
        let mut keys = BTreeMap::new();
        for jwk in jwks.keys {
            if jwk.kty != "RSA"
                || jwk.alg != "RS256"
                || jwk.purpose != "sig"
                || !super::payload::identifier(&jwk.kid)
            {
                return Err(CallbackError::KeysUnavailable);
            }
            let modulus = decode(&jwk.n).map_err(|_| CallbackError::KeysUnavailable)?;
            let exponent = decode(&jwk.e).map_err(|_| CallbackError::KeysUnavailable)?;
            if !(256..=1024).contains(&modulus.len())
                || !(1..=8).contains(&exponent.len())
                || modulus[0] == 0
                || exponent[0] == 0
                || keys.contains_key(&jwk.kid)
            {
                return Err(CallbackError::KeysUnavailable);
            }
            keys.insert(jwk.kid, RsaKey { modulus, exponent });
        }
        Ok(Self {
            issuer,
            keys,
            deadline,
        })
    }

    pub(super) fn verify<T: DeserializeOwned>(
        &self,
        token: &str,
        issuer: &str,
    ) -> Result<(T, String), CallbackError> {
        if self.issuer != issuer || self.deadline <= tokio::time::Instant::now() {
            return Err(CallbackError::KeysUnavailable);
        }
        if token.len() > 16384 {
            return Err(CallbackError::Bounds);
        }
        let mut segments = token.split('.');
        let header = segments.next().ok_or(CallbackError::Authentication)?;
        let body = segments.next().ok_or(CallbackError::Authentication)?;
        let signature = segments.next().ok_or(CallbackError::Authentication)?;
        if segments.next().is_some() || header.len() > 1024 {
            return Err(CallbackError::Authentication);
        }
        let header: Header =
            serde_json::from_slice(&decode(header)?).map_err(|_| CallbackError::Authentication)?;
        if header.alg != "RS256" || header.typ != "JWT" {
            return Err(CallbackError::Authentication);
        }
        let key = self
            .keys
            .get(&header.kid)
            .ok_or(CallbackError::KeysUnavailable)?;
        let signed_len = token.len() - signature.len() - 1;
        RsaPublicKeyComponents {
            n: &key.modulus,
            e: &key.exponent,
        }
        .verify(
            &RSA_PKCS1_2048_8192_SHA256,
            &token.as_bytes()[..signed_len],
            &decode(signature)?,
        )
        .map_err(|_| CallbackError::Authentication)?;
        let claims =
            serde_json::from_slice(&decode(body)?).map_err(|_| CallbackError::Authentication)?;
        Ok((claims, header.kid))
    }

    pub(super) fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    alg: String,
    typ: String,
    kid: String,
}

fn decode(value: &str) -> Result<Vec<u8>, CallbackError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| CallbackError::Authentication)
}

#[derive(Deserialize)]
pub(super) struct StandardClaims {
    pub iss: String,
    pub aud: String,
    pub iat: i64,
    pub exp: i64,
    pub nbf: Option<i64>,
    pub jti: String,
    pub token_type: String,
}

impl StandardClaims {
    pub fn require(
        &self,
        policy: &CallbackTokenPolicy,
        audience: &str,
        token_type: &str,
        ttl: u32,
        now: i64,
    ) -> Result<(), CallbackError> {
        let skew = i64::from(policy.clock_skew_seconds);
        if self.iss != policy.issuer
            || self.aud != audience
            || self.token_type != token_type
            || !super::payload::identifier(&self.jti)
            || self.iat < 0
            || now < 0
            || self.exp <= self.iat
            || self.exp.saturating_sub(self.iat) > i64::from(ttl) + skew
            || self.exp.saturating_add(skew) <= now
            || self.iat > now.saturating_add(skew)
            || self
                .nbf
                .is_some_and(|nbf| nbf < 0 || nbf > now.saturating_add(skew) || nbf >= self.exp)
        {
            return Err(CallbackError::Authentication);
        }
        Ok(())
    }
}
