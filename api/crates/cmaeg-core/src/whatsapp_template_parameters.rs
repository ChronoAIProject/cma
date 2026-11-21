//! Typed Meta Cloud API template substitutions; never arbitrary metadata.
use crate::{require_string, ValidationError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WhatsappTemplateParameter {
    Text { text: String },
    Currency { currency: WhatsappTemplateCurrency },
    DateTime { date_time: WhatsappTemplateDateTime },
    Image { image: WhatsappTemplateMedia },
    Video { video: WhatsappTemplateMedia },
    Document { document: WhatsappTemplateDocument },
}

/// A Meta media ID or HTTPS link, exactly one. Meta verifies media ownership/type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WhatsappTemplateMedia {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(min_length = 1, max_length = 32, pattern = "^[0-9]+$")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(min_length = 1, max_length = 8192)]
    pub link: Option<String>,
}

/// Header document reference, with an optional displayed filename.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WhatsappTemplateDocument {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(min_length = 1, max_length = 32, pattern = "^[0-9]+$")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(min_length = 1, max_length = 8192)]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(min_length = 1, max_length = 255)]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WhatsappTemplateCurrency {
    #[schema(min_length = 1, max_length = 1024)]
    pub fallback_value: String,
    /// Three uppercase ISO 4217 letters; Meta validates the currency itself.
    #[schema(min_length = 3, max_length = 3, pattern = "^[A-Z]{3}$")]
    pub code: String,
    /// Nonnegative amount multiplied by 1000, bounded to exact JSON safe integers.
    #[schema(minimum = 0, maximum = 9007199254740991_u64)]
    pub amount_1000: u64,
}

/// Explicit date/time label. This CMA contract accepts the fallback string only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WhatsappTemplateDateTime {
    #[schema(min_length = 1, max_length = 1024)]
    pub fallback_value: String,
}

fn text(value: &str, path: &str, max: usize) -> Result<(), ValidationError> {
    require_string(value, path, 1, max)?;
    if value.chars().any(char::is_control) {
        return Err(ValidationError::new(
            path,
            "template parameters cannot contain control characters",
        ));
    }
    Ok(())
}
fn media(id: Option<&str>, link: Option<&str>, path: &str) -> Result<(), ValidationError> {
    match (id, link) {
        (Some(id), None)
            if !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Ok(())
        }
        (None, Some(link)) => {
            text(link, &format!("{path}/link"), 8192)?;
            let parsed = url::Url::parse(link).ok();
            if !parsed.is_some_and(|url| {
                url.scheme() == "https"
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.fragment().is_none()
            }) || link.chars().any(char::is_whitespace)
            {
                return Err(ValidationError::new(
                    path,
                    "media link must be public HTTPS without userinfo or fragment",
                ));
            }
            Ok(())
        }
        _ => Err(ValidationError::new(
            path,
            "provide exactly one numeric Meta media ID or public HTTPS link",
        )),
    }
}
impl WhatsappTemplateParameter {
    pub(super) fn validate(&self, path: &str) -> Result<(), ValidationError> {
        match self {
            Self::Text { text: value } => text(
                value,
                &format!("{path}/text"),
                super::MAX_TEMPLATE_PARAMETER_BYTES,
            ),
            Self::Currency { currency } => {
                text(
                    &currency.fallback_value,
                    &format!("{path}/currency/fallback_value"),
                    1024,
                )?;
                if currency.code.len() != 3
                    || !currency.code.bytes().all(|b| b.is_ascii_uppercase())
                {
                    return Err(ValidationError::new(
                        format!("{path}/currency/code"),
                        "use three uppercase ISO 4217 letters",
                    ));
                }
                if currency.amount_1000 > 9_007_199_254_740_991 {
                    return Err(ValidationError::new(
                        format!("{path}/currency/amount_1000"),
                        "amount exceeds exact JSON integer range",
                    ));
                }
                Ok(())
            }
            Self::DateTime { date_time } => text(
                &date_time.fallback_value,
                &format!("{path}/date_time/fallback_value"),
                1024,
            ),
            Self::Image { image } => media(
                image.id.as_deref(),
                image.link.as_deref(),
                &format!("{path}/image"),
            ),
            Self::Video { video } => media(
                video.id.as_deref(),
                video.link.as_deref(),
                &format!("{path}/video"),
            ),
            Self::Document { document } => {
                media(
                    document.id.as_deref(),
                    document.link.as_deref(),
                    &format!("{path}/document"),
                )?;
                if let Some(filename) = &document.filename {
                    let path = format!("{path}/document/filename");
                    text(filename, &path, 255)?;
                    if filename.contains(['/', '\\']) || matches!(filename.as_str(), "." | "..") {
                        return Err(ValidationError::new(
                            path,
                            "filename must be one path component",
                        ));
                    }
                }
                Ok(())
            }
        }
    }
}
