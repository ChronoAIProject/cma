//! Exact native message resources, using a fresh regional token and bot custody.
use super::{
    native_lark_authority as authority,
    native_lark_transport::{Pinned, Transport},
    FetchRequest, MediaSource, SourceStream,
};
use crate::{
    channels::resolver::ChannelResolver,
    media::{
        body::{read_capped, stream_capped, FetchedBody},
        FetchError,
    },
    native_lark::Client,
};
use cmaeg_core::{activity::AttachmentSource, seal::ReplyAddress};
use futures_util::{future::BoxFuture, StreamExt};
use reqwest::{Response, Url};
use std::{sync::Arc, time::Duration};
use tokio::time::{timeout_at, Instant};

/// Both regional contracts support whole resources strictly below 100 MB.
/// Regional Range support is not shared; callers also impose smaller pipeline caps.
pub const MAX_BYTES: u64 = 100_000_000 - 1;
pub struct NativeLarkFileSource {
    client: Client,
    resolver: ChannelResolver,
    transport: Arc<dyn Transport>,
}
impl NativeLarkFileSource {
    pub fn new(client: Client, resolver: ChannelResolver) -> Self {
        Self {
            client,
            resolver,
            transport: Arc::new(Pinned),
        }
    }
    async fn response(
        &self,
        request: &FetchRequest<'_>,
    ) -> Result<(Response, Instant), FetchError> {
        let deadline = request
            .deadline
            .min(Instant::now() + Duration::from_secs(30));
        let request = FetchRequest {
            deadline,
            ..*request
        };
        let current = authority::refresh(&self.resolver, &request).await?;
        let ReplyAddress::NativeLark(reply) = &request.event.reply else {
            return Err(FetchError::Unauthorized);
        };
        let credentials =
            crate::native_lark::credentials(&current.definition, &request.event.reply)
                .ok_or(FetchError::Unauthorized)?;
        let token = timeout_at(
            deadline,
            self.client.verified_token(&reply.identity, credentials),
        )
        .await
        .map_err(|_| FetchError::TimedOut)?
        .map_err(|error| match error {
            crate::native_lark::Error::RateLimited => FetchError::RateLimited,
            crate::native_lark::Error::Rejected | crate::native_lark::Error::Invalid => {
                FetchError::Unauthorized
            }
            crate::native_lark::Error::Unconfirmed => FetchError::Upstream,
        })?;
        // Rotation or removal while token/custody reads were in flight revokes it.
        authority::refresh(&self.resolver, &request).await?;
        let Some(AttachmentSource::NativeLark {
            message_id,
            resource_key,
            resource_kind,
            ..
        }) = request.locator
        else {
            return Err(FetchError::NotRetrievable);
        };
        let mut url =
            Url::parse(reply.identity.region.origin()).map_err(|_| FetchError::Rejected)?;
        url.path_segments_mut()
            .map_err(|_| FetchError::Rejected)?
            .extend([
                "open-apis",
                "im",
                "v1",
                "messages",
                message_id,
                "resources",
                resource_key,
            ]);
        url.query_pairs_mut()
            .append_pair("type", resource_kind.as_str());
        let response = timeout_at(deadline, self.transport.get(&url, &token, deadline))
            .await
            .map_err(|_| FetchError::TimedOut)??;
        match response.status().as_u16() {
            200 => {}
            401 | 403 => return Err(FetchError::Unauthorized),
            404 => return Err(FetchError::NotFound),
            429 => return Err(FetchError::RateLimited),
            503 => return Err(FetchError::Busy),
            500..=599 => return Err(FetchError::Upstream),
            _ => return Err(FetchError::Rejected),
        }
        if response.content_length().is_some_and(|n| n > MAX_BYTES) {
            return Err(FetchError::TooLarge);
        }
        authority::refresh(&self.resolver, &request).await?;
        Ok((response, deadline))
    }
}
impl MediaSource for NativeLarkFileSource {
    fn fetch<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchedBody, FetchError>> {
        Box::pin(async move {
            let (response, deadline) = self.response(&request).await?;
            let body = read_capped(
                response,
                request.cap.min(MAX_BYTES),
                request.overflow,
                deadline,
            )
            .await?;
            authority::refresh(
                &self.resolver,
                &FetchRequest {
                    deadline,
                    ..request
                },
            )
            .await?;
            Ok(body)
        })
    }
    fn open<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<SourceStream, FetchError>> {
        Box::pin(async move {
            let (response, deadline) = self.response(&request).await?;
            let opened = stream_capped(response, request.cap.min(MAX_BYTES))?;
            let stream = futures_util::stream::unfold(
                (opened.stream, false),
                move |(mut stream, ended)| async move {
                    if ended {
                        return None;
                    }
                    match timeout_at(deadline, stream.next()).await {
                        Ok(Some(item)) => {
                            let ended = item.is_err();
                            Some((item, (stream, ended)))
                        }
                        Ok(None) => None,
                        Err(_) => Some((Err(FetchError::TimedOut), (stream, true))),
                    }
                },
            )
            .boxed();
            Ok(SourceStream {
                content_length: opened.content_length,
                stream,
            })
        })
    }
}

#[cfg(test)]
#[path = "native_lark_tests.rs"]
mod tests;
