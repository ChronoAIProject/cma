//! Bounded REST collection queries and their opaque continuation links.
//!
//! The core owns stable store positions; REST owns how one travels in a query
//! string. Keeping the cursor opaque lets that representation change without
//! making a database key part of the public resource shape.

use axum::http::{header, HeaderMap, HeaderValue};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use layer2_core::{
    seconds_from_rfc3339, PageCursor, PageRequest, SessionListQuery, SessionStatus,
    SessionSurfaceFilter, TriggerValidationError, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE,
};
use serde::{de, Deserialize, Deserializer, Serialize};

/// Query accepted by either session collection.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionPageParams {
    #[serde(default)]
    status: Option<SessionStatus>,
    #[serde(default)]
    surface: Option<SessionSurfaceFilter>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_session_cursor")]
    cursor: Option<PageCursor>,
}

impl SessionPageParams {
    pub(crate) fn store_query(&self) -> SessionListQuery {
        SessionListQuery::new(
            PageRequest::new(bounded_limit(self.limit), self.cursor.clone()),
            self.status,
        )
        .with_surface(self.surface)
    }

    pub(crate) fn next_headers(&self, path: &str, next: Option<&PageCursor>) -> HeaderMap {
        let Some(next) = next else {
            return HeaderMap::new();
        };
        let query = serde_urlencoded::to_string(SessionNextQuery {
            status: self.status,
            surface: self.surface,
            limit: bounded_limit(self.limit),
            cursor: encode_cursor(CursorCollection::Sessions, next),
        })
        .expect("the fixed session page query serializes");
        link_headers(path, &query)
    }
}

/// Query accepted by the operator session collection.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminSessionPageParams {
    #[serde(default)]
    status: Option<SessionStatus>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_admin_session_cursor")]
    cursor: Option<PageCursor>,
}

impl AdminSessionPageParams {
    pub(crate) fn store_query(&self) -> SessionListQuery {
        SessionListQuery::new(
            PageRequest::new(bounded_limit(self.limit), self.cursor.clone()),
            self.status,
        )
    }

    pub(crate) fn next_headers(&self, path: &str, next: Option<&PageCursor>) -> HeaderMap {
        let Some(next) = next else {
            return HeaderMap::new();
        };
        let query = serde_urlencoded::to_string(SessionNextQuery {
            status: self.status,
            surface: None,
            limit: bounded_limit(self.limit),
            cursor: encode_cursor(CursorCollection::AdminSessions, next),
        })
        .expect("the fixed admin session page query serializes");
        link_headers(path, &query)
    }
}

#[derive(Serialize)]
struct SessionNextQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<SessionStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    surface: Option<SessionSurfaceFilter>,
    limit: u32,
    cursor: String,
}

/// Query accepted by a non-session collection.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageParams {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_profile_cursor")]
    cursor: Option<PageCursor>,
}

impl PageParams {
    pub(crate) fn store_request(&self) -> PageRequest {
        PageRequest::new(bounded_limit(self.limit), self.cursor.clone())
    }

    pub(crate) fn next_headers(&self, path: &str, next: Option<&PageCursor>) -> HeaderMap {
        let Some(next) = next else {
            return HeaderMap::new();
        };
        let query = serde_urlencoded::to_string(NextQuery {
            limit: bounded_limit(self.limit),
            cursor: encode_cursor(CursorCollection::EnvironmentProfiles, next),
        })
        .expect("the fixed page query serializes");
        link_headers(path, &query)
    }
}

#[derive(Serialize)]
struct NextQuery {
    limit: u32,
    cursor: String,
}

fn bounded_limit(requested: Option<u32>) -> u32 {
    requested
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CursorCollection {
    Sessions,
    AdminSessions,
    EnvironmentProfiles,
    SponsorshipCredentials,
    SponsorshipPolicies,
    SponsorshipContributorGrants,
    SponsorshipSponsorGrants,
}

/// Route identity for sponsorship continuations. Binding it into the cursor
/// prevents a valid owner cursor from being replayed against another listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SponsorshipPageCollection {
    Credentials,
    Policies,
    ContributorGrants,
    SponsorGrants,
}

impl SponsorshipPageCollection {
    fn cursor_collection(self) -> CursorCollection {
        match self {
            Self::Credentials => CursorCollection::SponsorshipCredentials,
            Self::Policies => CursorCollection::SponsorshipPolicies,
            Self::ContributorGrants => CursorCollection::SponsorshipContributorGrants,
            Self::SponsorGrants => CursorCollection::SponsorshipSponsorGrants,
        }
    }

    fn role(self) -> Option<&'static str> {
        match self {
            Self::ContributorGrants => Some("contributor"),
            Self::SponsorGrants => Some("sponsor"),
            Self::Credentials | Self::Policies => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SponsorshipPageParams {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    cursor: Option<String>,
}

impl SponsorshipPageParams {
    pub(crate) fn new(limit: Option<u32>, cursor: Option<String>) -> Self {
        Self { limit, cursor }
    }

    pub(crate) fn store_request(
        &self,
        collection: SponsorshipPageCollection,
    ) -> Result<PageRequest, TriggerValidationError> {
        let cursor = self
            .cursor
            .as_deref()
            .map(|cursor| decode_cursor_value(cursor, collection.cursor_collection()))
            .transpose()
            .map_err(|_| TriggerValidationError::field("cursor", "is not a valid continuation"))?;
        Ok(PageRequest::new(bounded_limit(self.limit), cursor))
    }

    pub(crate) fn next_headers(
        &self,
        path: &str,
        collection: SponsorshipPageCollection,
        next: Option<&PageCursor>,
    ) -> HeaderMap {
        let Some(next) = next else {
            return HeaderMap::new();
        };
        let mut query = vec![
            ("limit", bounded_limit(self.limit).to_string()),
            (
                "cursor",
                encode_cursor(collection.cursor_collection(), next),
            ),
        ];
        if let Some(role) = collection.role() {
            query.insert(0, ("role", role.to_string()));
        }
        let query = serde_urlencoded::to_string(query)
            .expect("the fixed sponsorship page query serializes");
        link_headers(path, &query)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCursor {
    version: u8,
    collection: CursorCollection,
    created_at: String,
    id: String,
}

fn encode_cursor(collection: CursorCollection, cursor: &PageCursor) -> String {
    let bytes = serde_json::to_vec(&WireCursor {
        version: 1,
        collection,
        created_at: cursor.created_at.clone(),
        id: cursor.id.clone(),
    })
    .expect("a cursor made only of strings serializes");
    URL_SAFE_NO_PAD.encode(bytes)
}

fn deserialize_session_cursor<'de, D>(deserializer: D) -> Result<Option<PageCursor>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_cursor(deserializer, CursorCollection::Sessions)
}

fn deserialize_admin_session_cursor<'de, D>(deserializer: D) -> Result<Option<PageCursor>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_cursor(deserializer, CursorCollection::AdminSessions)
}

fn deserialize_profile_cursor<'de, D>(deserializer: D) -> Result<Option<PageCursor>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_cursor(deserializer, CursorCollection::EnvironmentProfiles)
}

fn deserialize_cursor<'de, D>(
    deserializer: D,
    expected: CursorCollection,
) -> Result<Option<PageCursor>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(encoded) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };
    decode_cursor_value(&encoded, expected)
        .map(Some)
        .map_err(|_| de::Error::custom("cursor is not a valid continuation"))
}

fn decode_cursor_value(encoded: &str, expected: CursorCollection) -> Result<PageCursor, ()> {
    const MAX_CURSOR_BYTES: usize = 2_048;
    if encoded.is_empty() || encoded.len() > MAX_CURSOR_BYTES {
        return Err(());
    }
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| ())?;
    if bytes.len() > MAX_CURSOR_BYTES {
        return Err(());
    }
    let cursor: WireCursor = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if cursor.version != 1
        || cursor.collection != expected
        || seconds_from_rfc3339(&cursor.created_at).is_none()
        || cursor.id.is_empty()
        || cursor.id.len() > 512
    {
        return Err(());
    }
    Ok(PageCursor::new(cursor.created_at, cursor.id))
}

fn link_headers(path: &str, query: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let value = format!("<{path}?{query}>; rel=\"next\"");
    headers.insert(
        header::LINK,
        HeaderValue::from_str(&value).expect("an encoded continuation is a valid Link header"),
    );
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_default_and_clamp_at_both_ends() {
        assert_eq!(bounded_limit(None), 50);
        assert_eq!(bounded_limit(Some(0)), 1);
        assert_eq!(bounded_limit(Some(12)), 12);
        assert_eq!(bounded_limit(Some(201)), 200);
    }

    #[test]
    fn a_link_cursor_round_trips_through_the_query_decoder() {
        let original = PageCursor::new("2026-08-24T10:11:12Z", "sesn/a value");
        let query = format!(
            "cursor={}",
            encode_cursor(CursorCollection::Sessions, &original)
        );
        let decoded: SessionPageParams = serde_urlencoded::from_str(&query).expect("cursor parses");
        assert_eq!(decoded.cursor, Some(original));
    }

    #[test]
    fn session_surface_filters_parse_and_survive_continuation_links() {
        for (wire, expected) in [
            ("chat", SessionSurfaceFilter::Chat),
            ("agent", SessionSurfaceFilter::Agent),
        ] {
            let params: SessionPageParams =
                serde_urlencoded::from_str(&format!("surface={wire}&limit=7"))
                    .expect("a documented surface parses");
            assert_eq!(params.store_query().surface, Some(expected));

            let next = PageCursor::new("2026-08-24T10:11:12Z", "sesn_surface");
            let headers = params.next_headers("/api/v1/sessions", Some(&next));
            let link = headers[header::LINK].to_str().unwrap();
            assert!(link.contains(&format!("surface={wire}")), "{link}");
            assert!(link.contains("limit=7"), "{link}");
        }

        assert!(serde_urlencoded::from_str::<SessionPageParams>("surface=workspace").is_err());
    }

    #[test]
    fn a_cursor_is_bound_to_its_collection() {
        let cursor = PageCursor::new("2026-08-24T10:11:12Z", "sesn_1");
        let query = format!(
            "cursor={}",
            encode_cursor(CursorCollection::Sessions, &cursor)
        );
        assert!(serde_urlencoded::from_str::<PageParams>(&query).is_err());
        assert!(serde_urlencoded::from_str::<AdminSessionPageParams>(&query).is_err());
    }

    #[test]
    fn a_cursor_requires_a_canonical_semantic_timestamp() {
        let encoded = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&WireCursor {
                version: 1,
                collection: CursorCollection::Sessions,
                created_at: "not-a-timestamp".into(),
                id: "sesn_1".into(),
            })
            .expect("test cursor serializes"),
        );
        assert!(
            serde_urlencoded::from_str::<SessionPageParams>(&format!("cursor={encoded}")).is_err()
        );
    }

    #[test]
    fn an_unsupported_cursor_version_is_refused() {
        let encoded = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&WireCursor {
                version: 2,
                collection: CursorCollection::Sessions,
                created_at: "2026-08-24T10:11:12Z".into(),
                id: "sesn_1".into(),
            })
            .expect("test cursor serializes"),
        );
        assert!(
            serde_urlencoded::from_str::<SessionPageParams>(&format!("cursor={encoded}")).is_err()
        );
    }

    #[test]
    fn an_empty_cursor_identifier_is_refused() {
        let encoded = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&WireCursor {
                version: 1,
                collection: CursorCollection::Sessions,
                created_at: "2026-08-24T10:11:12Z".into(),
                id: String::new(),
            })
            .expect("test cursor serializes"),
        );
        assert!(
            serde_urlencoded::from_str::<SessionPageParams>(&format!("cursor={encoded}")).is_err()
        );
    }

    #[test]
    fn an_unknown_cursor_payload_field_is_refused() {
        let encoded = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "collection": "sessions",
                "created_at": "2026-08-24T10:11:12Z",
                "id": "sesn_1",
                "offset": 1,
            }))
            .expect("test cursor serializes"),
        );
        assert!(
            serde_urlencoded::from_str::<SessionPageParams>(&format!("cursor={encoded}")).is_err()
        );
    }

    #[test]
    fn a_truncated_cursor_is_refused() {
        let cursor = PageCursor::new("2026-08-24T10:11:12Z", "sesn_1");
        let mut encoded = encode_cursor(CursorCollection::Sessions, &cursor);
        encoded.pop();
        assert!(
            serde_urlencoded::from_str::<SessionPageParams>(&format!("cursor={encoded}")).is_err()
        );
    }

    #[test]
    fn sponsorship_cursors_are_bound_to_route_and_grant_role() {
        let position = PageCursor::new("2026-08-24T10:11:12Z", "grant-1");
        let contributor = SponsorshipPageParams::new(
            Some(10),
            Some(encode_cursor(
                CursorCollection::SponsorshipContributorGrants,
                &position,
            )),
        );
        assert_eq!(
            contributor
                .store_request(SponsorshipPageCollection::ContributorGrants)
                .unwrap()
                .cursor(),
            Some(&position)
        );
        assert!(contributor
            .store_request(SponsorshipPageCollection::SponsorGrants)
            .is_err());
        assert!(contributor
            .store_request(SponsorshipPageCollection::Credentials)
            .is_err());
    }

    #[test]
    fn sponsorship_next_links_preserve_the_grant_role() {
        let params = SponsorshipPageParams::new(Some(7), None);
        let next = PageCursor::new("2026-08-24T10:11:12Z", "grant-1");
        let headers = params.next_headers(
            "/api/v1/sponsorships/grants",
            SponsorshipPageCollection::SponsorGrants,
            Some(&next),
        );
        let link = headers[header::LINK].to_str().unwrap();
        assert!(link.starts_with("</api/v1/sponsorships/grants?"));
        assert!(link.contains("role=sponsor"));
        assert!(link.contains("limit=7"));
        assert!(link.ends_with(">; rel=\"next\""));
    }
}
