//! Typed approved-template messages; recipients remain bound to the event reference.
use serde::{Deserialize, Serialize};

/// A template already approved in the destination WhatsApp account.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct WhatsappTemplate {
    /// Lowercase template name with digits and underscores, at most 512 bytes.
    pub name: String,
    /// Approved template language variant.
    pub language: WhatsappTemplateLanguage,
    /// Up to five unique components and twenty positional text parameters total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<WhatsappTemplateComponent>>,
}

/// Template language selection.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct WhatsappTemplateLanguage {
    /// Language code, optionally including uppercase region, such as en_US.
    pub code: String,
}

/// Bounded positional template components; native media and callbacks are unsupported.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WhatsappTemplateComponent {
    /// A single text header parameter.
    Header {
        /// Exactly one text parameter.
        parameters: Vec<WhatsappTemplateParameter>,
    },
    /// Positional text body parameters.
    Body {
        /// One to twenty text parameters.
        parameters: Vec<WhatsappTemplateParameter>,
    },
    /// URL suffix substitution for an approved template button.
    Button {
        /// Only URL buttons are supported.
        sub_type: WhatsappTemplateButtonType,
        /// Button index as "0", "1" or "2".
        index: String,
        /// Exactly one text parameter.
        parameters: Vec<WhatsappTemplateParameter>,
    },
}

/// Supported template button kind.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhatsappTemplateButtonType {
    /// A URL button, without an interactive callback.
    Url,
}

/// Supported template parameter kind.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WhatsappTemplateParameter {
    /// Positional text substitution.
    Text {
        /// One to 1024 bytes, without control characters.
        text: String,
    },
}
