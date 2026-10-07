//! Relayed attachments, through NyxID's attachment endpoint.
//!
//! NyxID buffers each file, caps it at 20 MiB and answers within 30 s. The
//! NyxID message ID and the reply token live only in the sealed reply address
//! of the event_ref, so an Activity alone can never direct a fetch. The reply
//! token and the channel key are sent to NyxID and nowhere else.
use super::{FetchRequest, MediaSource, SourceCredential, SourceStream};
use crate::media::{
    body::{read_capped, stream_capped, transport, FetchedBody},
    FetchError,
};
use cmaeg_core::{activity::AttachmentSource, channel::Secret, seal::ReplyAddress};
use futures_util::future::BoxFuture;
use reqwest::{StatusCode, Url};
use std::time::Duration;
use tokio::time::{timeout_at, Instant};

/// NyxID refuses larger attachments itself.
pub const NYXID_MAX_BYTES: u64 = 20 * 1024 * 1024;
/// NyxID's own deadline for one attachment.
const NYXID_DEADLINE: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// The protocol's identifier bound; NyxID message IDs are far shorter.
const MAX_MESSAGE_ID_BYTES: usize = 512;

/// Fetches `nyxid_relay{index}` locators.
pub struct NyxidSource {
    http: reqwest::Client,
    base: Url,
}

impl NyxidSource {
    /// `base` is the validated `CMAEG_NYXID_BASE_URL` origin.
    pub fn new(base: &str) -> Result<Self, crate::ConfigError> {
        let invalid = || crate::ConfigError::InvalidMedia {
            detail: "cannot configure the NyxID attachment client".into(),
        };
        let base = Url::parse(base).map_err(|_| invalid())?;
        if base.cannot_be_a_base() {
            return Err(invalid());
        }
        let http = reqwest::Client::builder()
            .use_rustls_tls()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|_| invalid())?;
        Ok(Self { http, base })
    }

    /// The successful response and the deadline its body must meet.
    async fn response(
        &self,
        request: &FetchRequest<'_>,
    ) -> Result<(reqwest::Response, Instant), FetchError> {
        let ReplyAddress::NyxidRelay {
            message_id,
            reply_token,
            reply_token_expires_at,
            ..
        } = &request.event.reply
        else {
            return Err(FetchError::NotRetrievable);
        };
        let Some(AttachmentSource::NyxidRelay { index }) = request.locator else {
            return Err(FetchError::NotRetrievable);
        };
        let url = self.attachment_url(message_id, *index)?;
        let deadline = request.deadline.min(Instant::now() + NYXID_DEADLINE);
        let channel_key = request.channel.definition.secrets.channel_key.as_ref();
        let token_live = *reply_token_expires_at > time::OffsetDateTime::now_utc().unix_timestamp();
        let reply_first = request.credential == SourceCredential::ReplyToken && token_live;
        let first = if reply_first {
            reply_token
        } else {
            channel_key.ok_or(FetchError::Unauthorized)?
        };
        let mut response = self.send(&url, first, deadline).await?;
        if reply_first && refused(response.status()) {
            if let Some(key) = channel_key {
                tracing::debug!(
                    status = response.status().as_u16(),
                    "relay attachment refused the reply token; retrying once with the channel key"
                );
                drop(response);
                response = self.send(&url, key, deadline).await?;
            }
        }
        classify(response.status())?;
        Ok((response, deadline))
    }

    /// `{base}/api/v1/channel-relay/messages/{message_id}/attachments/{index}`,
    /// each segment percent-encoded. Dot segments would be dropped by URL
    /// normalization and name another path, so they are refused.
    fn attachment_url(&self, message_id: &str, index: u8) -> Result<Url, FetchError> {
        if message_id.is_empty()
            || message_id.len() > MAX_MESSAGE_ID_BYTES
            || matches!(message_id, "." | "..")
        {
            return Err(FetchError::NotRetrievable);
        }
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| FetchError::NotRetrievable)?
            .pop_if_empty()
            .extend([
                "api",
                "v1",
                "channel-relay",
                "messages",
                message_id,
                "attachments",
                &index.to_string(),
            ]);
        Ok(url)
    }

    async fn send(
        &self,
        url: &Url,
        bearer: &Secret,
        deadline: Instant,
    ) -> Result<reqwest::Response, FetchError> {
        // The client-side timeout also bounds an opened stream's body.
        let request = self
            .http
            .get(url.clone())
            .bearer_auth(bearer.as_str())
            .timeout(deadline.saturating_duration_since(Instant::now()));
        match timeout_at(deadline, request.send()).await {
            Err(_) => Err(FetchError::TimedOut),
            Ok(Err(error)) => Err(transport(error)),
            Ok(Ok(response)) => Ok(response),
        }
    }
}

fn refused(status: StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 403)
}

/// NyxID's status, as a fixed classification. Redirects are never followed,
/// so a 3xx is a refusal.
fn classify(status: StatusCode) -> Result<(), FetchError> {
    match status.as_u16() {
        200..=299 => Ok(()),
        404 => Err(FetchError::NotFound),
        413 => Err(FetchError::TooLarge),
        429 => Err(FetchError::RateLimited),
        401 | 403 => Err(FetchError::Unauthorized),
        503 => Err(FetchError::Busy),
        500..=599 => Err(FetchError::Upstream),
        _ => Err(FetchError::Rejected),
    }
}

impl MediaSource for NyxidSource {
    fn fetch<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchedBody, FetchError>> {
        Box::pin(async move {
            let (response, deadline) = self.response(&request).await?;
            read_capped(
                response,
                request.cap.min(NYXID_MAX_BYTES),
                request.overflow,
                deadline,
            )
            .await
        })
    }

    fn open<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<SourceStream, FetchError>> {
        Box::pin(async move {
            let (response, _) = self.response(&request).await?;
            stream_capped(response, request.cap.min(NYXID_MAX_BYTES))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_fixed_classifications() {
        for (status, expected) in [
            (200, Ok(())),
            (404, Err(FetchError::NotFound)),
            (413, Err(FetchError::TooLarge)),
            (429, Err(FetchError::RateLimited)),
            (401, Err(FetchError::Unauthorized)),
            (403, Err(FetchError::Unauthorized)),
            (400, Err(FetchError::Rejected)),
            (410, Err(FetchError::Rejected)),
            (302, Err(FetchError::Rejected)),
            (503, Err(FetchError::Busy)),
            (500, Err(FetchError::Upstream)),
            (502, Err(FetchError::Upstream)),
        ] {
            assert_eq!(
                classify(StatusCode::from_u16(status).unwrap()),
                expected,
                "{status}"
            );
        }
    }

    #[test]
    fn segments_are_encoded_and_dot_segments_refused() {
        let source = NyxidSource::new("https://nyx.example").unwrap();
        assert_eq!(
            source.attachment_url("m/1?x#y", 3).unwrap().as_str(),
            "https://nyx.example/api/v1/channel-relay/messages/m%2F1%3Fx%23y/attachments/3"
        );
        for message_id in ["", ".", ".."] {
            assert_eq!(
                source.attachment_url(message_id, 0).unwrap_err(),
                FetchError::NotRetrievable,
                "{message_id:?}"
            );
        }
        assert!(source.attachment_url(&"m".repeat(513), 0).is_err());
        assert!(NyxidSource::new("mailto:nyx@example").is_err());
    }
}
