//! Explicit edits retain the same source and a confirmed original destination.
use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelReplyEditRequest {
    pub original_outbox_id: String,
    pub request_key: String,
    pub text: String,
}

impl ChannelReplySessionAuthorization {
    pub fn edit_request_identity(
        &self,
        request: &ChannelReplyEditRequest,
    ) -> Result<(String, String), OutboxError> {
        self.require_current()?;
        edit_identity(self.namespace(), request)
    }
}

fn edit_identity(
    namespace: &str,
    request: &ChannelReplyEditRequest,
) -> Result<(String, String), OutboxError> {
    if !request
        .original_outbox_id
        .strip_prefix("bro_")
        .is_some_and(super::digest)
        || request.text.trim().is_empty()
        || request.text.len() > 1_048_576
    {
        return Err(OutboxError::Bounds);
    }
    Ok((
        super::session::invocation_id(namespace, &request.request_key)?,
        super::hash(&(
            "cma.bridge.reply-edit.v1",
            &request.original_outbox_id,
            &request.text,
        ))?,
    ))
}

impl ChannelReplyToolIntent {
    pub fn prepare_edit(
        scope: ChannelReplyTurnAuthorization,
        request: ChannelReplyEditRequest,
    ) -> Result<Self, OutboxError> {
        scope.require_current()?;
        if !editable_platform(&scope.source().config().connector.platform) {
            return Err(OutboxError::CapabilityUnavailable);
        }
        let (invocation_id, request_sha256) = edit_identity(scope.namespace(), &request)?;
        Ok(Self {
            scope,
            invocation_id,
            request_sha256,
            text: request.text,
            original_outbox_id: Some(request.original_outbox_id),
        })
    }
}

pub fn editable_platform(platform: &crate::bridge::BridgePlatform) -> bool {
    matches!(
        platform,
        crate::bridge::BridgePlatform::Lark | crate::bridge::BridgePlatform::Feishu
    )
}

impl OutboxItem {
    pub fn from_edit_intent(
        intent: &ChannelReplyToolIntent,
        original: &Self,
        credential: DeliveryCredentialRef,
        sequence_start: u64,
        predecessor: Option<String>,
        now_ms: i64,
    ) -> Result<Vec<Self>, OutboxError> {
        let receipt = original_receipt(original, intent.scope().source())?;
        if intent.original_outbox_id() != Some(original.id.as_str())
            || original.sequence >= sequence_start
        {
            return Err(OutboxError::SourceMismatch);
        }
        let operation = RelayOperation::Edit {
            original_outbox_id: original.id.clone(),
            platform_message_id: receipt
                .platform_message_id
                .clone()
                .ok_or(OutboxError::OutputUnavailable)?,
        };
        Self::build_tool_output(
            intent,
            credential,
            sequence_start,
            predecessor,
            now_ms,
            operation,
        )
    }
    pub fn require_edit_original<'a>(
        &self,
        original: &'a Self,
    ) -> Result<&'a DeliveryReceipt, OutboxError> {
        self.validate()?;
        let RelayOperation::Edit {
            original_outbox_id,
            platform_message_id,
        } = &self.operation
        else {
            return Err(OutboxError::SourceMismatch);
        };
        let receipt = original_receipt(original, &self.source)?;
        if &original.id != original_outbox_id
            || original.sequence >= self.sequence
            || receipt.platform_message_id.as_ref() != Some(platform_message_id)
        {
            return Err(OutboxError::SourceMismatch);
        }
        Ok(receipt)
    }
}

fn original_receipt<'a>(
    original: &'a OutboxItem,
    source: &OutboxSource,
) -> Result<&'a DeliveryReceipt, OutboxError> {
    original.validate()?;
    if !editable_platform(&source.config().connector.platform) {
        return Err(OutboxError::CapabilityUnavailable);
    }
    if &original.source != source || original.operation != RelayOperation::InitialReply {
        return Err(OutboxError::SourceMismatch);
    }
    match &original.state {
        DeliveryState::Sent { receipt } if receipt.platform_message_id.is_some() => Ok(receipt),
        DeliveryState::Unknown { .. } => Err(OutboxError::UnknownOutcome),
        _ => Err(OutboxError::OutputUnavailable),
    }
}
