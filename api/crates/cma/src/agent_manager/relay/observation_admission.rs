//! Peer-only admission envelope. Its HTTP headers acknowledge capacity and
//! custody; its bounded first record carries the eventual public response head.
//! The public caller never sees the envelope or a premature successful status.
use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use futures_util::{stream, Stream, StreamExt, TryStreamExt};
use std::{future::Future, io};

pub(super) const HEADER: &str = "x-cma-read-admission";
pub(super) const VERSION: &str = "1";
const MAX_HEAD: usize = 64 * 1024;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Head {
    status: u16,
    headers: Vec<(String, Vec<u8>)>,
}

pub(super) fn requested(headers: &HeaderMap) -> bool {
    headers.get_all(HEADER).iter().count() == 1
        && headers.get(HEADER).is_some_and(|value| value == VERSION)
}

fn public_header(name: &str) -> bool {
    !matches!(
        name,
        HEADER
            | "connection"
            | "transfer-encoding"
            | "keep-alive"
            | "upgrade"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "trailer"
    )
}

fn encode(response: &Response) -> Result<Bytes, io::Error> {
    if response
        .headers()
        .iter()
        .map(|(name, value)| name.as_str().len() + value.len())
        .sum::<usize>()
        > MAX_HEAD
    {
        return Err(io::Error::other("read relay head exceeds bound"));
    }
    let head = serde_json::to_vec(&Head {
        status: response.status().as_u16(),
        headers: response
            .headers()
            .iter()
            .filter(|(name, _)| public_header(name.as_str()))
            .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
            .collect(),
    })?;
    if head.len() > MAX_HEAD {
        return Err(io::Error::other("read relay head exceeds bound"));
    }
    let mut record = Vec::with_capacity(4 + head.len());
    record.extend_from_slice(&(head.len() as u32).to_be_bytes());
    record.extend_from_slice(&head);
    Ok(record.into())
}

pub(super) fn acknowledge(response: impl Future<Output = Response> + Send + 'static) -> Response {
    // No detached task or prefetch queue. Dropping the peer body cancels cold
    // reconstruction and releases the enclosing relay permit and body budget.
    let body = stream::once(async move {
        let response = response.await;
        let head = encode(&response)?;
        Ok::<_, io::Error>(
            stream::once(async move { Ok(head) }).chain(
                response
                    .into_body()
                    .into_data_stream()
                    .map_err(io::Error::other),
            ),
        )
    })
    .try_flatten();
    let mut response = Response::new(Body::from_stream(body));
    response
        .headers_mut()
        .insert(HEADER, HeaderValue::from_static(VERSION));
    response
}

async fn take<S, E>(stream: &mut S, pending: &mut Bytes, length: usize) -> Result<Vec<u8>, ()>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    let mut bytes = Vec::with_capacity(length);
    while bytes.len() < length {
        if pending.is_empty() {
            *pending = stream.next().await.ok_or(())?.map_err(|_| ())?;
        }
        let count = pending.len().min(length - bytes.len());
        bytes.extend_from_slice(&pending.split_to(count));
    }
    Ok(bytes)
}

async fn decode<S, E>(mut stream: S) -> Result<Response, ()>
where
    S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let mut pending = Bytes::new();
    let length = take(&mut stream, &mut pending, 4).await?;
    let length = u32::from_be_bytes(length.try_into().map_err(|_| ())?) as usize;
    if length == 0 || length > MAX_HEAD {
        return Err(());
    }
    let head: Head =
        serde_json::from_slice(&take(&mut stream, &mut pending, length).await?).map_err(|_| ())?;
    let status = StatusCode::from_u16(head.status).map_err(|_| ())?;
    if status.is_informational() {
        return Err(());
    }
    let mut headers = HeaderMap::new();
    for (name, value) in head.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| ())?;
        let value = HeaderValue::from_bytes(&value).map_err(|_| ())?;
        if public_header(name.as_str()) {
            headers.append(name, value);
        }
    }
    let body = stream::once(async move { Ok::<_, E>(pending) }).chain(stream);
    let mut response = Response::new(Body::from_stream(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

pub(super) async fn response(response: reqwest::Response) -> Result<Response, ()> {
    if response.headers().contains_key(HEADER) {
        if response.status() != StatusCode::OK || !requested(response.headers()) {
            return Err(());
        }
        return decode(response.bytes_stream()).await;
    }
    // Older peers do not negotiate the envelope. Preserve their exact public
    // response; their header wait remains subject to the admission deadline.
    let status = response.status();
    let headers = response.headers().clone();
    let mut response = Response::new(Body::from_stream(response.bytes_stream()));
    *response.status_mut() = status;
    for (name, value) in &headers {
        if public_header(name.as_str()) {
            response.headers_mut().append(name, value.clone());
        }
    }
    Ok(response)
}

#[cfg(test)]
#[path = "observation_admission_tests.rs"]
mod tests;
