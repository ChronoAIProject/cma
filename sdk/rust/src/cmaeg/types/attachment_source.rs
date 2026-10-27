//! Non-secret source-bound attachment locators.
use super::LarkResourceKind;
use serde::{Deserialize, Serialize};

/// CMAEG `AttachmentSource` wire shape: a non-secret locator bound into the
/// activity digest. Fetching still requires the event reference's authority.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AttachmentSource {
    #[serde(rename = "native_lark")]
    NativeLark {
        message_id: String,
        resource_key: String,
        resource_kind: LarkResourceKind,
        source_revision: String,
    },
    /// Signed native WhatsApp media, fetched through current Meta custody.
    #[serde(rename = "whatsapp")]
    Whatsapp {
        /// Exact incoming native message ID.
        message_id: String,
        /// Exact media resource ID.
        media_id: String,
        /// Lowercase hexadecimal SHA256 from the signed webhook.
        sha256: String,
    },
    /// Sealed native resource retrieved through an exact scoped NyxID connection.
    #[serde(rename = "lark_resource")]
    LarkResource {
        /// Native Lark/Feishu message ID (`om_…`).
        message_id: String,
        /// Native image or file resource key.
        resource_key: String,
        /// Regional API resource type.
        resource_kind: LarkResourceKind,
        /// Authorized connection and expected application identity.
        binding: super::LarkResourceBinding,
        /// Independently pinned receiving bot open ID (`ou_…`).
        bot_id: String,
        /// Digest of the exact source and channel credential revision.
        source_revision: String,
    },
    /// Native Discord attachment, resolved with current bot custody.
    #[serde(rename = "discord")]
    Discord {
        /// Exact receiving channel.
        channel_id: String,
        /// Exact native message.
        message_id: String,
        /// Exact attachment resource.
        attachment_id: String,
    },
    /// Wire variant `nyxid_relay`.
    #[serde(rename = "nyxid_relay")]
    NyxidRelay {
        /// Wire field `index`.
        index: u64,
    },
    /// Wire variant `telegram`.
    #[serde(rename = "telegram")]
    Telegram {
        /// Wire field `file_id`.
        file_id: String,
        /// Wire field `file_unique_id`.
        file_unique_id: String,
    },
    /// Wire variant `url`.
    #[serde(rename = "url")]
    Url {
        /// Wire field `url`.
        url: String,
    },
    /// Wire variant `inline`.
    #[serde(rename = "inline")]
    Inline {
        /// Wire field `pointer`.
        pointer: String,
    },
}
