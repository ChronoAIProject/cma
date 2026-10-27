//! Durable claims and shared message fences prevent uncertain native edits being resent.
use super::{
    authority, denied,
    model::{Current, Original, Snapshot},
    store::Store,
    Client,
};
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    delivery::intents::{DeliveryOutcome, IntentReceipt},
    media::outbound,
    tools::{message_effects::mutation, reply_targets::receipts, ToolsState},
};
use cmaeg_core::{
    ids::MessageRef,
    message::{NativeMediaUpdate, RenderingOutcome},
    problem::{Problem, ProblemCode},
    seal::{native_media_edit::NativeMediaDestination as Destination, MessageRefPayload},
};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

impl ToolsState {
    pub(crate) async fn native_media_update(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        patch: &NativeMediaUpdate,
        key: Option<&str>,
    ) -> Result<IntentReceipt, Problem> {
        let key = key.ok_or_else(|| {
            Problem::new(
                ProblemCode::InvalidRequest,
                400,
                "native_media_update_requires_idempotency_key",
            )
        })?;
        let (channel, message, _) = self
            .authorize_message(caller, reference, "nativeMediaUpdate")
            .await?;
        let a = message.native_media_edit.as_ref().ok_or_else(denied)?;
        if a.expires_at <= super::now() {
            return Err(Problem::new(
                ProblemCode::EventRefExpired,
                410,
                "native_media_authority_expired",
            ));
        }
        let store = Store::new(self.resolver.record_client(), self.keys.clone());
        let (original, _) = store.load(&a.original_sha256).await?;
        authority::check(&channel, &message, &original, caller)?;
        if patch.replacement.as_ref().is_some_and(|r| r.kind != a.kind)
            || (matches!(a.destination, Destination::Telegram { .. }) && patch.text.is_some())
        {
            return Err(denied());
        }
        let client = self.native_media_edits.as_ref().ok_or_else(denied)?;
        let _permit = client.capacity.try_acquire().map_err(|_| {
            Problem::new(
                ProblemCode::RateLimited,
                429,
                "native_media_update_capacity",
            )
        })?;
        let destination = mutation::destination(
            a.destination.bot(),
            a.destination.channel(),
            &message.platform_message_id,
        );
        let claims = receipts::Store::new(self.resolver.record_client(), self.keys.clone());
        let scope = mutation::hash(
            &serde_json::json!({"destination":destination,"channel":message.channel_id,"record":a.record_id,"consumer":message.consumer,"original":a.original_sha256}),
        );
        let claim = match claims
            .claim_scoped(
                "native_media_update_v1",
                "cmaeg.native-media-update.v1",
                scope,
                key,
                mutation::hash(&serde_json::json!({"revision":a.revision,"patch":patch})),
                &original.owner,
                a.expires_at,
            )
            .await?
        {
            receipts::Admission::Replay(receipt) => return Ok(receipt),
            receipts::Admission::Claimed(claim) => claim,
        };
        let serial = match a.destination {
            Destination::Telegram { .. } => {
                mutation::Store::telegram_media(self.resolver.record_client(), self.keys.clone())
            }
            Destination::Discord { .. } => {
                mutation::Store::discord(self.resolver.record_client(), self.keys.clone())
            }
        };
        let held = match serial
            .acquire(destination, &original.owner, a.expires_at)
            .await?
        {
            Some(held) => held,
            None => {
                let receipt = unknown();
                claims.finish(claim, &receipt).await?;
                return Ok(receipt);
            }
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(65),
            self.execute_media(caller, reference, patch, client, &store, claim.id()),
        )
        .await;
        let receipt = outcome.unwrap_or_else(|_| unknown());
        if claims.finish(claim, &receipt).await.is_err() {
            return Ok(unknown());
        }
        if !matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }) {
            let _ = serial.release(held).await;
        }
        Ok(receipt)
    }
    async fn media_current(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        store: &Store,
    ) -> Result<(Arc<ResolvedChannel>, MessageRefPayload, Original, Current), Problem> {
        let (channel, message, _) = self
            .authorize_message(caller, reference, "nativeMediaUpdate")
            .await?;
        let a = message.native_media_edit.as_ref().ok_or_else(denied)?;
        let (original, current) = store.load(&a.original_sha256).await?;
        authority::check(&channel, &message, &original, caller)?;
        if a.revision != current.revision {
            return Err(Problem::new(
                ProblemCode::Conflict,
                409,
                "native_media_revision_stale",
            ));
        }
        Ok((channel, message, original, current))
    }
    async fn execute_media(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        patch: &NativeMediaUpdate,
        client: &Client,
        store: &Store,
        operation: &str,
    ) -> IntentReceipt {
        let Ok((channel, message, original, current)) =
            self.media_current(caller, reference, store).await
        else {
            return rejected("native_media_authority_or_revision_changed");
        };
        let a = message
            .native_media_edit
            .as_ref()
            .expect("checked authority");
        let Some(token) = authority::token(&channel, &a.destination) else {
            return rejected("native_media_credential_unavailable");
        };
        let token_hash = crate::channels::definition::hash_secret(token.as_str());
        let mut file = if let Some(replacement) = &patch.replacement {
            match outbound::prepare(&[replacement.attachment()]).await {
                Ok(mut f) if f.len() == 1 => f.pop(),
                _ => return rejected("attachment_preparation_failed"),
            }
        } else {
            None
        };
        let intended = serde_json::json!({"caption":patch.caption.as_ref().unwrap_or(&current.snapshot.caption),
            "content":patch.text.as_ref().unwrap_or(&current.snapshot.content),
            "file":file.as_ref().map(|f| serde_json::json!({"sha256":format!("{:x}",Sha256::digest(&f.bytes)),"size":f.bytes.len(),"filename":f.filename,"mime":f.mime}))});
        if store.freeze(&original, operation, &intended).await.is_err() {
            return rejected("native_media_attempt_unavailable");
        }
        let observed = match &a.destination {
            Destination::Telegram {
                bot_id,
                chat_id,
                thread_id,
            } => {
                if client
                    .telegram
                    .get_me(token)
                    .await
                    .ok()
                    .is_none_or(|b| b.id != *bot_id)
                {
                    return rejected("native_media_bot_custody_unavailable");
                }
                let caption = patch
                    .caption
                    .as_deref()
                    .unwrap_or(&current.snapshot.caption);
                let edit = match client
                    .telegram
                    .prepare_media_edit(
                        chat_id,
                        &message.platform_message_id,
                        a.kind,
                        caption,
                        file.take(),
                    )
                    .await
                {
                    Ok(edit) => edit,
                    Err(_) => return rejected("native_media_request_invalid"),
                };
                if !self
                    .media_final_check(caller, reference, store, &token_hash, current.version)
                    .await
                {
                    return rejected("native_media_authority_changed");
                }
                match client.telegram.edit_media(token, edit).await {
                    Ok(body) => crate::telegram::media_edit::snapshot(
                        &body,
                        bot_id,
                        chat_id,
                        thread_id.as_deref(),
                        &message.platform_message_id,
                        a.kind,
                    )
                    .ok(),
                    Err(crate::telegram::Error::Rejected) => {
                        return rejected("native_media_update_rejected")
                    }
                    Err(crate::telegram::Error::RateLimited(retry_after)) => {
                        return result(DeliveryOutcome::RateLimited { retry_after })
                    }
                    Err(_) => None,
                }
            }
            Destination::Discord { bot_id, channel_id } => {
                if client
                    .discord
                    .verify_media_bot(token, bot_id)
                    .await
                    .is_err()
                {
                    return rejected("native_media_bot_custody_unavailable");
                }
                if client
                    .discord
                    .media_channel(token, channel_id, &original.activity.conversation)
                    .await
                    .ok()
                    .as_ref()
                    != original.discord_channel.as_ref()
                {
                    return rejected("native_media_channel_changed");
                }
                let before = client
                    .discord
                    .message(token, channel_id, &message.platform_message_id)
                    .await
                    .ok()
                    .and_then(|body| {
                        crate::discord::rest::media_edit::snapshot(
                            &body,
                            bot_id,
                            channel_id,
                            &message.platform_message_id,
                            a.kind,
                        )
                        .ok()
                    });
                if before.as_ref() != Some(&current.snapshot) {
                    return rejected("native_media_message_changed");
                }
                let edit = match client
                    .discord
                    .prepare_media_edit(patch, &current.snapshot, file.take())
                    .await
                {
                    Ok(edit) => edit,
                    Err(_) => return rejected("native_media_request_invalid"),
                };
                if !self
                    .media_final_check(caller, reference, store, &token_hash, current.version)
                    .await
                {
                    return rejected("native_media_authority_changed");
                }
                match client
                    .discord
                    .edit_media(token, channel_id, &message.platform_message_id, edit)
                    .await
                {
                    Ok(body) => crate::discord::rest::media_edit::snapshot(
                        &body,
                        bot_id,
                        channel_id,
                        &message.platform_message_id,
                        a.kind,
                    )
                    .ok(),
                    Err(crate::discord::rest::RestError::RateLimited(retry_after)) => {
                        return result(DeliveryOutcome::RateLimited { retry_after })
                    }
                    Err(
                        crate::discord::rest::RestError::Rejected
                        | crate::discord::rest::RestError::NotFound
                        | crate::discord::rest::RestError::Unavailable,
                    ) => return rejected("native_media_update_rejected"),
                    Err(_) => None,
                }
            }
        };
        let Some(snapshot) = observed else {
            return unknown();
        };
        if !verify_result(
            &a.destination,
            patch,
            &current.snapshot,
            &snapshot,
            &intended,
        ) {
            return unknown();
        }
        let Ok(next) = store
            .advance(&original, &current, snapshot, operation)
            .await
        else {
            return unknown();
        };
        let Some(next_reference) =
            authority::reference(&original, &next, &self.keys, Some(reference))
        else {
            return unknown();
        };
        let mut receipt = result(DeliveryOutcome::Confirmed {
            platform_message_id: Some(message.platform_message_id),
        });
        receipt.rendering.outcome = RenderingOutcome::Rendered;
        receipt.message_ref = Some(next_reference);
        receipt
    }
    async fn media_final_check(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        store: &Store,
        token_hash: &str,
        version: u64,
    ) -> bool {
        let Ok((channel, message, _, current)) = self.media_current(caller, reference, store).await
        else {
            return false;
        };
        current.version == version
            && authority::token(
                &channel,
                &message
                    .native_media_edit
                    .as_ref()
                    .expect("checked")
                    .destination,
            )
            .is_some_and(|t| crate::channels::definition::hash_secret(t.as_str()) == token_hash)
    }
}
pub(crate) fn verify_result(
    destination: &Destination,
    patch: &NativeMediaUpdate,
    before: &Snapshot,
    after: &Snapshot,
    intended: &serde_json::Value,
) -> bool {
    if after.kind != before.kind
        || Some(after.caption.as_str()) != intended["caption"].as_str()
        || Some(after.content.as_str()) != intended["content"].as_str()
    {
        return false;
    }
    if patch.replacement.is_none() {
        return after.same_files(before);
    }
    match destination {
        Destination::Discord { .. } => {
            after.files.len() == 1
                && after.files[0].filename.as_deref() == intended["file"]["filename"].as_str()
                && after.files[0].size == intended["file"]["size"].as_u64()
                && after.files[0]
                    .mime
                    .as_deref()
                    .is_none_or(|mime| Some(mime) == intended["file"]["mime"].as_str())
                && !before.files.iter().any(|f| f.id == after.files[0].id)
        }
        Destination::Telegram { .. } => after.files.iter().all(|f| {
            f.filename
                .as_deref()
                .is_none_or(|v| Some(v) == intended["file"]["filename"].as_str())
                && f.mime
                    .as_deref()
                    .is_none_or(|v| Some(v) == intended["file"]["mime"].as_str())
                && (after.kind != cmaeg_core::message::NativeMediaKind::File
                    || f.size.is_none()
                    || f.size == intended["file"]["size"].as_u64())
        }),
    }
}
fn result(delivery: DeliveryOutcome) -> IntentReceipt {
    let mut r = unknown();
    r.delivery = delivery;
    r
}
fn rejected(reason: &str) -> IntentReceipt {
    result(DeliveryOutcome::Rejected {
        reason: reason.into(),
    })
}
fn unknown() -> IntentReceipt {
    receipts::uncertain("native_media_update_unconfirmed")
}
