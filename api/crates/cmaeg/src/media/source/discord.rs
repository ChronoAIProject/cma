//! Native Discord attachments resolved from current message metadata and bot custody.
use super::{
    url::{PinnedTransport, UrlTransport},
    FetchRequest, MediaSource, SourceStream,
};
use crate::{
    discord::{
        rest::{fetch_error, token},
        DiscordRest,
    },
    media::{
        body::{read_capped, stream_capped, FetchedBody},
        FetchError,
    },
};
use cmaeg_core::{
    activity::{AttachmentSource, SourceKind},
    seal::ReplyAddress,
};
use futures_util::future::BoxFuture;
use std::{sync::Arc, time::Duration};
use tokio::time::{timeout_at, Instant};

pub struct DiscordFileSource {
    api: DiscordRest,
    download: Arc<dyn UrlTransport>,
}
impl DiscordFileSource {
    pub fn new(api: DiscordRest) -> Self {
        Self {
            api,
            download: Arc::new(PinnedTransport),
        }
    }
    #[cfg(test)]
    pub(crate) fn with_download(mut self, download: Arc<dyn UrlTransport>) -> Self {
        self.download = download;
        self
    }
    async fn response(
        &self,
        request: &FetchRequest<'_>,
    ) -> Result<(reqwest::Response, Instant), FetchError> {
        let (
            SourceKind::Discord { bot_id },
            Some(AttachmentSource::Discord {
                channel_id,
                message_id,
                attachment_id,
            }),
        ) = (&request.activity.source, request.locator)
        else {
            return Err(FetchError::NotRetrievable);
        };
        if !matches!(&request.event.reply, ReplyAddress::Discord { bot_id: b, channel_id: c, message_id: m }
            if b == bot_id && c == channel_id && m == message_id)
            || request.activity.conversation.id != *channel_id
            || !request.event.verifies_activity(request.activity)
        {
            return Err(FetchError::NotRetrievable);
        }
        let credential = token(request.channel, bot_id).ok_or(FetchError::NotRetrievable)?;
        let deadline = request
            .deadline
            .min(Instant::now() + Duration::from_secs(30));
        let message = timeout_at(
            deadline,
            self.api.message(credential, channel_id, message_id),
        )
        .await
        .map_err(|_| FetchError::TimedOut)?
        .map_err(fetch_error)?;
        let files = message["attachments"]
            .as_array()
            .ok_or(FetchError::NotFound)?;
        let mut matching = files
            .iter()
            .filter(|file| file["id"].as_str() == Some(attachment_id));
        let file = matching.next().ok_or(FetchError::NotFound)?;
        if matching.next().is_some() {
            return Err(FetchError::Rejected);
        }
        if file["size"].as_u64().is_some_and(|size| size > request.cap)
            && request.overflow == crate::media::body::Overflow::Reject
        {
            return Err(FetchError::TooLarge);
        }
        let url = attachment_url(
            file["url"].as_str().ok_or(FetchError::NotFound)?,
            channel_id,
            attachment_id,
        )?;
        let response = timeout_at(deadline, self.download.get(&url, deadline))
            .await
            .map_err(|_| FetchError::TimedOut)??;
        match response.status().as_u16() {
            200..=299 => Ok((response, deadline)),
            404 => Err(FetchError::NotFound),
            429 => Err(FetchError::RateLimited),
            500..=599 => Err(FetchError::Upstream),
            _ => Err(FetchError::Rejected),
        }
    }
}
pub(crate) fn attachment_url(
    raw: &str,
    channel: &str,
    attachment: &str,
) -> Result<reqwest::Url, FetchError> {
    if raw.len() > 2048 || raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(FetchError::Rejected);
    }
    let url = reqwest::Url::parse(raw).map_err(|_| FetchError::Rejected)?;
    let parts: Vec<_> = url.path_segments().ok_or(FetchError::Rejected)?.collect();
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("cdn.discordapp.com" | "media.discordapp.net")
        )
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || parts.len() != 4
        || parts[0] != "attachments"
        || parts[1] != channel
        || parts[2] != attachment
        || parts[3].is_empty()
    {
        return Err(FetchError::Rejected);
    }
    Ok(url)
}
pub(super) fn configured(
    channel: &crate::channels::resolver::ResolvedChannel,
    source: &SourceKind,
) -> bool {
    matches!(source, SourceKind::Discord { bot_id } if token(channel, bot_id).is_some())
}
impl MediaSource for DiscordFileSource {
    fn fetch<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchedBody, FetchError>> {
        Box::pin(async move {
            let (response, deadline) = self.response(&request).await?;
            read_capped(response, request.cap, request.overflow, deadline).await
        })
    }
    fn open<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<SourceStream, FetchError>> {
        Box::pin(async move {
            let (response, _) = self.response(&request).await?;
            stream_capped(response, request.cap)
        })
    }
}
