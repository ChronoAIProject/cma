//! The owner switcher's organization list: NyxID's `GET /api/v1/orgs` with
//! the session's own access token, fetched at most once a minute per
//! session. The list is display only; selecting an organization is verified
//! afresh like any mutation.
use std::time::Duration;

use cmaeg_core::channel::Secret;
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;

use super::session::OrgChoice;

pub(crate) const REFETCH_AFTER: Duration = Duration::from_secs(60);
const TIMEOUT: Duration = Duration::from_secs(3);
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_ORGS: usize = 100;
const MAX_NAME_CHARS: usize = 64;
const MAX_ID_BYTES: usize = 512;

#[derive(Deserialize)]
struct OrgList {
    orgs: Vec<OrgItem>,
}

#[derive(Deserialize)]
struct OrgItem {
    id: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    your_role: Option<String>,
}

/// The caller's organizations, or `None` when NyxID could not say.
pub(crate) async fn fetch(
    client: &Client,
    nyxid_base_url: &str,
    access: &Secret,
) -> Option<Vec<OrgChoice>> {
    let url = Url::parse(nyxid_base_url).ok()?.join("/api/v1/orgs").ok()?;
    let mut response = client
        .get(url)
        .bearer_auth(access.as_str())
        .timeout(TIMEOUT)
        .send()
        .await
        .ok()?;
    if response.status() != StatusCode::OK {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    let list: OrgList = serde_json::from_slice(&body).ok()?;
    Some(
        list.orgs
            .into_iter()
            .filter(|org| valid_id(&org.id))
            .take(MAX_ORGS)
            .map(|org| {
                let name = org
                    .display_name
                    .or(org.slug)
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| org.id.clone());
                OrgChoice {
                    admin: org.your_role.as_deref() == Some("admin"),
                    name: name.chars().take(MAX_NAME_CHARS).collect(),
                    id: org.id,
                }
            })
            .collect(),
    )
}

/// The same shape `X-CMAEG-Owner` accepts.
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && !id
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}
