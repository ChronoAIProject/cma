//! Bounded approved-template payloads; destination authority is admitted separately.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ValidationError;

#[path = "whatsapp_template_parameters.rs"]
mod parameters;
pub use parameters::*;

pub const MAX_TEMPLATE_COMPONENTS: usize = 5;
pub const MAX_TEMPLATE_PARAMETERS: usize = 20;
pub const MAX_TEMPLATE_PARAMETER_BYTES: usize = 1024;

/// The template must already be approved in the destination WhatsApp account.
/// Approval and placeholder compatibility are checked by Meta on delivery.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WhatsappTemplate {
    pub name: String,
    pub language: WhatsappTemplateLanguage,
    #[serde(default)]
    pub components: Vec<WhatsappTemplateComponent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WhatsappTemplateLanguage {
    /// Two- or three-letter language, optionally followed by an uppercase region.
    pub code: String,
}

/// Bounded positional parameters for approved Cloud API templates.
/// URL buttons substitute a suffix in an already-approved URL, not a new route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WhatsappTemplateComponent {
    Header {
        parameters: Vec<WhatsappTemplateParameter>,
    },
    Body {
        parameters: Vec<WhatsappTemplateParameter>,
    },
    Button {
        sub_type: WhatsappTemplateButtonType,
        index: String,
        parameters: Vec<WhatsappTemplateParameter>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WhatsappTemplateButtonType {
    Url,
}

impl WhatsappTemplate {
    /// Media headers require the platform's media delivery budget and receipt.
    pub fn has_media(&self) -> bool {
        self.components.iter().any(|component| {
            let WhatsappTemplateComponent::Header { parameters } = component else {
                return false;
            };
            parameters.iter().any(|parameter| {
                matches!(
                    parameter,
                    WhatsappTemplateParameter::Image { .. }
                        | WhatsappTemplateParameter::Video { .. }
                        | WhatsappTemplateParameter::Document { .. }
                )
            })
        })
    }

    /// Links referenced by template headers; the transport applies its public URL policy.
    pub fn media_links(&self) -> impl Iterator<Item = &str> {
        self.components
            .iter()
            .flat_map(|component| match component {
                WhatsappTemplateComponent::Header { parameters } => parameters.as_slice(),
                _ => &[],
            })
            .filter_map(|parameter| match parameter {
                WhatsappTemplateParameter::Image { image } => image.link.as_deref(),
                WhatsappTemplateParameter::Video { video } => video.link.as_deref(),
                WhatsappTemplateParameter::Document { document } => document.link.as_deref(),
                _ => None,
            })
    }
    pub fn validate(&self) -> Result<(), ValidationError> {
        let invalid = |path, detail| ValidationError::new(path, detail);
        if self.name.is_empty()
            || self.name.len() > 512
            || !self
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(invalid(
                "/whatsapp_template/name",
                "use 1..512 lowercase ASCII letters, digits or underscores",
            ));
        }
        let code = &self.language.code;
        let (language, region) = code
            .split_once('_')
            .map_or((code.as_str(), None), |(a, b)| (a, Some(b)));
        if !(2..=3).contains(&language.len())
            || !language.bytes().all(|b| b.is_ascii_lowercase())
            || region.is_some_and(|r| r.len() != 2 || !r.bytes().all(|b| b.is_ascii_uppercase()))
        {
            return Err(invalid(
                "/whatsapp_template/language/code",
                "use a language code such as en, en_US or zh_CN",
            ));
        }
        if self.components.len() > MAX_TEMPLATE_COMPONENTS {
            return Err(invalid(
                "/whatsapp_template/components",
                "at most five components are supported",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut total = 0;
        for (i, component) in self.components.iter().enumerate() {
            let path = format!("/whatsapp_template/components/{i}");
            let (identity, parameters, max) = match component {
                WhatsappTemplateComponent::Header { parameters } => {
                    ("header".into(), parameters, 1)
                }
                WhatsappTemplateComponent::Body { parameters } => {
                    ("body".into(), parameters, MAX_TEMPLATE_PARAMETERS)
                }
                WhatsappTemplateComponent::Button {
                    index, parameters, ..
                } => {
                    if !matches!(index.as_str(), "0" | "1" | "2") {
                        return Err(ValidationError::new(
                            format!("{path}/index"),
                            "URL button index must be 0, 1 or 2",
                        ));
                    }
                    (format!("button:{index}"), parameters, 1)
                }
            };
            if !seen.insert(identity) || parameters.is_empty() || parameters.len() > max {
                return Err(ValidationError::new(
                    &path,
                    "component must be unique and contain its bounded nonempty parameter list",
                ));
            }
            total += parameters.len();
            if total > MAX_TEMPLATE_PARAMETERS {
                return Err(ValidationError::new(
                    &path,
                    "at most twenty parameters are supported in total",
                ));
            }
            for (j, parameter) in parameters.iter().enumerate() {
                let allowed = match component {
                    WhatsappTemplateComponent::Header { .. } => matches!(
                        parameter,
                        WhatsappTemplateParameter::Text { .. }
                            | WhatsappTemplateParameter::Image { .. }
                            | WhatsappTemplateParameter::Video { .. }
                            | WhatsappTemplateParameter::Document { .. }
                    ),
                    WhatsappTemplateComponent::Body { .. } => matches!(
                        parameter,
                        WhatsappTemplateParameter::Text { .. }
                            | WhatsappTemplateParameter::Currency { .. }
                            | WhatsappTemplateParameter::DateTime { .. }
                    ),
                    WhatsappTemplateComponent::Button { .. } => {
                        matches!(parameter, WhatsappTemplateParameter::Text { .. })
                    }
                };
                let parameter_path = format!("{path}/parameters/{j}");
                if !allowed {
                    return Err(ValidationError::new(
                        &parameter_path,
                        "parameter kind is not supported in this component",
                    ));
                }
                parameter.validate(&parameter_path)?;
            }
        }
        Ok(())
    }
}
