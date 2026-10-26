//! Neutral messages emitted by an agent turn.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    activity::AttachmentKind, channel::validate_https_url, ids::MessageRef, require_string,
    ValidationError,
};

#[path = "media_message.rs"]
mod media;
pub use media::{
    validate as validate_native_attachments, NativeAttachment, NativeMediaKind, NativeMediaSource,
    MAX_NATIVE_ATTACHMENTS, MAX_NATIVE_INLINE_BYTES,
};

pub const MAX_MESSAGE_TEXT_BYTES: usize = 32 * 1_024;
pub const MAX_BLOCKS: usize = 16;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutboundMessage {
    /// Explicit native media delivery. Legacy attachment blocks remain links.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_attachments: Vec<NativeAttachment>,
    /// Request the verified source thread; never fall back when unavailable.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub thread_reply: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<MessageRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum Block {
    Text {
        text: String,
    },
    Card {
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default)]
        fields: Vec<Field>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<String>,
    },
    Buttons {
        id: String,
        options: Vec<ChoiceOption>,
    },
    Choice {
        id: String,
        prompt: String,
        options: Vec<ChoiceOption>,
        multi: bool,
    },
    Form {
        id: String,
        fields: Vec<FormField>,
    },
    Attachment {
        reference: String,
        kind: AttachmentKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
    },
    Link {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OptionStyle {
    #[default]
    Default,
    Primary,
    Danger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChoiceOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub style: OptionStyle,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum FormFieldKind {
    Text {},
    Number {},
    Choice { options: Vec<ChoiceOption> },
    Date {},
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FormField {
    pub id: String,
    pub label: String,
    pub kind: FormFieldKind,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderingReport {
    pub outcome: RenderingOutcome,
    #[serde(default)]
    pub blocks: Vec<BlockRendering>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RenderingOutcome {
    Rendered,
    Degraded,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum BlockRenderingOutcome {
    Exact,
    Degraded { how: String },
    Dropped { why: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BlockRendering {
    pub index: u16,
    pub outcome: BlockRenderingOutcome,
}

impl OutboundMessage {
    /// Validate a provider response before handing it to a delivery adapter.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.text.is_none() && self.blocks.is_empty() && self.native_attachments.is_empty() {
            return Err(ValidationError::new(
                "/",
                "text or at least one block is required",
            ));
        }
        media::validate(&self.native_attachments)?;
        if let Some(text) = &self.text {
            require_string(text, "/text", 0, MAX_MESSAGE_TEXT_BYTES)?;
        }
        if self.blocks.len() > MAX_BLOCKS {
            return Err(ValidationError::new("/blocks", "too many blocks"));
        }
        let mut ids = BTreeSet::new();
        for (index, block) in self.blocks.iter().enumerate() {
            let pointer = format!("/blocks/{index}");
            match block {
                Block::Text { text } => {
                    require_string(text, &format!("{pointer}/text"), 0, MAX_MESSAGE_TEXT_BYTES)?
                }
                Block::Card {
                    title,
                    body,
                    fields,
                    image,
                } => {
                    require_string(title, &format!("{pointer}/title"), 1, 200)?;
                    if let Some(body) = body {
                        require_string(
                            body,
                            &format!("{pointer}/body"),
                            0,
                            MAX_MESSAGE_TEXT_BYTES,
                        )?;
                    }
                    if fields.len() > 16 {
                        return Err(ValidationError::new(&pointer, "card has too many fields"));
                    }
                    if let Some(image) = image {
                        validate_https_url(image, &format!("{pointer}/image"))?;
                    }
                }
                Block::Buttons { id, options } => {
                    validate_options(id, options, &pointer, 8, &mut ids)?;
                }
                Block::Choice {
                    id,
                    prompt,
                    options,
                    ..
                } => {
                    require_string(
                        prompt,
                        &format!("{pointer}/prompt"),
                        1,
                        MAX_MESSAGE_TEXT_BYTES,
                    )?;
                    validate_options(id, options, &pointer, 20, &mut ids)?;
                }
                Block::Form { id, fields } => {
                    require_id(id, &format!("{pointer}/id"), &mut ids)?;
                    if fields.is_empty() || fields.len() > 12 {
                        return Err(ValidationError::new(
                            format!("{pointer}/fields"),
                            "must contain one to twelve fields",
                        ));
                    }
                    for (field_index, field) in fields.iter().enumerate() {
                        require_string(
                            &field.id,
                            &format!("{pointer}/fields/{field_index}/id"),
                            1,
                            512,
                        )?;
                        require_string(
                            &field.label,
                            &format!("{pointer}/fields/{field_index}/label"),
                            1,
                            200,
                        )?;
                    }
                }
                Block::Attachment {
                    reference, caption, ..
                } => {
                    require_string(reference, &format!("{pointer}/reference"), 1, 4_096)?;
                    if let Some(caption) = caption {
                        require_string(
                            caption,
                            &format!("{pointer}/caption"),
                            0,
                            MAX_MESSAGE_TEXT_BYTES,
                        )?;
                    }
                }
                Block::Link { url, label } => {
                    validate_https_url(url, &format!("{pointer}/url"))?;
                    if let Some(label) = label {
                        require_string(label, &format!("{pointer}/label"), 1, 200)?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn validate_options(
    id: &str,
    options: &[ChoiceOption],
    pointer: &str,
    max: usize,
    ids: &mut BTreeSet<String>,
) -> Result<(), ValidationError> {
    require_id(id, &format!("{pointer}/id"), ids)?;
    if options.is_empty() || options.len() > max {
        return Err(ValidationError::new(
            format!("{pointer}/options"),
            "option count is outside its bound",
        ));
    }
    let mut option_ids = BTreeSet::new();
    for (index, option) in options.iter().enumerate() {
        require_string(&option.id, &format!("{pointer}/options/{index}/id"), 1, 512)?;
        require_string(
            &option.label,
            &format!("{pointer}/options/{index}/label"),
            1,
            64,
        )?;
        if !option_ids.insert(&option.id) {
            return Err(ValidationError::new(
                format!("{pointer}/options"),
                "option ids must be unique",
            ));
        }
    }
    Ok(())
}

fn require_id(id: &str, pointer: &str, ids: &mut BTreeSet<String>) -> Result<(), ValidationError> {
    require_string(id, pointer, 1, 512)?;
    if !ids.insert(id.to_owned()) {
        return Err(ValidationError::new(pointer, "block ids must be unique"));
    }
    Ok(())
}
