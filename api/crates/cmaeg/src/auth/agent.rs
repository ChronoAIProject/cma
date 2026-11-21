//! Agent bearer authentication is local; channel ownership is proved by its retained hash.
use crate::{channels::resolver::ResolvedChannel, http::problems::problem};
use axum::{
    extract::FromRequestParts,
    http::{request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use cmaeg_core::{
    ids::ConsumerId,
    problem::{Problem, ProblemCode},
};
use sha2::{Digest, Sha256};
use subtle::{Choice, ConditionallySelectable, ConstantTimeEq};
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
    /// The consumer whose key this caller holds. Compares against every consumer hash in
    /// constant time without an early exit; keys are unique per channel, so at most one matches.
    pub fn consumer(&self, channel: &ResolvedChannel) -> Result<ConsumerId, AgentAuthError> {
        let default = ConsumerId::default_consumer();
        // The default's hash is read from `agent_key_hash`, its authority, so a
        // channel restored without a `default` entry in `consumer_keys` still
        // authorizes it, and one with such an entry is not compared twice.
        let candidates = std::iter::once((&default, &channel.agent_key_hash)).chain(
            channel
                .consumer_keys
                .iter()
                .filter(|(id, _)| !id.is_default())
                .map(|(id, hash)| (id, hash)),
        );
        self.select(candidates).ok_or(AgentAuthError::Forbidden)
    }
    /// The id of the candidate whose hash is the caller's. Every candidate is
    /// compared, and the match is recorded with a constant-time select rather
    /// than a branch, so the time taken reveals neither whether nor where a
    /// key matched.
    fn select<'a>(
        &self,
        candidates: impl Iterator<Item = (&'a ConsumerId, &'a [u8; 32])>,
    ) -> Option<ConsumerId> {
        let mut found = Choice::from(0);
        let mut chosen = 0_u32;
        let mut ids = Vec::new();
        for (index, (id, hash)) in (0_u32..).zip(candidates) {
            let matched = self.key_hash.ct_eq(hash);
            chosen.conditional_assign(&index, matched);
            found |= matched;
            ids.push(id);
        }
        // Only the selected id is read data-dependently, after every comparison.
        bool::from(found).then(|| ids[chosen as usize].clone())
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
    fn caller(key: &str) -> AgentCaller {
        AgentCaller {
            key_hash: Sha256::digest(key.as_bytes()).into(),
        }
    }
    fn consumer(id: &str) -> ConsumerId {
        ConsumerId::parse(id).unwrap()
    }
    #[test]
    fn consumer_selection_visits_every_hash_even_after_a_match() {
        let caller = caller("nyx_selection_fixture_key");
        let ids = [
            ConsumerId::default_consumer(),
            consumer("csm_00000000000a"),
            consumer("csm_00000000000b"),
        ];
        for position in 0..ids.len() {
            let mut hashes = [[1; 32], [2; 32], [3; 32]];
            hashes[position] = caller.key_hash;
            let mut visited = 0;
            let selected = caller.select(ids.iter().zip(hashes.iter()).inspect(|_| visited += 1));
            assert_eq!(selected.as_ref(), Some(&ids[position]));
            assert_eq!(visited, ids.len(), "a match at {position} ended the scan");
        }
        let mut visited = 0;
        let unknown = [[1; 32], [2; 32], [3; 32]];
        let selected = caller.select(ids.iter().zip(unknown.iter()).inspect(|_| visited += 1));
        assert_eq!(selected, None);
        assert_eq!(visited, ids.len());
    }
    fn channel(consumer_keys: Vec<(ConsumerId, [u8; 32])>) -> ResolvedChannel {
        let definition: cmaeg_core::channel::ChannelDefinition = serde_json::from_str(
            include_str!("../../../cmaeg-core/tests/fixtures/channel.json"),
        )
        .unwrap();
        ResolvedChannel {
            id: cmaeg_core::ids::ChannelId::new("cmaeg1.ch.agent-fixture").unwrap(),
            pointer: cmaeg_core::seal::ChannelPointer {
                v: 1,
                record_id: definition.record_id.clone(),
                record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
            },
            definition,
            agent_key_hash: caller("nyx_default_fixture_key").key_hash,
            consumer_keys,
            loaded_at: std::time::Instant::now(),
            version: 1,
        }
    }
    #[test]
    fn a_caller_is_the_one_consumer_whose_key_it_holds() {
        let default = caller("nyx_default_fixture_key");
        let reviewer = caller("nyx_reviewer_fixture_key");
        let routed = channel(vec![
            (ConsumerId::default_consumer(), default.key_hash),
            (consumer("csm_00000000000a"), reviewer.key_hash),
        ]);
        assert!(default.consumer(&routed).unwrap().is_default());
        assert_eq!(
            reviewer.consumer(&routed).unwrap(),
            consumer("csm_00000000000a")
        );
        assert!(matches!(
            caller("nyx_unknown_fixture_key").consumer(&routed),
            Err(AgentAuthError::Forbidden)
        ));
        // Without routing only the default exists, read from its authority.
        let legacy = channel(Vec::new());
        assert!(default.consumer(&legacy).unwrap().is_default());
        assert!(matches!(
            reviewer.consumer(&legacy),
            Err(AgentAuthError::Forbidden)
        ));
    }
}
