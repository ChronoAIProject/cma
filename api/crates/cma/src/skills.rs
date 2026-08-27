//! The skill registry a caller picks from, reached through the platform proxy.
//!
//! The registry's public site serves a single-page application and no API, so
//! this talks to the service behind the identity platform's proxy instead —
//! the same way every other platform service is reached, and the reason no
//! credential for it is held here.
//!
//! Listing is a search endpoint rather than a collection route: the registry
//! has no "all skills" resource, and the scope parameter is what expresses
//! "everything public, plus mine, plus anything shared with me" in one request.
//!
//! Two identities can make that request, and which one is used decides what
//! comes back. The caller's own delegated credential answers with what *they*
//! can see. This service's machine identity is the fallback, and it sees only
//! what is public — correct for a signed-out browse, wrong as a substitute for
//! the caller, which is why it is never preferred over one.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures_util::StreamExt;
use layer2_core::store::{
    validate_skill_packs, SKILL_PACK_COUNT_LIMIT, SKILL_PACK_FILE_LIMIT, SKILL_PACK_PAYLOAD_LIMIT,
};
use layer2_core::{CatalogError, CatalogSkill, DescribedSkill, SkillCatalog, SkillId, SubjectId};
use serde::de::{self, MapAccess, Visitor};

use crate::store::token::TokenSource;

/// Path of the service behind the platform proxy.
const SERVICE_PATH: &str = "/api/v1/proxy/s/ornn-api";

/// The registry's search route, relative to the service root.
const SEARCH_PATH: &str = "/api/v1/skill-search";

/// Scope that means "public, mine, and shared with me".
///
/// The registry's own vocabulary. It is the union the picker needs; the
/// narrower scopes each drop one of the three.
const SCOPE: &str = "mixed";

/// How many skills one page returns.
///
/// The registry caps a page at 100 and returns an opaque cursor when more exist.
const PAGE_SIZE: usize = 100;

/// Maximum pages one catalogue response may fan out into.
///
/// This is the same external-list ceiling used for sandbox discovery: fifty
/// pages prove completeness for up to 5,000 skills without letting one picker
/// request drive an unbounded number of upstream calls. Page fifty is accepted
/// when it is terminal; a registry that still advertises more fails visibly.
const CATALOG_PAGE_LIMIT: usize = 50;

/// Maximum complete catalogue size implied by the page contract above.
const CATALOG_SKILL_LIMIT: usize = PAGE_SIZE * CATALOG_PAGE_LIMIT;

/// Maximum length the registry accepts for an opaque cursor.
const CURSOR_LENGTH_LIMIT: usize = 2_048;

/// Largest registry response this process will hold before decoding JSON.
///
/// Skill packs ultimately travel in a four-MiB runtime prelude. Applying that
/// same hard ceiling to every registry response keeps both declared-length and
/// chunked bodies bounded before serde can materialize attacker-controlled
/// strings and maps.
const REGISTRY_RESPONSE_LIMIT: usize = SKILL_PACK_PAYLOAD_LIMIT;

/// Maximum registry pack bodies materialized at once.
///
/// Each response can consume the four-MiB body budget. Four concurrent fetches
/// keep the transient response ceiling at sixteen MiB while avoiding one full
/// upstream round trip per selected skill.
const PACK_FETCH_CONCURRENCY: usize = 4;

/// One caller-visible budget for resolving every selected pack.
///
/// The HTTP client's fifteen-second timeout still bounds an individual
/// request. This outer deadline prevents a long list of individually slow but
/// successful requests from multiplying that latency without bound.
const PACK_RESOLUTION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
enum RegistryBodyError {
    #[error("response exceeds the {REGISTRY_RESPONSE_LIMIT}-byte limit")]
    TooLarge,
    #[error("response body could not be read: {0}")]
    Read(#[source] reqwest::Error),
}

async fn bounded_registry_body(response: reqwest::Response) -> Result<Vec<u8>, RegistryBodyError> {
    if response
        .content_length()
        .is_some_and(|length| length > REGISTRY_RESPONSE_LIMIT as u64)
    {
        return Err(RegistryBodyError::TooLarge);
    }

    let mut body = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default()
            .min(REGISTRY_RESPONSE_LIMIT),
    );
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(RegistryBodyError::Read)?;
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > REGISTRY_RESPONSE_LIMIT)
        {
            return Err(RegistryBodyError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// A skill as the registry describes it.
///
/// Deliberately forward-compatible: fields the registry adds later are ignored
/// rather than breaking the picker.
#[derive(Debug, serde::Deserialize)]
struct RegistrySkill {
    #[serde(default)]
    guid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    /// Why this caller can see it. Absent for an anonymous listing.
    #[serde(default, rename = "myAccessReason")]
    access_reason: Option<String>,
}

#[derive(Debug)]
enum DescriptionMatch {
    Missing,
    Unique { skill: CatalogSkill, exact_id: bool },
    Ambiguous,
}

impl DescriptionMatch {
    fn record(&mut self, skill: CatalogSkill, exact_id: bool) {
        *self = match self {
            Self::Missing => Self::Unique { skill, exact_id },
            Self::Unique { .. } | Self::Ambiguous => Self::Ambiguous,
        };
    }

    fn reject(&mut self) {
        *self = Self::Ambiguous;
    }
}

impl RegistrySkill {
    fn into_catalog(self) -> Result<CatalogSkill, CatalogError> {
        if self.guid.trim().is_empty() {
            return Err(CatalogError::Malformed(
                "skill search returned an item without a guid".into(),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(CatalogError::Malformed(format!(
                "skill search returned {} without a name",
                self.guid
            )));
        }
        Ok(CatalogSkill {
            id: SkillId::new(self.guid),
            name: self.name,
            description: self.description,
            access: self.access_reason,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct SearchPage {
    items: Vec<RegistrySkill>,
    #[serde(default)]
    meta: Option<SearchMeta>,
}

#[derive(Debug, serde::Deserialize)]
struct SearchMeta {
    limit: usize,
    #[serde(rename = "hasMore")]
    has_more: bool,
    #[serde(default, rename = "nextCursor")]
    next_cursor: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Option<SearchPage>,
}

/// The platform's skill registry, as a catalogue.
pub struct PlatformSkillCatalog {
    /// Fully qualified base, already including the proxy path to the service.
    base_url: String,
    /// This service's own identity, used when the caller delegated none.
    ///
    /// A source rather than a value: a machine identity's tokens expire, so a
    /// string read once at startup would work until the first expiry.
    tokens: Arc<dyn TokenSource>,
    http: reqwest::Client,
}

impl PlatformSkillCatalog {
    /// Build a catalogue over the identity platform's root.
    pub fn new(platform_base_url: &str, tokens: Arc<dyn TokenSource>) -> Self {
        Self {
            base_url: format!("{}{SERVICE_PATH}", platform_base_url.trim_end_matches('/')),
            tokens,
            http: reqwest::Client::builder()
                // A picker the user is waiting on: better to report the
                // registry as unavailable than to hold the dialog open.
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("http client builds"),
        }
    }

    /// Where a search is sent.
    fn search_url(&self) -> String {
        format!("{}{SEARCH_PATH}", self.base_url)
    }

    /// Resolve one opaque registry identifier without letting it become URL syntax.
    fn pack_url(&self, skill: &SkillId) -> Result<reqwest::Url, String> {
        // The URL standard treats these two values as navigation even when a
        // path-segment setter is used. They cannot name a contained resource.
        if matches!(skill.as_str(), "." | "..") {
            return Err(format!(
                "skill identifier {:?} is not a safe registry path segment",
                skill.as_str()
            ));
        }
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|error| format!("skill registry URL is invalid: {error}"))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|()| "skill registry URL cannot carry path segments".to_string())?;
            segments.extend(PACK_PATH.trim_start_matches('/').split('/'));
            // `SkillId` is deliberately opaque and can come from a stored profile.
            // Pushing it as one segment percent-encodes slashes, dot traversal,
            // query delimiters, and fragments before the authenticated request.
            segments.push(skill.as_str());
            segments.push("json");
        }
        Ok(url)
    }

    async fn fetch_pack_body(&self, bearer: &str, skill: &SkillId) -> Result<Vec<u8>, String> {
        let url = self.pack_url(skill)?;
        let response = self
            .http
            .get(url)
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| format!("{} could not be fetched: {e}", skill.0))?;
        // Named rather than swallowed. A session that starts without a skill
        // it was promised looks exactly like one that never asked for it, and
        // the difference only shows when the agent cannot do what it was told.
        if !response.status().is_success() {
            return Err(format!(
                "{} could not be fetched: the registry answered {}",
                skill.0,
                response.status()
            ));
        }
        bounded_registry_body(response)
            .await
            .map_err(|error| format!("{} could not be fetched: {error}", skill.0))
    }

    async fn packs_for_with_deadline(
        &self,
        caller_credential: Option<&str>,
        skills: &[SkillId],
        deadline: std::time::Duration,
    ) -> Result<Vec<layer2_core::SkillPack>, String> {
        if skills.len() > SKILL_PACK_COUNT_LIMIT {
            return Err(format!(
                "a session may carry at most {SKILL_PACK_COUNT_LIMIT} skill packs"
            ));
        }
        if skills.is_empty() {
            return Ok(Vec::new());
        }

        let resolution = async {
            let bearer = self
                .bearer(caller_credential)
                .await
                .map_err(|e| e.to_string())?;
            let requests = skills.iter().cloned().map(|skill| {
                let bearer = bearer.as_str();
                async move {
                    let body = self.fetch_pack_body(bearer, &skill).await;
                    (skill, body)
                }
            });
            let responses = futures_util::stream::iter(requests)
                // `buffered` polls concurrently but yields in source order. The
                // reducer therefore never stores more than this window of response
                // bodies and still derives grants in exact request order.
                .buffered(PACK_FETCH_CONCURRENCY);
            futures_util::pin_mut!(responses);

            let mut packs = Vec::with_capacity(skills.len());
            let mut remaining_files = SKILL_PACK_FILE_LIMIT;
            while let Some((skill, body)) = responses.next().await {
                remaining_files =
                    append_registry_pack(&skill, &body?, remaining_files, &mut packs)?;
            }
            tracing::info!(skills = packs.len(), "fetched this session's skills");
            Ok(packs)
        };

        tokio::time::timeout(deadline, resolution)
            .await
            .map_err(|_| {
                format!(
                    "skill pack resolution did not finish within {}s",
                    deadline.as_secs()
                )
            })?
    }

    /// Read one canonical cursor page.
    async fn search_page(
        &self,
        url: &str,
        bearer: &str,
        query: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<SearchPage, CatalogError> {
        let limit = PAGE_SIZE.to_string();
        let mut request = self.http.get(url).query(&[
            ("scope", SCOPE),
            ("limit", limit.as_str()),
            ("mode", "keyword"),
        ]);
        if let Some(cursor) = cursor {
            request = request.query(&[("cursor", cursor)]);
        }
        if let Some(query) = query {
            request = request.query(&[("q", query)]);
        }
        let response = request
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| CatalogError::Unavailable(format!("GET {url}: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(CatalogError::Unavailable(format!(
                "skill search returned {status}"
            )));
        }
        let body = bounded_registry_body(response)
            .await
            .map_err(|error| match error {
                RegistryBodyError::TooLarge => {
                    CatalogError::Malformed(format!("skill search {error}"))
                }
                RegistryBodyError::Read(_) => {
                    CatalogError::Unavailable(format!("skill search {error}"))
                }
            })?;
        let body: SearchResponse = serde_json::from_slice(&body)
            .map_err(|e| CatalogError::Malformed(format!("skill search response: {e}")))?;
        body.data
            .ok_or_else(|| CatalogError::Malformed("skill search response has no data".into()))
    }

    /// The credential this request should present.
    ///
    /// The caller's own when there is one. Falling back to this service's
    /// identity keeps the picker working in a deployment that delegates
    /// nothing, at the cost of showing only public skills.
    async fn bearer(&self, delegated: Option<&str>) -> Result<String, CatalogError> {
        match delegated {
            Some(c) if !c.trim().is_empty() => Ok(c.to_string()),
            _ => self
                .tokens
                .token()
                .await
                .map_err(|e| CatalogError::Unavailable(format!("no usable credential: {e}"))),
        }
    }
}

#[async_trait::async_trait]
impl SkillCatalog for PlatformSkillCatalog {
    async fn available(
        &self,
        subject: &SubjectId,
        credential: Option<&str>,
    ) -> Result<Vec<CatalogSkill>, CatalogError> {
        let url = self.search_url();
        let bearer = self.bearer(credential).await?;
        let mut cursor = None;
        let mut cursors = HashSet::new();
        let mut identities = HashSet::new();
        let mut skills = Vec::new();

        for page_number in 1..=CATALOG_PAGE_LIMIT {
            let page = self
                .search_page(&url, &bearer, None, cursor.as_deref())
                .await?;
            let meta = page.meta.ok_or_else(|| {
                CatalogError::Malformed(format!(
                    "skill search page {page_number} has no pagination metadata"
                ))
            })?;
            if meta.limit == 0 || meta.limit > PAGE_SIZE {
                return Err(CatalogError::Malformed(format!(
                    "skill search page {page_number} reported invalid limit {}",
                    meta.limit
                )));
            }
            if page.items.len() > meta.limit {
                return Err(CatalogError::Malformed(format!(
                    "skill search page {page_number} returned {} items for limit {}",
                    page.items.len(),
                    meta.limit
                )));
            }

            for item in page.items {
                let skill = item.into_catalog()?;
                if !identities.insert(skill.id.0.clone()) {
                    return Err(CatalogError::Malformed(format!(
                        "skill search repeated identity {}",
                        skill.id.0
                    )));
                }
                skills.push(skill);
            }

            if !meta.has_more {
                tracing::info!(
                    %subject,
                    count = skills.len(),
                    pages = page_number,
                    delegated = credential.is_some(),
                    "listed selectable skills"
                );
                return Ok(skills);
            }
            if skills.len() >= CATALOG_SKILL_LIMIT || page_number == CATALOG_PAGE_LIMIT {
                return Err(CatalogError::Malformed(format!(
                    "skill search did not end within {CATALOG_PAGE_LIMIT} pages / {CATALOG_SKILL_LIMIT} skills"
                )));
            }

            let next = meta
                .next_cursor
                .filter(|next| !next.trim().is_empty())
                .ok_or_else(|| {
                    CatalogError::Malformed(format!(
                        "skill search page {page_number} says more exist but has no next cursor"
                    ))
                })?;
            if next.len() > CURSOR_LENGTH_LIMIT {
                return Err(CatalogError::Malformed(format!(
                    "skill search page {page_number} returned an oversized cursor"
                )));
            }
            if !cursors.insert(next.clone()) {
                return Err(CatalogError::Malformed(format!(
                    "skill search repeated a cursor on page {page_number}"
                )));
            }
            cursor = Some(next);
        }

        unreachable!("the catalogue page loop returns at its explicit bound")
    }

    async fn describe(
        &self,
        subject: &SubjectId,
        credential: Option<&str>,
        skills: &[SkillId],
    ) -> Result<Vec<DescribedSkill>, CatalogError> {
        if skills.is_empty() {
            return Ok(Vec::new());
        }

        let url = self.search_url();
        let bearer = self.bearer(credential).await?;
        let mut requested = HashMap::new();
        let mut requested_order = Vec::new();
        let mut matches = Vec::new();
        for skill in skills {
            if requested.contains_key(&skill.0) {
                continue;
            }
            requested.insert(skill.0.clone(), matches.len());
            requested_order.push(skill.clone());
            matches.push(DescriptionMatch::Missing);
        }

        let mut cursor = None;
        let mut cursors = HashSet::new();
        let mut pages = 0;
        let mut failure = None;
        let mut complete = false;

        for page_number in 1..=CATALOG_PAGE_LIMIT {
            pages = page_number;
            let page = match self
                .search_page(&url, &bearer, None, cursor.as_deref())
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            };

            let item_count = page.items.len();
            let meta = match &page.meta {
                Some(meta) => meta,
                None => {
                    failure = Some(CatalogError::Malformed(format!(
                        "skill search page {page_number} has no pagination metadata"
                    )));
                    break;
                }
            };
            if meta.limit == 0 || meta.limit > PAGE_SIZE {
                failure = Some(CatalogError::Malformed(format!(
                    "skill search page {page_number} reported invalid limit {}",
                    meta.limit
                )));
                break;
            }
            if item_count > meta.limit {
                failure = Some(CatalogError::Malformed(format!(
                    "skill search page {page_number} returned {item_count} items for limit {}",
                    meta.limit
                )));
                break;
            }
            let next = if meta.has_more {
                if page_number == CATALOG_PAGE_LIMIT {
                    failure = Some(CatalogError::Malformed(format!(
                        "skill search did not end within {CATALOG_PAGE_LIMIT} pages / {CATALOG_SKILL_LIMIT} skills"
                    )));
                    break;
                }
                let Some(next) = meta
                    .next_cursor
                    .as_ref()
                    .filter(|next| !next.trim().is_empty())
                else {
                    failure = Some(CatalogError::Malformed(format!(
                        "skill search page {page_number} says more exist but has no next cursor"
                    )));
                    break;
                };
                if next.len() > CURSOR_LENGTH_LIMIT {
                    failure = Some(CatalogError::Malformed(format!(
                        "skill search page {page_number} returned an oversized cursor"
                    )));
                    break;
                }
                if !cursors.insert(next.clone()) {
                    failure = Some(CatalogError::Malformed(format!(
                        "skill search repeated a cursor on page {page_number}"
                    )));
                    break;
                }
                Some(next.clone())
            } else {
                None
            };

            for item in page.items {
                let guid_match = requested.get(&item.guid).copied();
                let name_match = requested.get(&item.name).copied();
                match item.into_catalog() {
                    Ok(candidate) => {
                        if let Some(index) = guid_match {
                            matches[index].record(candidate.clone(), true);
                        }
                        if let Some(index) = name_match.filter(|index| Some(*index) != guid_match) {
                            matches[index].record(candidate, false);
                        }
                    }
                    Err(error) => {
                        if let Some(index) = guid_match {
                            matches[index].reject();
                        }
                        if let Some(index) = name_match.filter(|index| Some(*index) != guid_match) {
                            matches[index].reject();
                        }
                        tracing::warn!(
                            page = page_number,
                            error = %error,
                            "ignored malformed skill metadata while describing durable grants"
                        );
                    }
                }
            }

            if let Some(next) = next {
                cursor = Some(next);
            } else {
                complete = true;
                break;
            }
        }

        let described: Vec<DescribedSkill> = requested_order
            .into_iter()
            .zip(matches)
            .filter_map(|(requested, matched)| match matched {
                DescriptionMatch::Unique { skill, exact_id } if complete || exact_id => {
                    Some(DescribedSkill { requested, skill })
                }
                DescriptionMatch::Unique { .. }
                | DescriptionMatch::Missing
                | DescriptionMatch::Ambiguous => None,
            })
            .collect();
        if described.is_empty() {
            if let Some(error) = failure {
                return Err(error);
            }
        } else if let Some(error) = failure {
            tracing::warn!(
                %subject,
                pages,
                count = described.len(),
                error = %error,
                "skill metadata walk ended early; preserving exact matches collected so far"
            );
        }
        tracing::info!(
            %subject,
            count = described.len(),
            requested = requested.len(),
            pages,
            delegated = credential.is_some(),
            "described this page's session skills"
        );
        Ok(described)
    }
}

/// Build the catalogue a deployment's configuration describes.
///
/// Returns `None` when the platform identity is not configured, which leaves
/// the picker empty rather than failing on first use.
pub fn from_config(
    platform_base_url: &str,
    tokens: Option<Arc<dyn TokenSource>>,
) -> Option<Arc<PlatformSkillCatalog>> {
    if platform_base_url.trim().is_empty() {
        return None;
    }
    let tokens = tokens?;
    Some(Arc::new(PlatformSkillCatalog::new(
        platform_base_url,
        tokens,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::token::StaticToken;

    fn catalog(base: &str) -> PlatformSkillCatalog {
        PlatformSkillCatalog::new(base, Arc::new(StaticToken("service-token".into())))
    }

    #[test]
    fn the_registry_is_addressed_through_the_platform_proxy() {
        // The registry's own hostname serves a single-page app and no API;
        // reaching it directly returns HTML with a 200, which is the failure
        // this asserts against.
        assert_eq!(
            catalog("https://platform.example").search_url(),
            "https://platform.example/api/v1/proxy/s/ornn-api/api/v1/skill-search"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_platform_root_does_not_double_up() {
        assert_eq!(
            catalog("https://platform.example/").search_url(),
            "https://platform.example/api/v1/proxy/s/ornn-api/api/v1/skill-search"
        );
    }

    #[tokio::test]
    async fn the_callers_own_credential_is_preferred_over_this_services() {
        // What makes the answer the caller's: their own private and shared
        // skills are invisible to this service's identity.
        let c = catalog("https://platform.example");
        assert_eq!(
            c.bearer(Some("caller-token")).await.expect("resolves"),
            "caller-token"
        );
    }

    #[tokio::test]
    async fn a_blank_delegated_credential_falls_back_rather_than_sending_nothing() {
        // An empty bearer is refused at the proxy with a 401, which reads as
        // "the registry is down" to a user who is merely not delegating.
        let c = catalog("https://platform.example");
        assert_eq!(
            c.bearer(Some("   ")).await.expect("resolves"),
            "service-token"
        );
        assert_eq!(c.bearer(None).await.expect("resolves"), "service-token");
    }

    #[tokio::test]
    async fn an_unreachable_registry_is_reported_rather_than_read_as_empty() {
        // Reporting no skills would tell a caller their registry is empty when
        // it is merely unreachable, and they would author a profile without
        // the skills they meant to select.
        let c = catalog("http://127.0.0.1:1");
        let err = c
            .available(&SubjectId("subject-a".into()), None)
            .await
            .expect_err("must fail against a dead registry");
        assert!(matches!(err, CatalogError::Unavailable(_)), "got {err:?}");
    }

    #[test]
    fn a_search_page_parses_from_the_registrys_envelope() {
        let body: SearchResponse = serde_json::from_str(
            r#"{"data":{"items":[
                {"guid":"g1","name":"one","description":"first","myAccessReason":"owner"},
                {"guid":"g2","name":"two","description":"second"}
            ],"total":2},"error":null}"#,
        )
        .expect("parses");
        let items = body.data.expect("has data").items;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].guid, "g1");
        assert_eq!(items[0].access_reason.as_deref(), Some("owner"));
        assert!(items[1].access_reason.is_none());
    }

    #[test]
    fn unknown_response_fields_are_ignored() {
        // The registry adds fields over time; a new one must not empty the
        // picker.
        let body: SearchResponse = serde_json::from_str(
            r#"{"data":{"items":[{"guid":"g1","tags":["x"],"isPrivate":false}],
                "meta":{"limit":100,"hasMore":false,"newField":"kept-forward-compatible"}},
                "error":null,"topLevelAddition":true}"#,
        )
        .expect("forward-compatible");
        assert_eq!(body.data.expect("has data").items[0].guid, "g1");
    }

    #[test]
    fn a_response_with_no_data_is_represented_as_missing_not_as_an_empty_page() {
        let body: SearchResponse = serde_json::from_str(r#"{"error":null}"#).expect("parses");
        assert!(body.data.is_none());
    }

    #[test]
    fn an_unconfigured_platform_yields_no_catalogue() {
        let tokens: Arc<dyn TokenSource> = Arc::new(StaticToken("t".into()));
        assert!(from_config("", Some(Arc::clone(&tokens))).is_none());
        assert!(from_config("https://platform.example", None).is_none());
        assert!(from_config("https://platform.example", Some(tokens)).is_some());
    }
}

#[cfg(test)]
#[path = "skills_paging_tests.rs"]
mod paging_tests;

/// Where one skill's contents are fetched from.
///
/// `{idOrName}` is what resolves an ambivalence the core has by construction:
/// `SkillId` is opaque, the picker fills it from the registry's guid, and the
/// deployment's baseline names skills in words. One route takes both.
const PACK_PATH: &str = "/api/v1/skills";

/// The registry's answer for one skill: what it is, and every file in it.
#[derive(Debug, serde::Deserialize)]
struct PackResponse {
    #[serde(default)]
    data: Option<PackBody>,
}

#[derive(Debug, serde::Deserialize)]
struct PackBody {
    #[serde(default)]
    name: String,
    /// Which version this fetch returned, where the registry reports one.
    #[serde(default)]
    version: Option<String>,
    /// Path within the skill's directory, to its contents.
    #[serde(default)]
    files: PackFiles,
}

/// A map that stops decoding before a zero-byte-file fanout can exceed the
/// shared session limit. The collection-wide check below remains authoritative
/// when more than one response contributes files.
#[derive(Debug, Default)]
struct PackFiles(std::collections::BTreeMap<String, String>);

impl<'de> serde::Deserialize<'de> for PackFiles {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(PackFilesVisitor)
    }
}

struct PackFilesVisitor;

impl<'de> Visitor<'de> for PackFilesVisitor {
    type Value = PackFiles;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a bounded map of skill file paths to contents")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut files = std::collections::BTreeMap::new();
        while let Some(path) = map.next_key::<String>()? {
            if files.len() >= SKILL_PACK_FILE_LIMIT {
                return Err(de::Error::custom(format_args!(
                    "skill response would exceed the {SKILL_PACK_FILE_LIMIT}-file session limit"
                )));
            }
            if files.contains_key(&path) {
                return Err(de::Error::custom(
                    "skill response repeats an effective file target",
                ));
            }
            let content = map.next_value()?;
            files.insert(path, content);
        }
        Ok(PackFiles(files))
    }
}

fn append_registry_pack(
    requested: &SkillId,
    response: &[u8],
    remaining_files: usize,
    packs: &mut Vec<layer2_core::SkillPack>,
) -> Result<usize, String> {
    let body = serde_json::from_slice::<PackResponse>(response)
        .map_err(|e| format!("{} came back in a shape we cannot read: {e}", requested.0))?
        .data
        .ok_or_else(|| format!("{} came back empty", requested.0))?;
    if body.files.0.is_empty() {
        return Err(format!("{} carries no files", requested.0));
    }
    if body.files.0.len() > remaining_files {
        return Err(format!(
            "{} would exceed the {SKILL_PACK_FILE_LIMIT}-file session limit",
            requested.0
        ));
    }

    // The registry's own name for it, because that is what the agent invokes —
    // the identifier may be a guid nobody types. The shared core validator
    // checks both values as path components, including the fallback.
    let name = match body.name.trim().is_empty() {
        true => requested.0.clone(),
        false => body.name,
    };
    let file_count = body.files.0.len();
    packs.push(layer2_core::SkillPack {
        name,
        // Kept from the fetch, so a session reports the version it was actually
        // given rather than whatever is current when its details are opened.
        version: body
            .version
            .map(|version| version.trim().to_string())
            .filter(|version| !version.is_empty()),
        files: body
            .files
            .0
            .into_iter()
            .map(|(path, content)| (path, content.into_bytes()))
            .collect(),
    });
    if let Err(error) = validate_skill_packs(packs) {
        packs.pop();
        return Err(format!(
            "{} returned an unusable skill pack: {error}",
            requested.0
        ));
    }
    Ok(remaining_files - file_count)
}

#[async_trait::async_trait]
impl layer2_core::SkillSource for PlatformSkillCatalog {
    async fn packs_for(
        &self,
        caller_credential: Option<&str>,
        skills: &[layer2_core::SkillId],
    ) -> Result<Vec<layer2_core::SkillPack>, String> {
        self.packs_for_with_deadline(caller_credential, skills, PACK_RESOLUTION_DEADLINE)
            .await
    }
}

#[cfg(test)]
mod pack_tests {
    use std::collections::{HashMap, VecDeque};
    use std::convert::Infallible;
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Poll;
    use std::time::Duration;

    use axum::body::{Body, Bytes};
    use axum::extract::{OriginalUri, Path, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::Response;
    use axum::routing::{any, get};
    use axum::Router;
    use tokio::sync::{watch, Mutex};

    use super::*;
    use crate::store::token::StaticToken;

    #[derive(Clone)]
    enum PackReply {
        Full(Vec<u8>),
        Chunked(Vec<Bytes>),
    }

    type Replies = Arc<Mutex<VecDeque<PackReply>>>;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct ContainedRequest {
        id: Option<String>,
        path: String,
        authorization: Option<String>,
    }

    type ContainedRequests = Arc<Mutex<Vec<ContainedRequest>>>;

    #[derive(Clone)]
    struct TimedRegistry {
        delays: Arc<HashMap<String, Duration>>,
        started: Arc<Mutex<Vec<String>>>,
        completed: Arc<Mutex<Vec<String>>>,
        started_count: watch::Sender<usize>,
        completed_count: watch::Sender<usize>,
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
    }

    async fn answer_pack(State(replies): State<Replies>) -> Response {
        let Some(reply) = replies.lock().await.pop_front() else {
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("unexpected registry request"))
                .expect("fixture response");
        };
        let body = match reply {
            PackReply::Full(body) => Body::from(body),
            PackReply::Chunked(chunks) => Body::from_stream(futures_util::stream::iter(
                chunks.into_iter().map(Ok::<_, Infallible>),
            )),
        };
        Response::builder()
            .status(StatusCode::OK)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .expect("fixture response")
    }

    async fn registry(replies: Vec<PackReply>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind registry fixture");
        let address = listener.local_addr().expect("fixture address");
        let app = Router::new()
            .fallback(any(answer_pack))
            .with_state(Arc::new(Mutex::new(replies.into())));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve registry fixture");
        });
        (format!("http://{address}"), server)
    }

    async fn answer_contained_pack(
        State(requests): State<ContainedRequests>,
        Path(id): Path<String>,
        OriginalUri(uri): OriginalUri,
        headers: HeaderMap,
    ) -> Response {
        requests.lock().await.push(ContainedRequest {
            id: Some(id),
            path: uri.path().to_string(),
            authorization: headers
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
        });
        Response::builder()
            .status(StatusCode::OK)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(Body::from(pack_response(
                "contained",
                None,
                file_map(&[("SKILL.md", "instructions")]),
            )))
            .expect("fixture response")
    }

    async fn record_escaped_pack_request(
        State(requests): State<ContainedRequests>,
        OriginalUri(uri): OriginalUri,
        headers: HeaderMap,
    ) -> Response {
        requests.lock().await.push(ContainedRequest {
            id: None,
            path: uri.path().to_string(),
            authorization: headers
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
        });
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("request escaped the skill-pack route"))
            .expect("fixture response")
    }

    async fn contained_registry() -> (String, ContainedRequests, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind registry fixture");
        let address = listener.local_addr().expect("fixture address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/api/v1/proxy/s/ornn-api/api/v1/skills/{id}/json",
                get(answer_contained_pack),
            )
            .fallback(any(record_escaped_pack_request))
            .with_state(Arc::clone(&requests));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve registry fixture");
        });
        (format!("http://{address}"), requests, server)
    }

    async fn answer_timed_pack(
        State(registry): State<TimedRegistry>,
        Path(id): Path<String>,
    ) -> Response {
        let Some(delay) = registry.delays.get(&id).copied() else {
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("unexpected timed registry request"))
                .expect("fixture response");
        };
        let active = registry.active.fetch_add(1, Ordering::SeqCst) + 1;
        registry.max_active.fetch_max(active, Ordering::SeqCst);

        let mut response_delay = Box::pin(tokio::time::sleep(delay));
        let already_elapsed = std::future::poll_fn(|context| {
            Poll::Ready(response_delay.as_mut().poll(context).is_ready())
        })
        .await;
        let started = {
            let mut started = registry.started.lock().await;
            started.push(id.clone());
            started.len()
        };
        registry.started_count.send_replace(started);
        if !already_elapsed {
            response_delay.await;
        }

        registry.active.fetch_sub(1, Ordering::SeqCst);
        let completed = {
            let mut completed = registry.completed.lock().await;
            completed.push(id.clone());
            completed.len()
        };
        registry.completed_count.send_replace(completed);
        Response::builder()
            .status(StatusCode::OK)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(Body::from(pack_response(
                &id,
                None,
                file_map(&[("SKILL.md", "instructions")]),
            )))
            .expect("fixture response")
    }

    async fn timed_registry(
        delays: HashMap<String, Duration>,
    ) -> (String, TimedRegistry, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind timed registry fixture");
        let address = listener.local_addr().expect("fixture address");
        let (started_count, _) = watch::channel(0);
        let (completed_count, _) = watch::channel(0);
        let registry = TimedRegistry {
            delays: Arc::new(delays),
            started: Arc::new(Mutex::new(Vec::new())),
            completed: Arc::new(Mutex::new(Vec::new())),
            started_count,
            completed_count,
            active: Arc::new(AtomicUsize::new(0)),
            max_active: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route(
                "/api/v1/proxy/s/ornn-api/api/v1/skills/{id}/json",
                get(answer_timed_pack),
            )
            .with_state(registry.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve timed registry fixture");
        });
        (format!("http://{address}"), registry, server)
    }

    async fn wait_for_count(count: &mut watch::Receiver<usize>, expected: usize, label: &str) {
        count
            .wait_for(|value| *value >= expected)
            .await
            .unwrap_or_else(|_| panic!("timed registry stopped before {expected} {label}"));
    }

    fn catalog(base: &str) -> PlatformSkillCatalog {
        PlatformSkillCatalog::new(base, Arc::new(StaticToken("service-token".into())))
    }

    fn file_map(files: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        files
            .iter()
            .map(|(path, body)| ((*path).to_string(), serde_json::json!(body)))
            .collect()
    }

    fn pack_response(
        name: &str,
        version: Option<&str>,
        files: serde_json::Map<String, serde_json::Value>,
    ) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "data": {
                "name": name,
                "version": version,
                "files": files,
                "futureRegistryField": { "ignored": true }
            },
            "error": null
        }))
        .expect("pack fixture serializes")
    }

    fn append(
        requested: &str,
        response: &[u8],
        remaining_files: usize,
        packs: &mut Vec<layer2_core::SkillPack>,
    ) -> Result<usize, String> {
        append_registry_pack(&SkillId::new(requested), response, remaining_files, packs)
    }

    #[tokio::test]
    async fn a_chunked_pack_response_is_stopped_one_byte_past_the_body_limit() {
        let (base, server) = registry(vec![PackReply::Chunked(vec![
            Bytes::from(vec![b' '; REGISTRY_RESPONSE_LIMIT]),
            Bytes::from_static(b"x"),
        ])])
        .await;

        let error = layer2_core::SkillSource::packs_for(
            &catalog(&base),
            None,
            &[SkillId::new("review-guid")],
        )
        .await
        .expect_err("an unframed chunked body must remain bounded");

        assert!(
            error.contains(&format!("{REGISTRY_RESPONSE_LIMIT}-byte limit")),
            "got {error}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn opaque_skill_ids_cannot_escape_the_authenticated_pack_route() {
        let (base, requests, server) = contained_registry().await;
        let catalog = catalog(&base);
        let adversarial_ids = [
            "../escape",
            "nested/escape",
            r"..\escape",
            "name?admin=true",
            "name#fragment",
            "%2e%2e/%2e%2e/escape",
        ];

        for id in [".", ".."] {
            let error = layer2_core::SkillSource::packs_for(
                &catalog,
                Some("caller-token"),
                &[SkillId::new(id)],
            )
            .await
            .expect_err("a dot-only identifier must be rejected before its request");
            assert!(error.contains("safe registry path segment"), "got {error}");
        }
        assert!(
            requests.lock().await.is_empty(),
            "dot-only identifiers must not emit an authenticated request"
        );

        for id in adversarial_ids {
            let packs = layer2_core::SkillSource::packs_for(
                &catalog,
                Some("caller-token"),
                &[SkillId::new(id)],
            )
            .await
            .unwrap_or_else(|error| panic!("opaque identifier {id:?} escaped: {error}"));
            assert_eq!(packs.len(), 1);
        }

        let requests = requests.lock().await;
        assert_eq!(requests.len(), adversarial_ids.len());
        for (request, expected_id) in requests.iter().zip(adversarial_ids) {
            assert_eq!(request.id.as_deref(), Some(expected_id));
            assert_eq!(
                request.authorization.as_deref(),
                Some("Bearer caller-token")
            );
            assert!(request
                .path
                .starts_with("/api/v1/proxy/s/ornn-api/api/v1/skills/"));
            assert!(request.path.ends_with("/json"));
            assert_eq!(
                request.path.split('/').count(),
                11,
                "the opaque identifier must occupy exactly one URL segment: {}",
                request.path
            );
        }
        drop(requests);
        server.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_pack_fetches_preserve_request_order_after_out_of_order_responses() {
        let delays = HashMap::from([
            ("first".to_string(), Duration::from_secs(9)),
            ("second".to_string(), Duration::from_secs(1)),
            ("third".to_string(), Duration::from_secs(5)),
            ("fourth".to_string(), Duration::from_secs(3)),
        ]);
        let (base, registry, server) = timed_registry(delays).await;
        let mut started = registry.started_count.subscribe();
        let mut completed = registry.completed_count.subscribe();
        let resolving = tokio::spawn(async move {
            layer2_core::SkillSource::packs_for(
                &catalog(&base),
                Some("caller-token"),
                &[
                    SkillId::new("first"),
                    SkillId::new("second"),
                    SkillId::new("third"),
                    SkillId::new("fourth"),
                ],
            )
            .await
        });

        wait_for_count(&mut started, PACK_FETCH_CONCURRENCY, "requests").await;
        tokio::time::pause();
        assert_eq!(
            registry.max_active.load(Ordering::SeqCst),
            PACK_FETCH_CONCURRENCY
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        wait_for_count(&mut completed, 1, "responses").await;
        tokio::time::advance(Duration::from_secs(2)).await;
        wait_for_count(&mut completed, 2, "responses").await;
        tokio::time::advance(Duration::from_secs(2)).await;
        wait_for_count(&mut completed, 3, "responses").await;
        tokio::time::advance(Duration::from_secs(4)).await;
        wait_for_count(&mut completed, 4, "responses").await;

        assert_eq!(
            registry.completed.lock().await.as_slice(),
            ["second", "fourth", "third", "first"]
        );
        let packs = resolving
            .await
            .expect("resolution task joins")
            .expect("all responses finish inside the aggregate deadline");
        assert_eq!(
            packs
                .iter()
                .map(|pack| pack.name.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third", "fourth"]
        );
        server.abort();
    }

    #[tokio::test]
    async fn individually_successful_slow_packs_share_one_named_resolution_deadline() {
        const TEST_DEADLINE: Duration = Duration::from_secs(2);
        let ids = (0..PACK_FETCH_CONCURRENCY * 2 + 1)
            .map(|index| format!("slow-{index}"))
            .collect::<Vec<_>>();
        let delays = ids
            .iter()
            .cloned()
            .map(|id| (id, Duration::from_millis(700)))
            .collect();
        let (base, registry, server) = timed_registry(delays).await;
        let mut started = registry.started_count.subscribe();
        let requested = ids.iter().cloned().map(SkillId::new).collect::<Vec<_>>();
        let resolving = tokio::spawn(async move {
            catalog(&base)
                .packs_for_with_deadline(Some("caller-token"), &requested, TEST_DEADLINE)
                .await
        });

        wait_for_count(&mut started, PACK_FETCH_CONCURRENCY, "requests").await;
        assert_eq!(
            registry.max_active.load(Ordering::SeqCst),
            PACK_FETCH_CONCURRENCY,
            "only one bounded response window may be materialized"
        );
        wait_for_count(&mut started, PACK_FETCH_CONCURRENCY * 2, "requests").await;
        wait_for_count(&mut started, PACK_FETCH_CONCURRENCY * 2 + 1, "requests").await;

        let error = resolving
            .await
            .expect("resolution task joins")
            .expect_err("the aggregate deadline must stop another slow batch");
        assert_eq!(
            error,
            format!(
                "skill pack resolution did not finish within {}s",
                TEST_DEADLINE.as_secs()
            )
        );
        assert_eq!(
            registry.max_active.load(Ordering::SeqCst),
            PACK_FETCH_CONCURRENCY
        );
        server.abort();
    }

    #[tokio::test]
    async fn pack_count_is_rejected_before_any_registry_request() {
        let requested = (0..=SKILL_PACK_COUNT_LIMIT)
            .map(|index| SkillId::new(format!("skill-{index}")))
            .collect::<Vec<_>>();

        let error =
            layer2_core::SkillSource::packs_for(&catalog("http://127.0.0.1:1"), None, &requested)
                .await
                .expect_err("the in-process count bound is authoritative");

        assert!(error.contains(&SKILL_PACK_COUNT_LIMIT.to_string()));
        assert!(!error.contains("could not be fetched"), "got {error}");
    }

    #[test]
    fn registry_names_and_paths_must_be_canonical_contained_targets() {
        for invalid_name in ["../review", "/tmp/review", "review/child", "bad\0name"] {
            let response = pack_response(
                invalid_name,
                None,
                file_map(&[("SKILL.md", "instructions")]),
            );
            let mut packs = Vec::new();
            let error = append("review-guid", &response, SKILL_PACK_FILE_LIMIT, &mut packs)
                .expect_err("an unsafe registry name must not become a directory");
            assert!(error.contains("safe single path component"), "got {error}");
            assert!(packs.is_empty());
        }

        for invalid_path in [
            "../outside",
            "/tmp/outside",
            "references/../../outside",
            "references/./api.md",
            "bad\0path",
        ] {
            let response =
                pack_response("review", None, file_map(&[(invalid_path, "instructions")]));
            let mut packs = Vec::new();
            let error = append("review-guid", &response, SKILL_PACK_FILE_LIMIT, &mut packs)
                .expect_err("an unsafe registry path must not become a file target");
            assert!(error.contains("canonical relative path"), "got {error}");
            assert!(packs.is_empty());
        }

        let fallback = pack_response(" ", None, file_map(&[("SKILL.md", "instructions")]));
        let mut packs = Vec::new();
        let error = append(
            "../unsafe-fallback",
            &fallback,
            SKILL_PACK_FILE_LIMIT,
            &mut packs,
        )
        .expect_err("an unsafe requested identifier is not a safe fallback name");
        assert!(error.contains("safe single path component"), "got {error}");
        assert!(packs.is_empty());
    }

    #[test]
    fn duplicate_registry_names_and_effective_file_targets_are_rejected() {
        let repeated_target =
            br#"{"data":{"name":"review","files":{"SKILL.md":"one","SKILL.md":"two"}}}"#;
        let mut packs = Vec::new();
        let error = append(
            "first-guid",
            repeated_target,
            SKILL_PACK_FILE_LIMIT,
            &mut packs,
        )
        .expect_err("a repeated JSON key must not be silently overwritten");
        assert!(
            error.contains("repeats an effective file target"),
            "got {error}"
        );
        assert!(packs.is_empty());

        let first = pack_response("review", None, file_map(&[("SKILL.md", "one")]));
        let second = pack_response("review", None, file_map(&[("references/api.md", "two")]));
        let remaining = append("first-guid", &first, SKILL_PACK_FILE_LIMIT, &mut packs)
            .expect("first unique target is accepted");
        let error = append("second-guid", &second, remaining, &mut packs)
            .expect_err("two packs cannot resolve to the same directory");
        assert!(
            error.contains("name \"review\" is duplicated"),
            "got {error}"
        );
        assert_eq!(packs.len(), 1, "the rejected candidate is removed");
    }

    #[test]
    fn zero_byte_file_fanout_is_bounded_during_deserialization() {
        let files = (0..=SKILL_PACK_FILE_LIMIT)
            .map(|index| {
                (
                    format!("references/{index}.md"),
                    serde_json::Value::String(String::new()),
                )
            })
            .collect();
        let response = pack_response("review", None, files);
        let mut packs = Vec::new();

        let error = append("review-guid", &response, SKILL_PACK_FILE_LIMIT, &mut packs)
            .expect_err("empty bodies must not bypass the file-count bound");

        assert!(
            error.contains(&format!("{SKILL_PACK_FILE_LIMIT}-file session limit")),
            "got {error}"
        );
        assert!(packs.is_empty());
    }

    #[test]
    fn metadata_heavy_zero_byte_files_consume_the_shared_payload_budget() {
        let suffix = "x".repeat(700);
        let files = (0..1_024)
            .map(|index| {
                (
                    format!("references/{index}-{suffix}.md"),
                    serde_json::Value::String(String::new()),
                )
            })
            .collect();
        let response = pack_response("review", Some("1"), files);
        assert!(response.len() < REGISTRY_RESPONSE_LIMIT);
        let mut packs = Vec::new();

        let error = append("review-guid", &response, SKILL_PACK_FILE_LIMIT, &mut packs)
            .expect_err("metadata counts even when every file body is empty");

        assert!(error.contains("bounded representation"), "got {error}");
        assert!(packs.is_empty());

        let small = pack_response("review", Some(" 1 "), file_map(&[("empty", "")]));
        append("review-guid", &small, SKILL_PACK_FILE_LIMIT, &mut packs)
            .expect("a bounded zero-byte file is valid");
        assert_eq!(packs[0].version.as_deref(), Some("1"));
        assert!(packs[0].files[0].1.is_empty());
    }

    #[test]
    fn individually_bounded_files_cannot_exceed_the_collection_budget() {
        let content = "x".repeat(SKILL_PACK_PAYLOAD_LIMIT / 2);
        let first = pack_response("one", None, file_map(&[("SKILL.md", &content)]));
        let second = pack_response("two", None, file_map(&[("SKILL.md", &content)]));
        assert!(first.len() < REGISTRY_RESPONSE_LIMIT);
        assert!(second.len() < REGISTRY_RESPONSE_LIMIT);
        let mut packs = Vec::new();

        let remaining = append("one-guid", &first, SKILL_PACK_FILE_LIMIT, &mut packs)
            .expect("one half-limit pack fits");
        let error = append("two-guid", &second, remaining, &mut packs)
            .expect_err("the complete collection must fit one prelude budget");

        assert!(error.contains("bounded representation"), "got {error}");
        assert_eq!(packs.len(), 1, "the rejected candidate is removed");
    }

    #[tokio::test]
    async fn valid_registry_packs_are_fetched_in_request_order() {
        let (base, server) = registry(vec![
            PackReply::Full(pack_response(
                "one",
                Some(" 1.2.3 "),
                file_map(&[("SKILL.md", "first")]),
            )),
            PackReply::Full(pack_response(
                "two",
                None,
                file_map(&[("references/api.md", "second")]),
            )),
        ])
        .await;

        let packs = layer2_core::SkillSource::packs_for(
            &catalog(&base),
            Some("caller-token"),
            &[SkillId::new("guid-one"), SkillId::new("guid-two")],
        )
        .await
        .expect("bounded valid packs resolve");

        assert_eq!(
            packs
                .iter()
                .map(|pack| pack.name.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(packs[0].version.as_deref(), Some("1.2.3"));
        assert_eq!(packs[1].files[0].0, "references/api.md");
        server.abort();
    }
}
