//! Bounded REST collection queries and their opaque continuation links.
//!
//! The core owns stable store positions; REST owns how one travels in a query
//! string. Keeping the cursor opaque lets that representation change without
//! making a database key part of the public resource shape.

use axum::http::{header, HeaderMap, HeaderValue};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use layer2_core::{
    seconds_from_rfc3339, PageCursor, PageRequest, SessionListQuery, SessionStatus,
    DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE,
};
use serde::{de, Deserialize, Deserializer, Serialize};

/// Query accepted by either session collection.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionPageParams {
    #[serde(default)]
    status: Option<SessionStatus>,
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
    }

    pub(crate) fn next_headers(&self, path: &str, next: Option<&PageCursor>) -> HeaderMap {
        let Some(next) = next else {
            return HeaderMap::new();
        };
        let query = serde_urlencoded::to_string(SessionNextQuery {
            status: self.status,
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
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| de::Error::custom("cursor is not a valid continuation"))?;
    let cursor: WireCursor = serde_json::from_slice(&bytes)
        .map_err(|_| de::Error::custom("cursor is not a valid continuation"))?;
    if cursor.version != 1
        || cursor.collection != expected
        || seconds_from_rfc3339(&cursor.created_at).is_none()
        || cursor.id.is_empty()
    {
        return Err(de::Error::custom("cursor is not a valid continuation"));
    }
    Ok(Some(PageCursor::new(cursor.created_at, cursor.id)))
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
    fn a_structurally_mutated_cursor_is_refused() {
        let cursor = PageCursor::new("2026-08-24T10:11:12Z", "sesn_1");
        let mut encoded = encode_cursor(CursorCollection::Sessions, &cursor);
        encoded.pop();
        assert!(
            serde_urlencoded::from_str::<SessionPageParams>(&format!("cursor={encoded}")).is_err()
        );
    }
}
