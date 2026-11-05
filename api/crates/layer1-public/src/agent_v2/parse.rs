use super::error::Error;
use axum::http::HeaderMap;
use std::collections::BTreeMap;

fn query(raw: Option<String>, allowed: &[&str]) -> Result<BTreeMap<String, String>, Error> {
    let raw = raw.unwrap_or_default();
    if raw.len() > 1024 {
        return Err(Error::Query);
    }
    let entries: Vec<(String, String)> =
        serde_urlencoded::from_str(&raw).map_err(|_| Error::Query)?;
    let mut result = BTreeMap::new();
    for (key, value) in entries {
        if !allowed.contains(&key.as_str()) || result.insert(key, value).is_some() {
            return Err(Error::Query);
        }
    }
    Ok(result)
}
pub(super) fn head(raw: Option<String>) -> Result<(), Error> {
    query(raw, &[]).map(|_| ())
}
pub(super) fn snapshot(raw: Option<String>) -> Result<(u16, Option<String>), Error> {
    page_query(raw, "page_cursor")
}
pub(super) fn history(raw: Option<String>) -> Result<(u16, Option<String>), Error> {
    page_query(raw, "through_cursor")
}
pub(super) fn current_state(
    raw: Option<String>,
) -> Result<(Option<String>, Option<String>), Error> {
    let mut query = query(raw, &["through_cursor", "after_approval_cursor"])?;
    let through = query.remove("through_cursor");
    let after = query.remove("after_approval_cursor");
    if after.is_some() && through.is_none() {
        return Err(Error::Query);
    }
    Ok((through, after))
}
fn page_query(raw: Option<String>, cursor: &str) -> Result<(u16, Option<String>), Error> {
    let mut query = query(raw, &["limit", cursor])?;
    let limit = match query.remove("limit") {
        None => 100,
        Some(value) => value
            .parse::<u16>()
            .ok()
            .filter(|n| (1..=200).contains(n))
            .filter(|n| n.to_string() == value)
            .ok_or(Error::Query)?,
    };
    Ok((limit, query.remove(cursor)))
}
pub(super) fn tail(
    raw: Option<String>,
    headers: &HeaderMap,
) -> Result<(Option<String>, bool), Error> {
    let mut query = query(raw, &["after_cursor", "follow"])?;
    let follow = match query.remove("follow").as_deref() {
        None | Some("true") => true,
        Some("false") => false,
        _ => return Err(Error::Query),
    };
    if headers.get_all("last-event-id").iter().count() > 1 {
        return Err(Error::Query);
    }
    let header = headers
        .get("last-event-id")
        .map(|v| v.to_str().map(str::to_owned))
        .transpose()
        .map_err(|_| Error::Cursor)?;
    let cursor = query.remove("after_cursor");
    if cursor
        .as_ref()
        .zip(header.as_ref())
        .is_some_and(|(a, b)| a != b)
    {
        return Err(Error::Cursor);
    }
    Ok((cursor.or(header), follow))
}
