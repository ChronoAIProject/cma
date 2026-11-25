use reqwest::{
    header::{HeaderMap, CONTENT_TYPE},
    Response,
};
use serde_json::Value;

use crate::{config::identifier, model::Reason};

pub const MAX_BODY: usize = 1024 * 1024;

pub fn content_type(response: &Response) -> Option<&str> {
    response
        .headers()
        .get(CONTENT_TYPE)?
        .to_str()
        .ok()?
        .split(';')
        .next()
        .map(str::trim)
}

pub fn status(response: &Response) -> Result<(), Reason> {
    match response.status().as_u16() {
        200 => Ok(()),
        401 if response
            .headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.to_ascii_lowercase().contains("expired")) =>
        {
            Err(Reason::CredentialExpired)
        }
        401 | 403 => Err(Reason::CredentialDenied),
        _ => Err(Reason::Http),
    }
}

pub async fn json(response: Response) -> Result<Value, Reason> {
    status(&response)?;
    if content_type(&response) != Some("application/json") {
        return Err(Reason::Contract);
    }
    serde_json::from_slice(&body(response, MAX_BODY).await?).map_err(|_| Reason::Contract)
}

pub async fn body(mut response: Response, limit: usize) -> Result<Vec<u8>, Reason> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(Reason::Contract);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Reason::Transport)? {
        if bytes.len() + chunk.len() > limit {
            return Err(Reason::Contract);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub fn id(value: &Value) -> Result<&str, Reason> {
    value
        .as_str()
        .filter(|v| identifier(v))
        .ok_or(Reason::Contract)
}

pub fn header_id(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    (values.next().is_none() && identifier(value)).then(|| value.to_owned())
}

pub fn same(value: &Value, expected: &str) -> Result<(), Reason> {
    (value.as_str() == Some(expected))
        .then_some(())
        .ok_or(Reason::Contract)
}
