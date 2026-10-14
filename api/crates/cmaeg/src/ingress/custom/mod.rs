//! Custom CloudEvents authentication and neutral normalization.
pub mod activity;
pub mod attachments;
pub mod template;
use crate::auth::webhook;
use axum::http::HeaderMap;
use cmaeg_core::channel::{ChannelSecrets, CustomAuth};
use sha2::{Digest, Sha256};
use subtle::{Choice, ConditionallySelectable, ConstantTimeEq};

/// Which retained credential authenticated a custom event: the console's
/// advisory producer hint. It names a key, never its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProducerHint {
    /// The HMAC secret with this key id.
    Hmac { key_id: String },
    /// The NyxID key whose hash is at this position of `key_hashes`.
    NyxidKey { index: usize },
}

impl std::fmt::Display for ProducerHint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hmac { key_id } => write!(formatter, "hmac:{key_id}"),
            Self::NyxidKey { index } => write!(formatter, "nyxid_key:#{index}"),
        }
    }
}

/// Whether the request is authentic; see [`authenticate`].
pub fn authenticated(
    auth: &CustomAuth,
    secrets: &ChannelSecrets,
    headers: &HeaderMap,
    body: &[u8],
    now: i64,
) -> bool {
    authenticate(auth, secrets, headers, body, now).is_some()
}

/// The credential that authenticates the request, or `None`. Every NyxID
/// key hash is compared in constant time, and the matching position is
/// selected without a data-dependent branch.
pub fn authenticate(
    auth: &CustomAuth,
    secrets: &ChannelSecrets,
    headers: &HeaderMap,
    body: &[u8],
    now: i64,
) -> Option<ProducerHint> {
    match auth {
        CustomAuth::Hmac { key_id } => secrets
            .hmac_secrets
            .iter()
            .find(|(id, _)| id == key_id)
            .is_some_and(|(_, secret)| webhook::verify(headers, secret, body, now))
            .then(|| ProducerHint::Hmac {
                key_id: key_id.clone(),
            }),
        CustomAuth::NyxidKey { key_hashes } => {
            let (scheme, key) = webhook::single(headers, "authorization")
                .and_then(|header| header.split_once(' '))?;
            if !scheme.eq_ignore_ascii_case("Bearer")
                || key.is_empty()
                || key.len() > 4096
                || !key.bytes().all(|byte| (33..=126).contains(&byte))
            {
                return None;
            }
            let hash = format!("sha256:{:x}", Sha256::digest(key.as_bytes()));
            let (valid, index) = key_hashes.iter().enumerate().fold(
                (Choice::from(0), 0u64),
                |(valid, found), (position, expected)| {
                    let matches = hash.as_bytes().ct_eq(expected.as_bytes());
                    let position = position as u64;
                    (
                        valid | matches,
                        u64::conditional_select(&found, &position, matches),
                    )
                },
            );
            bool::from(valid).then(|| ProducerHint::NyxidKey {
                index: usize::try_from(index).unwrap_or(usize::MAX),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmaeg_core::channel::Secret;

    fn bearer(key: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {key}").parse().unwrap());
        headers
    }

    fn hash(key: &str) -> String {
        format!("sha256:{:x}", Sha256::digest(key.as_bytes()))
    }

    #[test]
    fn a_nyxid_key_names_the_position_of_its_hash() {
        let auth = CustomAuth::NyxidKey {
            key_hashes: vec![hash("fixture-key-a"), hash("fixture-key-b")],
        };
        let secrets = ChannelSecrets::default();
        for (key, index) in [("fixture-key-a", 0), ("fixture-key-b", 1)] {
            let hint = authenticate(&auth, &secrets, &bearer(key), b"{}", 0);
            assert_eq!(hint, Some(ProducerHint::NyxidKey { index }));
            assert_eq!(hint.unwrap().to_string(), format!("nyxid_key:#{index}"));
            assert!(authenticated(&auth, &secrets, &bearer(key), b"{}", 0));
        }
        assert_eq!(
            authenticate(&auth, &secrets, &bearer("fixture-key-c"), b"{}", 0),
            None
        );
        assert!(!authenticated(&auth, &secrets, &HeaderMap::new(), b"{}", 0));
    }

    #[test]
    fn an_hmac_hint_names_the_key_id_and_never_the_secret() {
        let auth = CustomAuth::Hmac {
            key_id: "primary".into(),
        };
        let secrets = ChannelSecrets {
            hmac_secrets: vec![("primary".into(), Secret::new("fixture-hmac-value"))],
            ..ChannelSecrets::default()
        };
        // Unsigned: refused, with no hint.
        assert_eq!(
            authenticate(&auth, &secrets, &HeaderMap::new(), b"{}", 0),
            None
        );
        let hint = ProducerHint::Hmac {
            key_id: "primary".into(),
        };
        assert_eq!(hint.to_string(), "hmac:primary");
    }
}
