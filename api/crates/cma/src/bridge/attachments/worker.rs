//! Deferred bounded capture under current exact Channel invocation authority.
use layer2_core::bridge::{
    ingress::{CapturedAttachmentText, InboxAttachment, StoredInbox},
    invoke::{BridgeInvokeProof, BridgeInvokeRequest, BridgeInvokeService},
    BridgePlatform,
};
use zeroize::Zeroizing;

use super::*;
use crate::{
    bridge::channel_scheduler::ChannelWorkError as Error,
    store::{MongoBridgeInvokeStore, MongoCallbackInbox},
};

pub(crate) async fn capture(
    inbox: &StoredInbox,
    proof: &BridgeInvokeProof,
    invokes: &BridgeInvokeService,
    custody: &MongoCallbackInbox,
    store: &MongoBridgeInvokeStore,
) -> Result<StoredInbox, Error> {
    capture_using(
        inbox,
        proof,
        invokes,
        custody,
        store,
        (&network::SystemResolver, &network::HttpsTransport),
    )
    .await
}

pub(super) async fn capture_using(
    inbox: &StoredInbox,
    proof: &BridgeInvokeProof,
    invokes: &BridgeInvokeService,
    custody: &MongoCallbackInbox,
    store: &MongoBridgeInvokeStore,
    io: (&dyn Resolver, &dyn Transport),
) -> Result<StoredInbox, Error> {
    let Some(metadata) = &inbox.text_attachment else {
        return Ok(inbox.clone());
    };
    let now = mongodb::bson::DateTime::now().timestamp_millis() / 1000;
    metadata
        .require_custody(inbox, now)
        .map_err(|_| Error::AttachmentExpired)?;
    if metadata.captured.is_some() {
        return Ok(inbox.clone());
    }
    let url = custody
        .text_attachment_url(inbox, proof, store)
        .await
        .map_err(|_| Error::AttachmentUnavailable)?;
    let request = FetchRequest {
        source: SourceBinding {
            owner: inbox.owner.clone(),
            bridge: inbox.pin.clone(),
            bot_id: inbox.bot_id.clone(),
            platform: BridgePlatform::Discord,
            source_id: inbox.id.clone(),
        },
        reference: InboxAttachment {
            content_type: "text".into(),
            url: String::new(),
            platform_message_id: Some(inbox.message_id.clone()),
            file_key: Some(metadata.attachment_id.clone()),
            image_key: None,
            filename: Some(metadata.filename.clone()),
            mime_type: Some("text/plain".into()),
            size_bytes: Some(metadata.size_bytes),
        },
    };
    let authority = DiscordAuthority {
        inbox,
        proof,
        invokes,
        request: &request,
        url,
    };
    let fetched = retrieve(&request, &authority, io.0, io.1, &UnavailableScanner)
        .await
        .map_err(|_| Error::AttachmentUnavailable)?;
    if fetched.source != request.source
        || fetched.mime != Mime::Text
        || fetched.bytes.len() as u64 != metadata.size_bytes
    {
        return Err(Error::AttachmentUnavailable);
    }
    let text = std::str::from_utf8(&fetched.bytes)
        .map_err(|_| Error::AttachmentUnavailable)?
        .to_owned();
    store
        .capture_text_attachment(
            proof,
            inbox,
            &CapturedAttachmentText {
                text,
                sha256: fetched.sha256,
            },
        )
        .await
        .map_err(|_| Error::AttachmentUnavailable)
}

struct DiscordAuthority<'a> {
    inbox: &'a StoredInbox,
    proof: &'a BridgeInvokeProof,
    invokes: &'a BridgeInvokeService,
    request: &'a FetchRequest,
    url: Zeroizing<String>,
}

impl DiscordAuthority<'_> {
    async fn current(&self) -> Result<(), Unavailable> {
        self.proof
            .require_current()
            .map_err(|_| Unavailable::Authorization)?;
        let metadata = self
            .inbox
            .text_attachment
            .as_ref()
            .ok_or(Unavailable::Authorization)?;
        let now = mongodb::bson::DateTime::now().timestamp_millis() / 1000;
        metadata
            .require_custody(self.inbox, now)
            .map_err(|_| Unavailable::Deadline)?;
        if discord::signed_url(
            &self.url,
            &self.inbox.envelope.chat_id,
            &metadata.attachment_id,
            &metadata.filename,
            now,
        ) != Some(metadata.source_expires_at)
        {
            return Err(Unavailable::Authorization);
        }
        let fresh = self
            .invokes
            .authorize(BridgeInvokeRequest {
                grant_id: self.proof.grant().id.clone(),
                pin: self.inbox.pin.clone(),
                target: self.proof.grant().target.ids.clone(),
                effect: self.proof.effect().clone(),
            })
            .await
            .map_err(|_| Unavailable::Authorization)?;
        if fresh.grant() != self.proof.grant()
            || fresh.bridge() != self.proof.bridge()
            || fresh.custody_pin() != self.proof.custody_pin()
        {
            return Err(Unavailable::Authorization);
        }
        self.proof
            .require_current()
            .map_err(|_| Unavailable::Authorization)
    }
}

#[async_trait::async_trait]
impl ConnectorAuthority for DiscordAuthority<'_> {
    async fn authorize(&self, request: &FetchRequest) -> Result<FetchGrant, Unavailable> {
        if request != self.request {
            return Err(Unavailable::Authorization);
        }
        self.current().await?;
        let metadata = self
            .inbox
            .text_attachment
            .as_ref()
            .ok_or(Unavailable::Authorization)?;
        let remaining = metadata
            .custody_expires_at
            .saturating_mul(1000)
            .saturating_sub(mongodb::bson::DateTime::now().timestamp_millis());
        if remaining <= 0 {
            return Err(Unavailable::Deadline);
        }
        FetchGrant::issue(
            request.clone(),
            &ConnectorHosts::new(&["cdn.discordapp.com"])?,
            &self.url,
            Mime::Text,
            metadata.size_bytes as usize,
            self.proof
                .deadline()
                .min(Instant::now() + MAX_DURATION)
                .min(Instant::now() + std::time::Duration::from_millis(remaining as u64)),
        )
    }
    async fn revalidate(&self, grant: &FetchGrant) -> Result<(), Unavailable> {
        if grant.request != *self.request || grant.url.as_str() != self.url.as_str() {
            return Err(Unavailable::Authorization);
        }
        self.current().await
    }
}
