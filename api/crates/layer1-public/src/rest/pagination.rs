//! Bounded REST collection queries and their opaque continuation links.
//!
//! The core owns stable store positions; REST owns how one travels in a query
//! string. Keeping the cursor opaque lets that representation change without
//! making a database key part of the public resource shape.

use axum::http::{header, HeaderMap, HeaderValue};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use layer2_core::{
    PageCursor, PageRequest, SessionListQuery, SessionStatus, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE,
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
    #[serde(default, deserialize_with = "deserialize_cursor")]
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
            cursor: encode_cursor(next),
        })
        .expect("the fixed session page query serializes");
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
    #[serde(default, deserialize_with = "deserialize_cursor")]
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
            cursor: encode_cursor(next),
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

#[derive(Serialize, Deserialize)]
struct WireCursor {
    created_at: String,
    id: String,
}

fn encode_cursor(cursor: &PageCursor) -> String {
    let bytes = serde_json::to_vec(&WireCursor {
        created_at: cursor.created_at.clone(),
        id: cursor.id.clone(),
    })
    .expect("a cursor made only of strings serializes");
    URL_SAFE_NO_PAD.encode(bytes)
}

fn deserialize_cursor<'de, D>(deserializer: D) -> Result<Option<PageCursor>, D::Error>
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
    if cursor.created_at.is_empty() || cursor.id.is_empty() {
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
        let query = format!("cursor={}", encode_cursor(&original));
        let decoded: PageParams = serde_urlencoded::from_str(&query).expect("cursor parses");
        assert_eq!(decoded.cursor, Some(original));
    }
}
