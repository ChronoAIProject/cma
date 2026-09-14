//! Recovery is limited to owned, immutable parts and their final manifest.

use std::time::{Duration, SystemTime};

use futures_util::StreamExt;
use reqwest::{header, Request, Response, StatusCode};
use sha2::{Digest, Sha256};

use super::{multipart::PART_BYTES, rejected, unavailable, ChronoStorage, Result, UPLOAD_LIFETIME};

const ATTEMPTS: u32 = 3;

struct Failure {
    stage: &'static str,
    status: Option<StatusCode>,
    retryable: bool,
    retry_after: Duration,
}

impl Failure {
    fn permanent(stage: &'static str) -> Self {
        Self {
            stage,
            status: None,
            retryable: false,
            retry_after: Duration::ZERO,
        }
    }

    fn transport(stage: &'static str, error: reqwest::Error) -> Self {
        Self {
            retryable: !error.is_builder() && !error.is_redirect(),
            ..Self::permanent(stage)
        }
    }

    fn response(stage: &'static str, response: &Response) -> Self {
        let status = response.status();
        Self {
            stage,
            status: Some(status),
            retryable: transient(status),
            retry_after: retry_after(response.headers(), SystemTime::now()),
        }
    }

    fn report(&self, key_sha256: &str, attempt: u32) {
        tracing::warn!(
            stage = self.stage,
            status = self.status.map(|status| status.as_u16()),
            attempt,
            retryable = self.retryable,
            retry_after_seconds = self.retry_after.as_secs(),
            object_key_sha256 = key_sha256,
            "checkpoint immutable part transfer failed"
        );
    }

    fn error(&self) -> layer3_adapter::AdapterError {
        layer3_adapter::AdapterError::Unavailable(format!(
            "checkpoint immutable part {} failed (HTTP {})",
            self.stage,
            self.status
                .map_or_else(|| "none".into(), |status| status.as_u16().to_string())
        ))
    }
}

pub(super) async fn upload(store: &ChronoStorage, key: &str, bytes: Vec<u8>) -> Result<String> {
    if bytes.is_empty() || bytes.len() > PART_BYTES {
        return Err(rejected("checkpoint immutable part size is invalid"));
    }
    let size = bytes.len() as u64;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let key_sha256 = format!("{:x}", Sha256::digest(key.as_bytes()));
    // A reusable reqwest body shares these exact bytes across attempts without copying a part.
    let post = store
        .http
        .post(store.objects.clone())
        .query(&[
            ("key", key),
            ("contentType", "application/vnd.chrono.cma-checkpoint.v1"),
        ])
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, size)
        .body(bytes)
        .build()
        .map_err(|_| unavailable("checkpoint immutable part request is invalid"))?;
    let get = store
        .http
        .get(store.download.clone())
        .query(&[("key", key)])
        .build()
        .map_err(|_| unavailable("checkpoint immutable part request is invalid"))?;
    let mut needs_post = true;
    let mut confirmed = false;
    for attempt in 1..=ATTEMPTS {
        if needs_post {
            match post_once(store, &post).await {
                Ok(()) => confirmed = true,
                Err(failure) => {
                    failure.report(&key_sha256, attempt);
                    if !failure.retryable {
                        return Err(failure.error());
                    }
                    // Even the last uncertain POST can be acknowledged by exact readback.
                    wait_for_retry(attempt, failure.retry_after).await;
                }
            }
            needs_post = false;
        }
        match read_back(store, &get, size, &sha256).await {
            Ok(()) => return Ok(sha256),
            Err(failure) => {
                failure.report(&key_sha256, attempt);
                if !confirmed && failure.status == Some(StatusCode::NOT_FOUND) {
                    needs_post = true;
                } else if failure.retryable {
                    if attempt < ATTEMPTS {
                        wait_for_retry(attempt, failure.retry_after).await;
                    }
                } else {
                    return Err(failure.error());
                }
            }
        }
    }
    Err(unavailable(
        "checkpoint immutable part retry budget exhausted",
    ))
}

async fn send(
    store: &ChronoStorage,
    template: &Request,
    stage: &'static str,
) -> std::result::Result<Response, Failure> {
    let mut request = template
        .try_clone()
        .ok_or_else(|| Failure::permanent("request"))?;
    let token = store
        .tokens
        .token()
        .await
        .map_err(|_| Failure::permanent("authentication"))?;
    let mut authorization = header::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| Failure::permanent("authentication"))?;
    authorization.set_sensitive(true);
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, authorization);
    store
        .http
        .execute(request)
        .await
        .map_err(|error| Failure::transport(stage, error))
}

async fn post_once(store: &ChronoStorage, request: &Request) -> std::result::Result<(), Failure> {
    let response = send(store, request, "upload").await?;
    if !response.status().is_success() {
        return Err(Failure::response("upload", &response));
    }
    Ok(())
}

async fn read_back(
    store: &ChronoStorage,
    request: &Request,
    expected_size: u64,
    expected_sha: &str,
) -> std::result::Result<(), Failure> {
    let response = send(store, request, "readback").await?;
    if !response.status().is_success() {
        return Err(Failure::response("readback", &response));
    }
    if response
        .content_length()
        .is_some_and(|size| size != expected_size)
    {
        return Err(Failure::permanent("readback_size"));
    }
    let mut stream = response.bytes_stream();
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    // Readback stays inside the owning future, so cancellation drops all transfer work.
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| Failure::transport("readback_body", error))?;
        size = size
            .checked_add(chunk.len() as u64)
            .filter(|size| *size <= expected_size)
            .ok_or_else(|| Failure::permanent("readback_size"))?;
        digest.update(&chunk);
    }
    if size != expected_size || format!("{:x}", digest.finalize()) != expected_sha {
        return Err(Failure::permanent("readback_integrity"));
    }
    Ok(())
}

fn transient(status: StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504)
}

fn retry_after(headers: &header::HeaderMap, now: SystemTime) -> Duration {
    let Some(value) = headers
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
    else {
        return Duration::ZERO;
    };
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Duration::from_secs(value.parse().unwrap_or(u64::MAX));
    }
    httpdate::parse_http_date(value)
        .ok()
        .and_then(|date| date.duration_since(now).ok())
        .unwrap_or_default()
}

async fn wait_for_retry(attempt: u32, retry_after: Duration) {
    let delay = retry_after.max(Duration::from_secs(u64::from(attempt)));
    if delay >= UPLOAD_LIFETIME {
        // The enclosing staged upload expires first; never shorten a server's delay or overflow Instant.
        std::future::pending::<()>().await;
    } else {
        tokio::time::sleep(delay).await;
    }
}

#[cfg(test)]
mod tests;
