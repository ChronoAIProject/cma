use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::resource_access::ResourceOwner;

use super::super::{BridgeId, BridgeRecord, BridgeValidation, BridgeWorkPin};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallbackError {
    #[error("callback authentication failed")]
    Authentication,
    #[error("callback signing keys unavailable")]
    KeysUnavailable,
    #[error("callback binding is unavailable or changed")]
    BindingUnavailable,
    #[error("callback exceeds supported bounds")]
    Bounds,
    #[error("callback payload is invalid")]
    Payload,
    #[error("callback message conflicts with accepted content")]
    ReplayConflict,
    #[error("callback ingress capacity exceeded")]
    Capacity,
    #[error("callback acceptance is unavailable or uncertain")]
    Unavailable,
}

/// Cannot be deserialized or created from callback claims. The adapter resolves
/// the URL's bridge ID, then verifies live NyxID route/key/custody facts.
#[derive(Clone)]
pub struct CallbackBinding {
    pub(super) bridge: BridgeRecord,
    pub(super) proof: BridgeValidation,
}

impl CallbackBinding {
    pub fn new(bridge: BridgeRecord, proof: BridgeValidation) -> Result<Self, CallbackError> {
        bridge
            .admission_pin()
            .map_err(|_| CallbackError::BindingUnavailable)?;
        proof
            .require(
                &bridge.owner,
                &bridge
                    .current()
                    .map_err(|_| CallbackError::BindingUnavailable)?
                    .config,
            )
            .map_err(|_| CallbackError::BindingUnavailable)?;
        Ok(Self { bridge, proof })
    }

    pub fn require_current(&self, current: &BridgeRecord) -> Result<(), CallbackError> {
        if current != &self.bridge {
            return Err(CallbackError::BindingUnavailable);
        }
        current
            .admission_pin()
            .map_err(|_| CallbackError::BindingUnavailable)?;
        self.proof
            .require(
                &current.owner,
                &current
                    .current()
                    .map_err(|_| CallbackError::BindingUnavailable)?
                    .config,
            )
            .map_err(|_| CallbackError::BindingUnavailable)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxAdmission {
    Accepted,
    SenderDenied,
    PartitionDenied {
        reason: crate::bridge::partition::PartitionRefusal,
    },
}

/// Corroboration only. Neither variant grants ambient owner authority, renewal,
/// provisioning, execution, or permission at ordinary management endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelayCorroboration {
    AbsentInboxOnly,
    VerifiedInboxOnly {
        subject: String,
        key_id: String,
        expires_at: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallbackAttestation {
    pub issuer: String,
    pub signing_key_id: String,
    pub token_id: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub body_sha256: String,
}

/// Bounded normalized content. Deliberately has no Debug implementation and no
/// raw platform payload, access token, reply token, or untrusted resource IDs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxEnvelope {
    pub platform: String,
    pub route_id: String,
    pub chat_id: String,
    pub conversation_type: String,
    pub sender_id: String,
    pub sender_display_name: Option<String>,
    pub content_type: String,
    pub text: Option<String>,
    pub attachments: Vec<InboxAttachment>,
    pub reply_to_message_id: Option<String>,
    pub reply_to_platform_message_id: Option<String>,
    pub protected_thread_context_present: bool,
    pub timestamp: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxAttachment {
    pub content_type: String,
    pub url: String,
    pub platform_message_id: Option<String>,
    pub file_key: Option<String>,
    pub image_key: Option<String>,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredInbox {
    pub id: String,
    pub owner: ResourceOwner,
    pub pin: BridgeWorkPin,
    pub bot_id: String,
    pub key_id: String,
    pub message_id: String,
    pub correlation_id: String,
    pub envelope: InboxEnvelope,
    pub attestation: CallbackAttestation,
    pub relay: RelayCorroboration,
    pub admission: InboxAdmission,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<crate::bridge::partition::StoredPartitionObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_hold: Option<InboxExecutionHold>,
    pub accepted_at: i64,
    pub expires_at: i64,
    pub reply_expires_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxReceipt {
    pub inbox_id: String,
    pub bridge_id: BridgeId,
    pub message_id: String,
    pub accepted_at: i64,
    pub admission: InboxAdmission,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_hold: Option<InboxExecutionHold>,
}

/// A durable admission is not evidence that an Agent worker is available.
/// Only a separately authorized worker transition may clear this hold later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxExecutionHold {
    WorkerUnavailable,
}

impl StoredInbox {
    pub fn receipt(&self) -> InboxReceipt {
        InboxReceipt {
            inbox_id: self.id.clone(),
            bridge_id: self.pin.bridge_id.clone(),
            message_id: self.message_id.clone(),
            accepted_at: self.accepted_at,
            admission: self.admission.clone(),
            execution_hold: self.execution_hold,
        }
    }
}

/// Opaque authenticated, bounded write. Persist atomically with the current
/// bridge state fence, queue/rate limits and encrypted expiring reply context.
/// There is no Serialize/Deserialize or Debug implementation for this proof.
pub struct InboxWrite {
    pub(super) platform_redelivery: Option<super::VerifiedPlatformRedelivery>,
    pub(super) binding: CallbackBinding,
    pub(super) record: StoredInbox,
    pub(super) reply_token: Option<Zeroizing<String>>,
    pub(super) thread_context: Option<Zeroizing<String>>,
    pub(super) deadline: tokio::time::Instant,
}

/// Cryptographic transport evidence against a stored configuration snapshot.
/// This cannot be persisted or used as owner authority. Only a matching fresh
/// live binding can turn it into an InboxWrite, retaining its original deadline.
pub struct SignedCallback {
    pub(super) bridge: BridgeRecord,
    pub(super) record: StoredInbox,
    pub(super) reply_token: Option<Zeroizing<String>>,
    pub(super) thread_context: Option<Zeroizing<String>>,
    pub(super) deadline: tokio::time::Instant,
}

impl SignedCallback {
    pub fn bind(mut self, binding: CallbackBinding) -> Result<InboxWrite, CallbackError> {
        binding.require_current(&self.bridge)?;
        if self.record.admission != InboxAdmission::Accepted {
            self.reply_token = None;
            self.thread_context = None;
            self.record.reply_expires_at = None;
            self.record.execution_hold = None;
            self.record.envelope.protected_thread_context_present = false;
        }
        let write = InboxWrite {
            platform_redelivery: None,
            binding,
            record: self.record,
            reply_token: self.reply_token,
            thread_context: self.thread_context,
            deadline: self.deadline,
        };
        write.require_current(write.bridge())?;
        Ok(write)
    }
}

impl InboxWrite {
    pub fn with_platform_redelivery(
        mut self,
        proof: Option<super::VerifiedPlatformRedelivery>,
    ) -> Result<Self, CallbackError> {
        if let Some(proof) = &proof {
            proof.require_source(&self.record)?;
        }
        self.platform_redelivery = proof;
        Ok(self)
    }
    pub fn platform_redelivery(&self) -> Option<&super::VerifiedPlatformRedelivery> {
        self.platform_redelivery.as_ref()
    }

    pub fn record(&self) -> &StoredInbox {
        &self.record
    }
    pub fn bridge(&self) -> &BridgeRecord {
        &self.binding.bridge
    }
    pub fn custody_pin(&self) -> &crate::bridge::BridgeCustodyPin {
        self.binding.proof.custody_pin()
    }
    pub fn reply_token(&self) -> Option<&str> {
        self.reply_token.as_ref().map(|s| s.as_str())
    }
    pub fn thread_context(&self) -> Option<&str> {
        self.thread_context.as_ref().map(|s| s.as_str())
    }
    pub fn require_current(&self, current: &BridgeRecord) -> Result<(), CallbackError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(CallbackError::Authentication);
        }
        self.binding.require_current(current)
    }
    pub fn match_replay(&self, stored: &StoredInbox) -> Result<InboxReceipt, CallbackError> {
        self.require_current(self.bridge())?;
        if stored.expires_at <= self.record.accepted_at {
            return Err(CallbackError::Unavailable);
        }
        if stored.id != self.record.id
            || stored.attestation.body_sha256 != self.record.attestation.body_sha256
        {
            return Err(CallbackError::ReplayConflict);
        }
        Ok(stored.receipt())
    }
}

#[async_trait::async_trait]
pub trait CallbackInboxStore: Send + Sync {
    /// Read-only recovery after optional enrichment fails. Implementations must
    /// revalidate current binding/custody and exact original callback identity;
    /// absence never authorizes fresh acceptance or a permanent refusal.
    async fn recover_receipt(
        &self,
        _write: &InboxWrite,
    ) -> Result<Option<InboxReceipt>, CallbackError> {
        Ok(None)
    }

    /// Success means durable majority acceptance. A DB timeout or unknown commit
    /// without a majority-visible matching receipt must return Unavailable.
    async fn accept(&self, write: InboxWrite) -> Result<InboxReceipt, CallbackError>;
}
