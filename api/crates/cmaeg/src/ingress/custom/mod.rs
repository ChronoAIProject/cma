//! Custom CloudEvents authentication and neutral normalization.
pub mod activity;
pub mod attachments;
pub mod template;
use crate::auth::webhook;
use axum::http::HeaderMap;
use cmaeg_core::channel::{ChannelSecrets, CustomAuth};
use sha2::{Digest, Sha256};
use subtle::{Choice, ConstantTimeEq};

pub fn authenticated(
    auth: &CustomAuth,
    secrets: &ChannelSecrets,
    headers: &HeaderMap,
    body: &[u8],
    now: i64,
) -> bool {
    match auth {
        CustomAuth::Hmac { key_id } => secrets
            .hmac_secrets
            .iter()
            .find(|(id, _)| id == key_id)
            .is_some_and(|(_, secret)| webhook::verify(headers, secret, body, now)),
        CustomAuth::NyxidKey { key_hashes } => {
            let Some((scheme, key)) =
                webhook::single(headers, "authorization").and_then(|header| header.split_once(' '))
            else {
                return false;
            };
            if !scheme.eq_ignore_ascii_case("Bearer")
                || key.is_empty()
                || key.len() > 4096
                || !key.bytes().all(|byte| (33..=126).contains(&byte))
            {
                return false;
            }
            let hash = format!("sha256:{:x}", Sha256::digest(key.as_bytes()));
            bool::from(key_hashes.iter().fold(Choice::from(0), |valid, expected| {
                valid | hash.as_bytes().ct_eq(expected.as_bytes())
            }))
        }
    }
}
