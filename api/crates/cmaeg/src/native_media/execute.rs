//! Durable claims and shared message fences prevent uncertain native edits being resent.
use super::{
    authority, denied,
    model::{Current, Original},
    provider::{self, PreparationError, WriteError},
    store::Store,
    verify::{self, Confirmation},
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
        let execution = outcome.unwrap_or_else(|_| Execution::held(unknown()));
        let receipt = execution.receipt;
        if claims.finish(claim, &receipt).await.is_err() {
            return Ok(unknown());
        }
        if execution.release {
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
        if a.revision != current.revision || !current.ready() {
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
    ) -> Execution {
        let Ok((channel, message, original, current)) =
            self.media_current(caller, reference, store).await
        else {
            return Execution::settled(rejected("native_media_authority_or_revision_changed"));
        };
        let a = message
            .native_media_edit
            .as_ref()
            .expect("checked authority");
        let Some(token) = authority::token(&channel, &a.destination) else {
            return Execution::settled(rejected("native_media_credential_unavailable"));
        };
        let token_hash = crate::channels::definition::hash_secret(token.as_str());
        let file = if let Some(replacement) = &patch.replacement {
            match outbound::prepare(&[replacement.attachment()]).await {
                Ok(mut f) if f.len() == 1 => f.pop(),
                _ => return Execution::settled(rejected("attachment_preparation_failed")),
            }
        } else {
            None
        };
        let intended = serde_json::json!({
            "before": current.snapshot,
            "caption": patch.caption.as_ref().unwrap_or(&current.snapshot.caption),
            "content": patch.text.as_ref().unwrap_or(&current.snapshot.content),
            "replace_caption": patch.caption.is_some(), "replace_content": patch.text.is_some(),
            "file": file.as_ref().map(|f| serde_json::json!({"sha256":format!("{:x}",Sha256::digest(&f.bytes)),"size":f.bytes.len(),"filename":f.filename,"mime":f.mime}))
        });
        if store.freeze(&original, operation, &intended).await.is_err() {
            return Execution::settled(rejected("native_media_attempt_unavailable"));
        }
        let prepared = match provider::prepare(
            client, token, &message, &original, &current, patch, file,
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(PreparationError::Rejected(reason)) => return Execution::settled(rejected(reason)),
            Err(PreparationError::Changed) => {
                return if store
                    .retire(
                        &original,
                        &current,
                        operation,
                        "native_media_message_changed",
                    )
                    .await
                    .is_ok()
                {
                    Execution::settled(rejected("native_media_message_changed"))
                } else {
                    Execution::held(unknown())
                };
            }
        };
        // This CAS closes the old revision before a write can become confirmed.
        let pending = match store.begin(&original, &current, operation).await {
            Ok(pending) => pending,
            Err(_) => return Execution::held(unknown()),
        };
        if !self
            .media_final_check(caller, reference, store, &token_hash, &pending, operation)
            .await
        {
            return Execution {
                receipt: rejected("native_media_authority_changed"),
                release: store.restore(&original, &pending, operation).await.is_ok(),
            };
        }
        let observed = match prepared.send(client, token, &message).await {
            Ok(observed) => observed,
            Err(WriteError::Unknown) => return Execution::held(unknown()),
            Err(error) => {
                let receipt = match error {
                    WriteError::Rejected => rejected("native_media_update_rejected"),
                    WriteError::RateLimited(retry_after) => {
                        result(DeliveryOutcome::RateLimited { retry_after })
                    }
                    WriteError::Unknown => unreachable!(),
                };
                return Execution {
                    receipt,
                    release: store.restore(&original, &pending, operation).await.is_ok(),
                };
            }
        };
        let Some(confirmation) = verify::confirm(
            &a.destination,
            patch,
            &current.snapshot,
            observed,
            &intended,
        ) else {
            return Execution::held(unknown());
        };
        let mut receipt = result(DeliveryOutcome::Confirmed {
            platform_message_id: Some(message.platform_message_id.clone()),
        });
        receipt.rendering.outcome = RenderingOutcome::Rendered;
        match confirmation {
            Confirmation::Renewable(snapshot) => {
                match store
                    .advance(&original, &pending, *snapshot, operation)
                    .await
                {
                    Ok(next) => {
                        receipt.message_ref =
                            authority::reference(&original, &next, &self.keys, Some(reference));
                        if receipt.message_ref.is_some() {
                            return Execution::settled(receipt);
                        }
                        // Never leave a ready revision behind without its exact reference.
                        let retired = store
                            .retire(
                                &original,
                                &next,
                                operation,
                                "native_media_reference_unavailable",
                            )
                            .await
                            .is_ok();
                        Execution {
                            receipt,
                            release: retired,
                        }
                    }
                    Err(_) => Execution::held(receipt),
                }
            }
            Confirmation::Retired(reason) => Execution {
                receipt,
                release: store
                    .retire(&original, &pending, operation, reason)
                    .await
                    .is_ok(),
            },
        }
    }
    async fn media_final_check(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        store: &Store,
        token_hash: &str,
        pending: &Current,
        operation: &str,
    ) -> bool {
        let Ok((channel, message, _)) = self
            .authorize_message(caller, reference, "nativeMediaUpdate")
            .await
        else {
            return false;
        };
        let Some(a) = message.native_media_edit.as_ref() else {
            return false;
        };
        let Ok((original, current)) = store.load(&a.original_sha256).await else {
            return false;
        };
        authority::check(&channel, &message, &original, caller).is_ok()
            && current.version == pending.version
            && current.revision == a.revision
            && current.owns_attempt(operation)
            && authority::token(&channel, &a.destination)
                .is_some_and(|t| crate::channels::definition::hash_secret(t.as_str()) == token_hash)
    }
}
struct Execution {
    receipt: IntentReceipt,
    release: bool,
}
impl Execution {
    fn settled(receipt: IntentReceipt) -> Self {
        Self {
            receipt,
            release: true,
        }
    }
    fn held(receipt: IntentReceipt) -> Self {
        Self {
            receipt,
            release: false,
        }
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
