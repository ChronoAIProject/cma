//! Exact-status, bounded-body HTTP boundary for REST resources.

use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime},
};

use reqwest::{Method, StatusCode, header};
use serde::{Deserialize, Serialize};

use crate::{Error, IdempotencyKey, Result, client::ClientInner};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct Request<'a> {
    method: Method,
    segments: &'a [&'a str],
    pub(crate) query: &'a [(&'a str, String)],
    body: Option<Vec<u8>>,
    key: Option<&'a IdempotencyKey>,
    pub(crate) statuses: Vec<StatusCode>,
    pub(crate) anonymous: bool,
    pub(crate) root: bool,
    pub(crate) headers: header::HeaderMap,
}

impl<'a> Request<'a> {
    pub(crate) fn get(segments: &'a [&'a str]) -> Self {
        Self {
            method: Method::GET,
            segments,
            query: &[],
            body: None,
            key: None,
            statuses: vec![StatusCode::OK],
            anonymous: false,
            root: false,
            headers: header::HeaderMap::new(),
        }
    }

    pub(crate) fn command<B: Serialize>(
        method: Method,
        segments: &'a [&'a str],
        body: Option<&B>,
        key: &'a IdempotencyKey,
        status: StatusCode,
    ) -> Result<Self> {
        let mut request = Self::operation(method, segments, body, status)?;
        request.key = Some(key);
        Ok(request)
    }

    pub(crate) fn with_command_key(mut self, key: Option<&str>) -> Result<Self> {
        if let Some(key) = key {
            if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
                return Err(Error::validation(
                    "idempotency key must contain 1 through 256 bytes without control characters",
                ));
            }
            self.headers.insert(
                "idempotency-key",
                header::HeaderValue::from_str(key)
                    .map_err(|_| Error::validation("idempotency key is not a valid HTTP header"))?,
            );
        }
        Ok(self)
    }

    pub(crate) fn operation<B: Serialize>(
        method: Method,
        segments: &'a [&'a str],
        body: Option<&B>,
        status: StatusCode,
    ) -> Result<Self> {
        Ok(Self {
            method,
            segments,
            query: &[],
            key: None,
            statuses: vec![status],
            anonymous: false,
            root: false,
            headers: header::HeaderMap::new(),
            body: body
                .map(serde_json::to_vec)
                .transpose()
                .map_err(|_| Error::validation("request body could not be encoded as JSON"))?,
        })
    }
}

pub(crate) async fn send(client: &ClientInner, request: Request<'_>) -> Result<Vec<u8>> {
    Ok(receive(client, request).await?.body)
}

pub(crate) struct ResponseBody {
    pub(crate) status: StatusCode,
    pub(crate) body: Vec<u8>,
    pub(crate) headers: header::HeaderMap,
}

pub(crate) async fn receive(client: &ClientInner, request: Request<'_>) -> Result<ResponseBody> {
    receive_bounded(client, request, MAX_RESPONSE_BYTES).await
}

pub(crate) async fn receive_bounded(
    client: &ClientInner,
    request: Request<'_>,
    max_bytes: usize,
) -> Result<ResponseBody> {
    let statuses = request.statuses.clone();
    let mut response = outgoing(client, request)?
        .send()
        .await
        .map_err(|_| Error::transport("request failed before a complete HTTP response"))?;
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    let retry_after = response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after);
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return too_large(max_bytes);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::transport("could not read response body"))?
    {
        if chunk.len() > max_bytes.saturating_sub(body.len()) {
            return too_large(max_bytes);
        }
        body.extend_from_slice(&chunk);
    }
    if !statuses.contains(&status) {
        if !status.is_success() {
            return Err(problem_error(status, request_id, retry_after, &body));
        }
        return Err(Error::protocol(format!(
            "operation received unexpected HTTP {}",
            status.as_u16()
        )));
    }
    Ok(ResponseBody {
        status,
        body,
        headers: response.headers().clone(),
    })
}

pub(crate) fn url(client: &ClientInner, segments: &[&str], root: bool) -> Result<reqwest::Url> {
    if segments
        .iter()
        .any(|segment| matches!(*segment, "" | "." | ".."))
    {
        return Err(Error::validation(
            "resource identifiers must be nonempty path segments",
        ));
    }
    let mut url = client.base_url.clone();
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| Error::protocol("invalid deployment path"))?;
        path.pop_if_empty();
        if !root {
            path.extend(["api", "v1"]);
        }
        path.extend(segments);
    }
    Ok(url)
}

pub(crate) fn outgoing(
    client: &ClientInner,
    request: Request<'_>,
) -> Result<reqwest::RequestBuilder> {
    let mut url = url(client, request.segments, request.root)?;
    if !request.query.is_empty() {
        url.query_pairs_mut()
            .extend_pairs(request.query.iter().map(|(key, value)| (*key, value)));
    }
    let mut outgoing = client
        .http
        .request(request.method, url)
        .header(header::ACCEPT, "application/json")
        .timeout(client.request_timeout);
    let has_content_type = request.headers.contains_key(header::CONTENT_TYPE);
    outgoing = outgoing.headers(request.headers);
    if !request.anonymous
        && let Some(bearer) = &client.bearer
    {
        outgoing = outgoing.header(header::AUTHORIZATION, bearer.clone());
    }
    if let Some(key) = request.key {
        outgoing = outgoing.header("Idempotency-Key", key.as_str());
    }
    if let Some(body) = request.body {
        if !has_content_type {
            outgoing = outgoing.header(header::CONTENT_TYPE, "application/json");
        }
        outgoing = outgoing.body(body);
    }
    Ok(outgoing)
}

fn too_large<T>(max_bytes: usize) -> Result<T> {
    Err(Error::protocol(format!(
        "response exceeds the {max_bytes}-byte boundary"
    )))
}

pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    let duration = if value.bytes().all(|byte| byte.is_ascii_digit()) {
        Duration::from_secs(value.parse().ok()?)
    } else {
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(SystemTime::now())
            .ok()?
    };
    (!duration.is_zero()).then_some(duration)
}

#[derive(Deserialize)]
struct Problem {
    status: Option<u16>,
    code: Option<String>,
    detail: Option<String>,
    title: Option<String>,
    #[serde(flatten)]
    extensions: BTreeMap<String, serde_json::Value>,
}

pub(crate) fn problem_error(
    status: StatusCode,
    request_id: Option<String>,
    retry_after: Option<Duration>,
    body: &[u8],
) -> Error {
    let Ok(mut problem) = serde_json::from_slice::<Problem>(body) else {
        return Error::protocol(format!(
            "HTTP {} did not carry a machine-readable problem document",
            status.as_u16()
        ));
    };
    if problem
        .status
        .is_some_and(|declared| declared != status.as_u16())
    {
        return Error::protocol(format!(
            "problem status did not match HTTP {}",
            status.as_u16()
        ));
    }
    if problem
        .detail
        .as_ref()
        .is_none_or(|value| value.trim().is_empty())
        && problem
            .title
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
        && let Some(error) = problem.extensions.remove("error")
    {
        let Some(value) = error.as_object() else {
            return Error::protocol("Responses error envelope is invalid");
        };
        let field = |name| {
            value
                .get(name)
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
        };
        let (Some(code), Some(message), Some(kind), Some(param)) = (
            field("code"),
            field("message"),
            field("type"),
            value.get("param"),
        ) else {
            return Error::protocol("Responses error fields are invalid");
        };
        if !param.is_null() && !param.is_string() {
            return Error::protocol("Responses error param is invalid");
        }
        return Error::Api {
            status: status.as_u16(),
            code: Some(code.into()),
            message: message.into(),
            request_id,
            retry_after,
            extensions: BTreeMap::from([
                ("type".into(), kind.into()),
                ("param".into(), param.clone()),
            ]),
        };
    }
    let message = problem
        .detail
        .filter(|value| !value.trim().is_empty())
        .or_else(|| problem.title.filter(|value| !value.trim().is_empty()));
    let Some(message) = message else {
        return Error::protocol(format!(
            "HTTP {} problem document carried no detail",
            status.as_u16()
        ));
    };
    problem.extensions.remove("type");
    problem.extensions.remove("instance");
    Error::Api {
        status: status.as_u16(),
        code: problem.code.filter(|value| !value.trim().is_empty()),
        message,
        request_id,
        retry_after,
        extensions: problem.extensions,
    }
}
