use axum::http::{header, HeaderMap, Method, Uri};
use serde::Serialize;

const EMPTY_BODY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Serialize)]
pub(super) struct Descriptor {
    pub version: u8,
    pub method: String,
    pub path_and_query: String,
    pub content_type: Option<String>,
    pub body_sha256: &'static str,
    pub idempotency_key_sha256: Option<String>,
}

impl Descriptor {
    pub(super) fn from_request(
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
    ) -> Result<Self, ()> {
        if !matches!(*method, Method::GET | Method::HEAD) || !read_route(uri.path()) {
            return Err(());
        }
        for name in [
            "idempotency-key",
            "x-http-method-override",
            "x-method-override",
            "x-http-method",
            "x-original-method",
            "x-method",
        ] {
            if headers.contains_key(name) {
                return Err(());
            }
        }
        if url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()).any(
            |(name, _)| {
                matches!(
                    name.to_ascii_lowercase().as_str(),
                    "_method"
                        | "method"
                        | "http_method"
                        | "x-http-method-override"
                        | "x-method-override"
                )
            },
        ) {
            return Err(());
        }
        if one_header(headers, header::CONTENT_ENCODING.as_str())?
            .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
            || one_header(headers, header::CONTENT_LENGTH.as_str())?
                .is_some_and(|value| value.parse::<u64>() != Ok(0))
        {
            return Err(());
        }
        let path_and_query = uri.path_and_query().ok_or(())?.as_str();
        if path_and_query.len() > 8 * 1024 {
            return Err(());
        }
        Ok(Self {
            version: 1,
            method: method.as_str().into(),
            path_and_query: path_and_query.into(),
            content_type: one_header(headers, header::CONTENT_TYPE.as_str())?.map(str::to_owned),
            body_sha256: EMPTY_BODY_SHA256,
            idempotency_key_sha256: None,
        })
    }
}

pub(super) fn one_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let first = values
        .next()
        .map(|value| value.to_str().map_err(|_| ()))
        .transpose()?;
    if values.next().is_some() {
        return Err(());
    }
    Ok(first)
}

/// Finite database reads only. Adding a route requires reviewing its effects;
/// GET alone does not prove that a handler has no credential or runtime effects.
fn read_route(path: &str) -> bool {
    let parts: Vec<_> = path.split('/').collect();
    let id = |value: &str| {
        !value.is_empty()
            && value.len() <= 256
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    };
    match parts.as_slice() {
        ["", "api", "v1", "agents" | "sandboxes"] => true,
        ["", "api", "v1", "agents" | "sandboxes" | "workspaces" | "worktrees", value] => id(value),
        ["", "api", "v1", "sandboxes", value, "workspaces"]
        | ["", "api", "v1", "workspaces", value, "worktrees"] => id(value),
        ["", "api", "v1", "agents", value, "transcript"] => id(value),
        ["", "api", "v2", "agents", value, "events", "snapshot"] => id(value),
        _ => false,
    }
}
