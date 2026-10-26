//! Typed approved-template messages; recipients remain bound to the admitted destination.
use serde::{Deserialize, Serialize};

/// A template already approved in the destination WhatsApp account.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct WhatsappTemplate {
    /// Lowercase template name with digits and underscores, at most 512 bytes.
    pub name: String,
    /// Approved template language variant.
    pub language: WhatsappTemplateLanguage,
    /// Up to five unique components and twenty positional parameters total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<WhatsappTemplateComponent>>,
}

/// Template language selection.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct WhatsappTemplateLanguage {
    /// Language code, optionally including uppercase region, such as en_US.
    pub code: String,
}

/// Bounded positional template components; template callbacks are unsupported.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WhatsappTemplateComponent {
    /// A single text, image, video or document header parameter.
    Header {
        /// Exactly one parameter of the permitted kind.
        parameters: Vec<WhatsappTemplateParameter>,
    },
    /// Positional text, currency or date/time body parameters.
    Body {
        /// One to twenty permitted body parameters.
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
    /// A body currency substitution.
    Currency {
        /// Bounded amount, currency code and fallback label.
        currency: WhatsappTemplateCurrency,
    },
    /// A body date/time substitution using an explicit fallback label.
    DateTime {
        /// The date/time label accepted by this CMA contract.
        date_time: WhatsappTemplateDateTime,
    },
    /// Header image reference.
    Image {
        /// Exactly one uploaded Meta media ID or public HTTPS link.
        image: WhatsappTemplateMedia,
    },
    /// Header video reference.
    Video {
        /// Exactly one uploaded Meta media ID or public HTTPS link.
        video: WhatsappTemplateMedia,
    },
    /// Header document reference.
    Document {
        /// Media reference and optional display filename.
        document: WhatsappTemplateDocument,
    },
}

/// A header media reference. Supply exactly one non-null ID or link.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct WhatsappTemplateMedia {
    /// Numeric Meta media ID, at most 32 bytes; Meta checks ownership/type.
    #[serde(default)]
    pub id: Option<String>,
    /// Public HTTPS link, at most 8192 bytes, without credentials or fragment.
    #[serde(default)]
    pub link: Option<String>,
}
/// A document header reference and optional display filename.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct WhatsappTemplateDocument {
    /// Numeric Meta media ID, at most 32 bytes; choose exactly one ID or link.
    #[serde(default)]
    pub id: Option<String>,
    /// Public HTTPS link, at most 8192 bytes, without credentials or fragment.
    #[serde(default)]
    pub link: Option<String>,
    /// One path component, at most 255 bytes, without control characters.
    #[serde(default)]
    pub filename: Option<String>,
}
/// Nonnegative currency amount with an explicit formatted fallback.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct WhatsappTemplateCurrency {
    /// One to 1024 bytes, without control characters.
    pub fallback_value: String,
    /// Three uppercase ISO 4217 letters; Meta checks the currency itself.
    pub code: String,
    /// Amount multiplied by 1000, at most 9,007,199,254,740,991.
    pub amount_1000: u64,
}
/// Date/time label; this CMA contract accepts the fallback string only.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct WhatsappTemplateDateTime {
    /// One to 1024 bytes, without control characters.
    pub fallback_value: String,
}
