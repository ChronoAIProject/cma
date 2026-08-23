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

use std::sync::Arc;

use layer2_core::{CatalogError, CatalogSkill, SkillCatalog, SkillId, SubjectId};

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
/// The registry caps a page at 100. The picker shows a flat list, so a single
/// page is fetched rather than paging: a caller with more than this many
/// skills is better served by search than by scrolling, and unbounded paging
/// here would let one request fan out into dozens.
const PAGE_SIZE: usize = 100;

/// A skill as the registry describes it.
///
/// Deliberately forward-compatible: fields the registry adds later are ignored
/// rather than breaking the picker.
#[derive(Debug, serde::Deserialize)]
struct RegistrySkill {
    guid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    /// Why this caller can see it. Absent for an anonymous listing.
    #[serde(default, rename = "myAccessReason")]
    access_reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct SearchPage {
    #[serde(default)]
    items: Vec<RegistrySkill>,
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
        let resp = self
            .http
            .get(&url)
            .query(&[
                ("scope", SCOPE),
                ("pageSize", &PAGE_SIZE.to_string()),
                ("mode", "keyword"),
            ])
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| CatalogError::Unavailable(format!("GET {url}: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            // The body can name the reason but is also where a proxy echoes a
            // request back, so the status alone is reported.
            return Err(CatalogError::Unavailable(format!(
                "skill search returned {status}"
            )));
        }
        let body: SearchResponse = resp
            .json()
            .await
            .map_err(|e| CatalogError::Malformed(format!("skill search response: {e}")))?;

        let items = body.data.map(|d| d.items).unwrap_or_default();
        let skills: Vec<CatalogSkill> = items
            .into_iter()
            // A skill with no identifier cannot be stored in a profile, so it
            // is dropped rather than offered and then rejected on save.
            .filter(|s| !s.guid.trim().is_empty())
            .map(|s| CatalogSkill {
                id: SkillId::new(s.guid),
                name: s.name,
                description: s.description,
                access: s.access_reason,
            })
            .collect();
        tracing::info!(
            %subject,
            count = skills.len(),
            delegated = credential.is_some(),
            "listed selectable skills"
        );
        Ok(skills)
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
                "meta":{"hasMore":false}},"error":null}"#,
        )
        .expect("forward-compatible");
        assert_eq!(body.data.expect("has data").items[0].guid, "g1");
    }

    #[test]
    fn a_response_with_no_data_is_an_empty_list_rather_than_an_error() {
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
    files: std::collections::BTreeMap<String, String>,
}

#[async_trait::async_trait]
impl layer2_core::SkillSource for PlatformSkillCatalog {
    async fn packs_for(
        &self,
        caller_credential: Option<&str>,
        skills: &[layer2_core::SkillId],
    ) -> Result<Vec<layer2_core::SkillPack>, String> {
        let bearer = self
            .bearer(caller_credential)
            .await
            .map_err(|e| e.to_string())?;
        let mut packs = Vec::with_capacity(skills.len());
        for skill in skills {
            let url = format!("{}{PACK_PATH}/{}/json", self.base_url, skill.0);
            let response = self
                .http
                .get(&url)
                .bearer_auth(&bearer)
                .send()
                .await
                .map_err(|e| format!("{} could not be fetched: {e}", skill.0))?;
            // Named rather than swallowed. A session that starts without a
            // skill it was promised looks exactly like one that never asked
            // for it, and the difference only shows when the agent cannot do
            // what it was told it could.
            if !response.status().is_success() {
                return Err(format!(
                    "{} could not be fetched: the registry answered {}",
                    skill.0,
                    response.status()
                ));
            }
            let body: PackResponse = response
                .json()
                .await
                .map_err(|e| format!("{} came back in a shape we cannot read: {e}", skill.0))?;
            let body = body
                .data
                .ok_or_else(|| format!("{} came back empty", skill.0))?;
            if body.files.is_empty() {
                return Err(format!("{} carries no files", skill.0));
            }
            // The registry's own name for it, because that is what the agent
            // invokes — the identifier may be a guid nobody types.
            let name = match body.name.trim().is_empty() {
                true => skill.0.clone(),
                false => body.name.clone(),
            };
            packs.push(layer2_core::SkillPack {
                name,
                // Kept from the fetch, so a session reports the version it was
                // actually given rather than whatever is current when somebody
                // opens its details months later.
                version: body
                    .version
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty()),
                files: body
                    .files
                    .into_iter()
                    .map(|(path, content)| (path, content.into_bytes()))
                    .collect(),
            });
        }
        tracing::info!(skills = packs.len(), "fetched this session's skills");
        Ok(packs)
    }
}
