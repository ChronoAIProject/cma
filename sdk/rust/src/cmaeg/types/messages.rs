//! Messages wire types for the independent CMAEG v1 protocol.
use super::{AttachmentKind, MessageRef, OptionStyle, RenderingOutcome, TypingOutcome};
use serde::{Deserialize, Serialize};

/// CMAEG `Block` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Block {
    /// Wire variant `text`.
    #[serde(rename = "text")]
    Text {
        /// Wire field `text`.
        text: String,
    },
    /// Wire variant `card`.
    #[serde(rename = "card")]
    Card {
        /// Wire field `body`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        /// Wire field `fields`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<Field>>,
        /// Wire field `image`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<String>,
        /// Wire field `title`.
        title: String,
    },
    /// Wire variant `buttons`.
    #[serde(rename = "buttons")]
    Buttons {
        /// Wire field `id`.
        id: String,
        /// Wire field `options`.
        options: Vec<ChoiceOption>,
    },
    /// Wire variant `choice`.
    #[serde(rename = "choice")]
    Choice {
        /// Wire field `id`.
        id: String,
        /// Wire field `multi`.
        multi: bool,
        /// Wire field `options`.
        options: Vec<ChoiceOption>,
        /// Wire field `prompt`.
        prompt: String,
    },
    /// Wire variant `form`.
    #[serde(rename = "form")]
    Form {
        /// Wire field `fields`.
        fields: Vec<FormField>,
        /// Wire field `id`.
        id: String,
    },
    /// Wire variant `attachment`.
    #[serde(rename = "attachment")]
    Attachment {
        /// Wire field `caption`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        /// Wire field `kind`.
        kind: AttachmentKind,
        /// Wire field `reference`.
        reference: String,
    },
    /// Wire variant `link`.
    #[serde(rename = "link")]
    Link {
        /// Wire field `label`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// Wire field `url`.
        url: String,
    },
}

/// CMAEG `BlockRendering` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockRendering {
    /// Wire field `index`.
    pub index: u64,
    /// Wire field `outcome`.
    pub outcome: BlockRenderingOutcome,
}

/// CMAEG `BlockRenderingOutcome` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BlockRenderingOutcome {
    /// Wire variant `exact`.
    #[serde(rename = "exact")]
    Exact {},
    /// Wire variant `degraded`.
    #[serde(rename = "degraded")]
    Degraded {
        /// Wire field `how`.
        how: String,
    },
    /// Wire variant `dropped`.
    #[serde(rename = "dropped")]
    Dropped {
        /// Wire field `why`.
        why: String,
    },
}

/// CMAEG `ChoiceOption` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceOption {
    /// Wire field `id`.
    pub id: String,
    /// Wire field `label`.
    pub label: String,
    /// Wire field `selected`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    /// Wire field `style`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<OptionStyle>,
}

/// CMAEG `DeliveryOutcome` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome")]
pub enum DeliveryOutcome {
    /// Wire variant `confirmed`.
    #[serde(rename = "confirmed")]
    Confirmed {
        /// Wire field `platform_message_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        platform_message_id: Option<String>,
    },
    /// Wire variant `rejected`.
    #[serde(rename = "rejected")]
    Rejected {
        /// Wire field `reason`.
        reason: String,
    },
    /// Wire variant `rate_limited`.
    #[serde(rename = "rate_limited")]
    RateLimited {
        /// Wire field `retry_after`.
        retry_after: u64,
    },
    /// Wire variant `unknown`.
    #[serde(rename = "unknown")]
    Unknown {
        /// Wire field `reason`.
        reason: String,
    },
    /// Wire variant `unsupported`.
    #[serde(rename = "unsupported")]
    Unsupported {},
}

/// CMAEG `Field` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    /// Wire field `label`.
    pub label: String,
    /// Wire field `value`.
    pub value: String,
}

/// CMAEG `FormField` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct FormField {
    /// Wire field `id`.
    pub id: String,
    /// Wire field `kind`.
    pub kind: FormFieldKind,
    /// Wire field `label`.
    pub label: String,
    /// Wire field `required`.
    pub required: bool,
}

/// CMAEG `FormFieldKind` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FormFieldKind {
    /// Wire variant `text`.
    #[serde(rename = "text")]
    Text {},
    /// Wire variant `number`.
    #[serde(rename = "number")]
    Number {},
    /// Wire variant `choice`.
    #[serde(rename = "choice")]
    Choice {
        /// Wire field `options`.
        options: Vec<ChoiceOption>,
    },
    /// Wire variant `date`.
    #[serde(rename = "date")]
    Date {},
}

/// CMAEG `IntentReceipt` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentReceipt {
    /// Wire field `delivery`.
    pub delivery: DeliveryOutcome,
    /// Wire field `message_ref`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<MessageRef>,
    /// Wire field `rendering`.
    pub rendering: RenderingReport,
}

/// CMAEG `OutboundMessage` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboundMessage {
    /// Wire field `blocks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<Block>>,
    /// Wire field `reply_to`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<MessageRef>,
    /// Wire field `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// CMAEG `RenderingReport` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderingReport {
    /// Wire field `blocks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<BlockRendering>>,
    /// Wire field `outcome`.
    pub outcome: RenderingOutcome,
}

/// CMAEG `TypingReceipt` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct TypingReceipt {
    /// Wire field `outcome`.
    pub outcome: TypingOutcome,
}
