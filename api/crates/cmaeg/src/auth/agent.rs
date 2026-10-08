//! Agent bearer authentication is local; channel ownership is proved by its retained hash.
use crate::{channels::resolver::ResolvedChannel, http::problems::problem};
use axum::{
    extract::FromRequestParts,
    http::{request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use cmaeg_core::problem::{Problem, ProblemCode};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
#[derive(Clone, Copy)]
pub struct AgentCaller {
    pub key_hash: [u8; 32],
}
impl AgentCaller {
    pub fn from_headers(headers: &HeaderMap) -> Result<Self, AgentAuthError> {
        let mut values = headers.get_all("authorization").iter();
        let value = values.next().ok_or(AgentAuthError::MissingOrInvalid)?;
        if values.next().is_some() {
            return Err(AgentAuthError::MissingOrInvalid);
        }
        let (scheme, key) = value
            .to_str()
            .ok()
            .and_then(|value| value.split_once(' '))
            .ok_or(AgentAuthError::MissingOrInvalid)?;
        if !scheme.eq_ignore_ascii_case("Bearer") || !valid_key(key) {
            return Err(AgentAuthError::MissingOrInvalid);
        }
        Ok(Self {
            key_hash: Sha256::digest(key.as_bytes()).into(),
        })
    }
    /// The caller holds the agent key of `channel`'s default consumer, the
    /// only consumer a tool call can be made as today.
    pub fn authorize(&self, channel: &ResolvedChannel) -> Result<(), AgentAuthError> {
        channel
            .consumer_key_hash(None)
            .is_some_and(|expected| self.matches(expected))
            .then_some(())
            .ok_or(AgentAuthError::Forbidden)
    }
    pub fn matches(&self, expected: &[u8; 32]) -> bool {
        bool::from(self.key_hash.ct_eq(expected))
    }
}
fn valid_key(key: &str) -> bool {
    key.strip_prefix("nyxid_ag_")
        .or_else(|| key.strip_prefix("nyx_"))
        .is_some_and(|suffix| {
            (16..=128).contains(&suffix.len())
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}
#[derive(Debug, Clone, Copy)]
pub enum AgentAuthError {
    MissingOrInvalid,
    Forbidden,
}
impl AgentAuthError {
    pub fn problem(self) -> Problem {
        match self {
            Self::MissingOrInvalid => Problem::new(
                ProblemCode::NotAuthenticated,
                401,
                "a valid agent bearer is required",
            ),
            Self::Forbidden => Problem::new(
                ProblemCode::Forbidden,
                403,
                "agent key does not authorize this channel",
            ),
        }
    }
}
impl IntoResponse for AgentAuthError {
    fn into_response(self) -> Response {
        let error = self.problem();
        problem(
            StatusCode::from_u16(error.status).unwrap(),
            error.code,
            error.detail.unwrap_or_default(),
        )
    }
}
impl<S: Send + Sync> FromRequestParts<S> for AgentCaller {
    type Rejection = AgentAuthError;
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Self::from_headers(&parts.headers)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_one_bounded_agent_bearer_and_hashes_exact_bytes() {
        let mut headers = HeaderMap::new();
        assert!(AgentCaller::from_headers(&headers).is_err());
        for prefix in ["nyx_", "nyxid_ag_"] {
            let key = format!("{prefix}{}", uuid::Uuid::new_v4().simple());
            headers.insert("authorization", format!("Bearer {key}").parse().unwrap());
            let caller = AgentCaller::from_headers(&headers).unwrap();
            let expected: [u8; 32] = Sha256::digest(key.as_bytes()).into();
            assert!(caller.matches(&expected));
            assert!(!caller.matches(&[0; 32]));
            headers.append("authorization", "Bearer duplicate".parse().unwrap());
            assert!(AgentCaller::from_headers(&headers).is_err());
            headers.clear();
        }
        for key in [
            "short".into(),
            format!("nyx_{}", "a".repeat(15)),
            format!("nyx_{}", "a".repeat(129)),
            format!("nyx_{}!", "a".repeat(16)),
            format!("other_{}", "a".repeat(16)),
        ] {
            headers.insert("authorization", format!("Bearer {key}").parse().unwrap());
            assert!(AgentCaller::from_headers(&headers).is_err());
        }
    }
}
