//! RS256-only verification; token headers never select a key endpoint.
use super::{payload::identifier, VerificationError};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::signature::{RsaPublicKeyComponents, RSA_PKCS1_2048_8192_SHA256};
use serde::{de::DeserializeOwned, Deserialize};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    alg: String,
    typ: String,
    kid: String,
}
pub(super) struct Token<'a> {
    pub kid: String,
    signed: &'a [u8],
    signature: Vec<u8>,
    body: Vec<u8>,
}
impl<'a> Token<'a> {
    pub fn parse(token: &'a str) -> Result<Self, VerificationError> {
        if token.is_empty() || token.len() > 16_384 {
            return Err(VerificationError("token_bounds"));
        }
        let parts: Vec<_> = token.split('.').collect();
        if parts.len() != 3 || parts[0].len() > 1024 {
            return Err(VerificationError("token_format"));
        }
        let header: Header = serde_json::from_slice(&decode(parts[0])?)
            .map_err(|_| VerificationError("token_header"))?;
        if header.alg != "RS256" {
            return Err(VerificationError("token_algorithm"));
        }
        if header.typ != "JWT" || !identifier(&header.kid) {
            return Err(VerificationError("token_header"));
        }
        Ok(Self {
            kid: header.kid,
            signed: &token.as_bytes()[..token.len() - parts[2].len() - 1],
            signature: decode(parts[2])?,
            body: decode(parts[1])?,
        })
    }
}
fn decode(value: &str) -> Result<Vec<u8>, VerificationError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| VerificationError("token_encoding"))
}
#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}
#[derive(Deserialize)]
struct Jwk {
    kty: String,
    kid: String,
    alg: Option<String>,
    #[serde(rename = "use")]
    purpose: Option<String>,
    n: String,
    e: String,
}
struct RsaKey {
    n: Vec<u8>,
    e: Vec<u8>,
}
pub(super) struct KeySet {
    keys: BTreeMap<String, RsaKey>,
}
impl KeySet {
    pub fn parse(bytes: &[u8]) -> Result<Self, VerificationError> {
        if bytes.len() > 32_768 {
            return Err(VerificationError("jwks_bounds"));
        }
        let jwks: Jwks =
            serde_json::from_slice(bytes).map_err(|_| VerificationError("jwks_invalid"))?;
        if jwks.keys.is_empty() || jwks.keys.len() > 16 {
            return Err(VerificationError("jwks_bounds"));
        }
        let mut keys = BTreeMap::new();
        for jwk in jwks.keys {
            if jwk.kty != "RSA"
                || jwk.alg.as_deref().is_some_and(|alg| alg != "RS256")
                || jwk
                    .purpose
                    .as_deref()
                    .is_some_and(|purpose| purpose != "sig")
                || !identifier(&jwk.kid)
            {
                return Err(VerificationError("jwks_invalid"));
            }
            let n = decode(&jwk.n).map_err(|_| VerificationError("jwks_invalid"))?;
            let e = decode(&jwk.e).map_err(|_| VerificationError("jwks_invalid"))?;
            if !(256..=1024).contains(&n.len())
                || !(1..=8).contains(&e.len())
                || n[0] == 0
                || e[0] == 0
                || keys.insert(jwk.kid, RsaKey { n, e }).is_some()
            {
                return Err(VerificationError("jwks_invalid"));
            }
        }
        Ok(Self { keys })
    }
    pub fn contains(&self, kid: &str) -> bool {
        self.keys.contains_key(kid)
    }
    pub fn verify<T: DeserializeOwned>(&self, token: &Token<'_>) -> Result<T, VerificationError> {
        let key = self
            .keys
            .get(&token.kid)
            .ok_or(VerificationError("signing_key_unknown"))?;
        RsaPublicKeyComponents {
            n: &key.n,
            e: &key.e,
        }
        .verify(&RSA_PKCS1_2048_8192_SHA256, token.signed, &token.signature)
        .map_err(|_| VerificationError("token_signature"))?;
        serde_json::from_slice(&token.body).map_err(|_| VerificationError("token_claims"))
    }
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
        issuer: &str,
        audience: &str,
        kind: &str,
        ttl: i64,
        now: i64,
    ) -> Result<(), VerificationError> {
        if self.iss != issuer {
            return Err(VerificationError("token_issuer"));
        }
        if self.aud != audience {
            return Err(VerificationError("token_audience"));
        }
        if self.token_type != kind {
            return Err(VerificationError("token_type"));
        }
        if !identifier(&self.jti) {
            return Err(VerificationError("token_id"));
        }
        if self.iat < 0
            || now < 0
            || self.exp <= self.iat
            || self.exp.saturating_sub(self.iat) > ttl
            || self.exp.saturating_add(60) <= now
            || self.iat > now.saturating_add(60)
            || self
                .nbf
                .is_some_and(|nbf| nbf < 0 || nbf > now.saturating_add(60) || nbf >= self.exp)
        {
            return Err(VerificationError("token_time"));
        }
        Ok(())
    }
}
