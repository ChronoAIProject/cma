//! Native media is resolved afresh using exact signed app/account/phone/message facts.
use super::{
    whatsapp_body,
    whatsapp_transport::{self, Transport},
    FetchRequest, MediaSource, SourceStream,
};
use crate::{
    channels::resolver::ResolvedChannel,
    media::{
        body::{read_capped, FetchedBody, Overflow},
        FetchError,
    },
};
use cmaeg_core::{
    activity::{ActivityKind, AttachmentSource, SourceKind},
    channel::{
        whatsapp::{media_sha256, valid_meta_id},
        Secret, Source,
    },
    seal::ReplyAddress,
};
use futures_util::future::BoxFuture;
use reqwest::{Response, Url};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio::time::{timeout_at, Instant};

const MAX_BYTES: u64 = 100_000_000;
const METADATA_CAP: u64 = 32 * 1024;

pub struct WhatsappFileSource {
    transport: Arc<dyn Transport>,
}
impl WhatsappFileSource {
    pub fn new() -> Self {
        Self {
            transport: Arc::new(whatsapp_transport::Pinned),
        }
    }

    async fn metadata(
        &self,
        phone: &str,
        media: &str,
        digest: [u8; 32],
        mime: Option<&str>,
        token: &Secret,
        deadline: Instant,
    ) -> Result<(Url, u64), FetchError> {
        if !valid_meta_id(media) || !valid_meta_id(phone) {
            return Err(FetchError::Rejected);
        }
        let mut url = Url::parse(&format!(
            "https://graph.facebook.com/{}/{media}",
            crate::whatsapp::GRAPH_API_VERSION
        ))
        .map_err(|_| FetchError::Rejected)?;
        url.query_pairs_mut().append_pair("phone_number_id", phone);
        let response = self.get(&url, token, deadline).await?;
        whatsapp_transport::status(response.status())?;
        let body = read_capped(response, METADATA_CAP, Overflow::Reject, deadline).await?;
        let meta: Value = serde_json::from_slice(&body.bytes).map_err(|_| FetchError::Rejected)?;
        if meta["id"].as_str() != Some(media)
            || meta["messaging_product"] != "whatsapp"
            || meta["sha256"].as_str().and_then(media_sha256) != Some(digest)
        {
            return Err(FetchError::Rejected);
        }
        let actual_mime = meta["mime_type"]
            .as_str()
            .filter(|s| s.len() <= 512)
            .ok_or(FetchError::Rejected)?;
        if mime.is_some_and(|expected| {
            !actual_mime
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case(expected)
        }) {
            return Err(FetchError::Rejected);
        }
        let size = meta["file_size"]
            .as_u64()
            .or_else(|| {
                meta["file_size"]
                    .as_str()
                    .filter(|s| {
                        !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
                    })
                    .and_then(|s| s.parse().ok())
            })
            .ok_or(FetchError::Rejected)?;
        if size > MAX_BYTES {
            return Err(FetchError::TooLarge);
        }
        let url = whatsapp_transport::download_url(
            meta["url"].as_str().ok_or(FetchError::Rejected)?,
            media,
        )?;
        Ok((url, size))
    }

    async fn get(
        &self,
        url: &Url,
        token: &Secret,
        deadline: Instant,
    ) -> Result<Response, FetchError> {
        timeout_at(deadline, self.transport.get(url, token, deadline))
            .await
            .map_err(|_| FetchError::TimedOut)?
    }

    async fn response(
        &self,
        request: &FetchRequest<'_>,
    ) -> Result<(Response, Instant, u64, [u8; 32]), FetchError> {
        let (
            SourceKind::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
            },
            Some(
                locator @ AttachmentSource::Whatsapp {
                    message_id,
                    media_id,
                    sha256,
                },
            ),
        ) = (&request.activity.source, request.locator)
        else {
            return Err(FetchError::NotRetrievable);
        };
        let exact = match &request.event.reply {
            ReplyAddress::Whatsapp {
                app_id: a,
                waba_id: w,
                phone_number_id: p,
                recipient_id: r,
                message_id: m,
            } => {
                a == app_id
                    && w == waba_id
                    && p == phone_number_id
                    && m == message_id
                    && r == &request.activity.actor.id
                    && r == &request.activity.conversation.id
            }
            ReplyAddress::WhatsappGroup(reply) => {
                &reply.app_id == app_id
                    && &reply.waba_id == waba_id
                    && &reply.phone_number_id == phone_number_id
                    && &reply.message_id == message_id
                    && reply.participant_id == request.activity.actor.id
                    && reply.group_id == request.activity.conversation.id
                    && request.activity.conversation.kind
                        == cmaeg_core::activity::ConversationKind::Group
            }
            _ => false,
        };
        if !exact
            || request.event.channel_id != request.channel.id
            || request.event.record_id != request.channel.pointer.record_id
            || !request.event.verifies_activity(request.activity)
        {
            return Err(FetchError::NotRetrievable);
        }
        let ActivityKind::Message { attachments, .. } = &request.activity.kind else {
            return Err(FetchError::NotRetrievable);
        };
        let attachment = attachments
            .iter()
            .find(|a| a.source.as_ref() == Some(locator))
            .ok_or(FetchError::NotRetrievable)?;
        let token =
            &crate::whatsapp::credentials(&request.channel.definition, &request.event.reply)
                .ok_or(FetchError::Unauthorized)?
                .access_token;
        let digest = media_sha256(sha256).ok_or(FetchError::Rejected)?;
        let deadline = request
            .deadline
            .min(Instant::now() + Duration::from_secs(30));
        for attempt in 0..2 {
            let (url, size) = self
                .metadata(
                    phone_number_id,
                    media_id,
                    digest,
                    attachment.mime_type.as_deref(),
                    token,
                    deadline,
                )
                .await?;
            if size > request.cap && request.overflow == Overflow::Reject {
                return Err(FetchError::TooLarge);
            }
            let response = self.get(&url, token, deadline).await?;
            // Meta recommends refreshing a five-minute URL once after a 404.
            if attempt == 0 && response.status() == reqwest::StatusCode::NOT_FOUND {
                continue;
            }
            whatsapp_transport::status(response.status())?;
            if response.content_length().is_some_and(|n| n != size) {
                return Err(FetchError::Rejected);
            }
            return Ok((response, deadline, size, digest));
        }
        Err(FetchError::NotFound)
    }
}

impl Default for WhatsappFileSource {
    fn default() -> Self {
        Self::new()
    }
}
pub(super) fn configured(channel: &ResolvedChannel, source: &SourceKind) -> bool {
    let SourceKind::Whatsapp {
        app_id,
        waba_id,
        phone_number_id,
    } = source
    else {
        return false;
    };
    channel.definition.secrets.whatsapp.is_some()
        && channel.definition.sources.iter().any(|s| {
            matches!(s, Source::Whatsapp { app_id: a, waba_id: w, phone_number_id: p, .. }
            if a == app_id && w == waba_id && p == phone_number_id)
        })
}
impl MediaSource for WhatsappFileSource {
    fn fetch<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchedBody, FetchError>> {
        Box::pin(async move {
            let (response, deadline, size, digest) = self.response(&request).await?;
            whatsapp_body::fetch(
                response,
                request.cap.min(MAX_BYTES),
                request.overflow,
                deadline,
                size,
                digest,
            )
            .await
        })
    }
    fn open<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<SourceStream, FetchError>> {
        Box::pin(async move {
            let request = FetchRequest {
                overflow: Overflow::Reject,
                ..request
            };
            let (response, deadline, size, digest) = self.response(&request).await?;
            whatsapp_body::open(response, request.cap.min(MAX_BYTES), deadline, size, digest)
        })
    }
}

#[cfg(test)]
#[path = "whatsapp_tests.rs"]
mod tests;
