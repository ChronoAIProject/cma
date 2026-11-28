use futures_util::TryStreamExt as _;
use layer2_core::profiles::authoring::{
    AuthoringSkillEntry, AuthoringSourceError, SkillCatalogQuery, MAX_SKILL_VERSIONS,
};
use layer2_core::profiles::ProfilePage;
use serde::Deserialize;

use super::http::{invalid, json, unavailable};

pub(super) struct SkillCatalog {
    pub http: reqwest::Client,
    pub origin: reqwest::Url,
}

#[derive(Deserialize)]
struct Envelope<T> {
    data: Option<T>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ProxyRefusal {
    error: String,
    message: String,
}

#[derive(Deserialize)]
struct SearchPage {
    items: Vec<SearchItem>,
    meta: SearchMeta,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchMeta {
    has_more: bool,
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct SearchItem {
    guid: String,
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct Versions {
    items: Vec<Version>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Version {
    version: String,
    is_deprecated: bool,
}

impl SkillCatalog {
    pub async fn search(
        &self,
        bearer: Option<&str>,
        query: &SkillCatalogQuery,
    ) -> Result<ProfilePage<AuthoringSkillEntry>, AuthoringSourceError> {
        let mut url = self.url(&["skill-search"])?;
        url.query_pairs_mut()
            .append_pair("q", &query.query)
            .append_pair("mode", "keyword")
            .append_pair("scope", if bearer.is_some() { "mixed" } else { "public" })
            .append_pair("limit", &query.limit.to_string());
        if let Some(cursor) = &query.cursor {
            url.query_pairs_mut().append_pair("cursor", cursor);
        }
        let response = self.get(url, bearer, "skill search").await?;
        if response.status() == reqwest::StatusCode::BAD_REQUEST {
            return Err(AuthoringSourceError::InvalidQuery(
                "skill search query was refused".into(),
            ));
        }
        let page: SearchPage = envelope(response, "skill search").await?;
        if page.items.len() > usize::from(query.limit) {
            return Err(invalid("skill search exceeded the page limit"));
        }
        let next = if page.meta.has_more {
            let cursor = page
                .meta
                .next_cursor
                .filter(|value| !value.is_empty() && value.len() <= 2048)
                .ok_or_else(|| invalid("skill search omitted its continuation"))?;
            if Some(&cursor) == query.cursor.as_ref() {
                return Err(invalid("skill search did not advance"));
            }
            Some(cursor)
        } else {
            None
        };
        let items: Vec<Option<AuthoringSkillEntry>> =
            futures_util::stream::iter(page.items.into_iter().map(Ok::<_, AuthoringSourceError>))
                .map_ok(|item| self.entry(item, bearer))
                .try_buffered(4)
                .try_collect()
                .await?;
        Ok(ProfilePage {
            items: items.into_iter().flatten().collect(),
            next,
        })
    }

    async fn entry(
        &self,
        item: SearchItem,
        bearer: Option<&str>,
    ) -> Result<Option<AuthoringSkillEntry>, AuthoringSourceError> {
        if !safe_id(&item.guid) {
            return Err(invalid("skill search returned an unsafe identity"));
        }
        // These skills are already installed by every supported agent home.
        if matches!(item.guid.as_str(), "nyxid" | "ornn-agent-manual-cli") {
            return Ok(None);
        }
        let url = self.url(&["skills", &item.guid, "versions"])?;
        let versions: Versions = envelope(
            self.get(url, bearer, "skill versions").await?,
            "skill versions",
        )
        .await?;
        if versions.items.len() > 1000 {
            return Err(invalid("skill versions exceeded the row limit"));
        }
        let mut releases = Vec::new();
        for version in versions.items {
            let key = release_key(&version.version)
                .ok_or_else(|| invalid("skill version is not a literal release"))?;
            if !version.is_deprecated {
                releases.push((key, version.version));
            }
        }
        releases.sort_by_key(|release| std::cmp::Reverse(release.0));
        if releases.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(invalid("skill catalogue repeated a release"));
        }
        releases.truncate(MAX_SKILL_VERSIONS);
        if releases.is_empty() {
            return Ok(None);
        }
        Ok(Some(AuthoringSkillEntry {
            id: layer2_core::SkillId::new(item.guid),
            name: item.name,
            description: item.description,
            versions: releases.into_iter().map(|(_, version)| version).collect(),
        }))
    }

    fn url(&self, parts: &[&str]) -> Result<reqwest::Url, AuthoringSourceError> {
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| invalid("skill catalogue origin has no path"))?
            .extend(["api", "v1", "proxy", "s", "ornn-api", "api", "v1"])
            .extend(parts);
        Ok(url)
    }

    async fn get(
        &self,
        url: reqwest::Url,
        bearer: Option<&str>,
        stage: &str,
    ) -> Result<reqwest::Response, AuthoringSourceError> {
        let mut request = self.http.get(url);
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        // Only the stage and the failure class are named; a transport error
        // can carry internal addresses.
        request.send().await.map_err(|error| {
            unavailable(&if error.is_timeout() {
                format!("{stage} request timed out")
            } else {
                format!("{stage} request failed")
            })
        })
    }
}

async fn envelope<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    stage: &str,
) -> Result<T, AuthoringSourceError> {
    let status = response.status();
    if status != reqwest::StatusCode::OK {
        // The proxy uses the same error key for node and service refusals.
        if status == reqwest::StatusCode::FORBIDDEN
            && json::<ProxyRefusal>(response).await.is_ok_and(|refusal| {
                refusal.error == "api_key_scope_forbidden"
                    && refusal.message
                        == "API key scope forbidden: API key does not have access to this service"
            })
        {
            return Err(AuthoringSourceError::SkillsAuthorizationRequired);
        }
        // The status separates a proxy or scope refusal from an Ornn outage.
        return Err(unavailable(&format!(
            "{stage} answered HTTP {}",
            status.as_u16()
        )));
    }
    let envelope: Envelope<T> = json(response).await?;
    if envelope.error.is_some() {
        return Err(invalid("skill catalogue success contains an error"));
    }
    envelope
        .data
        .ok_or_else(|| invalid("skill catalogue omitted its data"))
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn release_key(version: &str) -> Option<(u64, u64)> {
    if version.len() > 20 {
        return None;
    }
    let (major, minor) = version.split_once('.')?;
    let decimal = |value: &str| {
        (!value.is_empty()
            && (value == "0" || !value.starts_with('0'))
            && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
    };
    Some((decimal(major)?, decimal(minor)?))
}
