//! Bounded contact cards and static geographic facts; never recipients or live tracking.
use crate::{require_string, ValidationError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StructuredContent {
    Contact {
        phone_number: String,
        first_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vcard: Option<String>,
    },
    Location {
        latitude: f64,
        longitude: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        horizontal_accuracy: Option<f64>,
    },
    Venue {
        latitude: f64,
        longitude: f64,
        title: String,
        address: String,
    },
}

impl StructuredContent {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Contact {
                phone_number,
                first_name,
                last_name,
                vcard,
            } => {
                text(phone_number, "phone_number", 128)?;
                text(first_name, "first_name", 1024)?;
                if let Some(value) = last_name {
                    text(value, "last_name", 1024)?;
                }
                if let Some(value) = vcard {
                    require_string(value, "/structured_content/vcard", 0, 2048)?;
                    if value.chars().any(|c| {
                        crate::activity::forbidden_text_char(c) && !matches!(c, '\n' | '\r' | '\t')
                    }) {
                        return Err(invalid("vcard"));
                    }
                }
            }
            Self::Location {
                latitude,
                longitude,
                horizontal_accuracy,
            } => {
                coordinates(*latitude, *longitude)?;
                if horizontal_accuracy
                    .is_some_and(|n| !n.is_finite() || !(0.0..=1500.0).contains(&n))
                {
                    return Err(invalid("horizontal_accuracy"));
                }
            }
            Self::Venue {
                latitude,
                longitude,
                title,
                address,
            } => {
                coordinates(*latitude, *longitude)?;
                text(title, "title", 1024)?;
                text(address, "address", 4096)?;
            }
        }
        Ok(())
    }
}
fn coordinates(latitude: f64, longitude: f64) -> Result<(), ValidationError> {
    if !latitude.is_finite() || !(-90.0..=90.0).contains(&latitude) {
        return Err(invalid("latitude"));
    }
    if !longitude.is_finite() || !(-180.0..=180.0).contains(&longitude) {
        return Err(invalid("longitude"));
    }
    Ok(())
}
fn text(value: &str, field: &str, max: usize) -> Result<(), ValidationError> {
    require_string(value, &format!("/structured_content/{field}"), 1, max)?;
    if value.trim().is_empty() || value.chars().any(crate::activity::forbidden_text_char) {
        return Err(invalid(field));
    }
    Ok(())
}
fn invalid(field: &str) -> ValidationError {
    ValidationError::new(
        format!("/structured_content/{field}"),
        "invalid static structured content",
    )
}
