//! Native Meta identities and private credentials; relay WhatsApp is independent.
use super::{AdmissionPolicy, Secret};
use crate::ValidationError;
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WhatsappCredentials {
    pub app_secret: Secret,
    pub access_token: Secret,
    pub verify_token: Secret,
}

impl WhatsappCredentials {
    pub fn validate(&self) -> Result<(), ValidationError> {
        for (name, secret, max) in [
            ("app_secret", &self.app_secret, 256),
            ("access_token", &self.access_token, 16_384),
            ("verify_token", &self.verify_token, 256),
        ] {
            let value = secret.as_str();
            if value.len() < 16
                || value.len() > max
                || !value.bytes().all(|byte| (33..=126).contains(&byte))
            {
                return Err(ValidationError::new(
                    format!("/credentials/whatsapp/{name}"),
                    "requires bounded printable secret bytes, at least 16 bytes",
                ));
            }
        }
        Ok(())
    }

    /// Only the encrypted channel serializer may use these plaintext values.
    pub(super) fn private_json(&self) -> serde_json::Value {
        serde_json::json!({
            "app_secret": self.app_secret.as_str(),
            "access_token": self.access_token.as_str(),
            "verify_token": self.verify_token.as_str(),
        })
    }
}

pub fn valid_meta_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && !value.starts_with('0')
}

pub fn valid_message_id(value: &str) -> bool {
    value.starts_with("wamid.")
        && value.len() > 6
        && value.len() <= 512
        && value.bytes().all(|byte| (33..=126).contains(&byte))
}

pub(super) fn validate(
    app_id: &str,
    waba_id: &str,
    phone_number_id: &str,
    admission: &AdmissionPolicy,
    credentials: Option<&WhatsappCredentials>,
    pointer: &str,
) -> Result<(), ValidationError> {
    for (name, value) in [
        ("app_id", app_id),
        ("waba_id", waba_id),
        ("phone_number_id", phone_number_id),
    ] {
        if !valid_meta_id(value) {
            return Err(ValidationError::new(
                format!("{pointer}/{name}"),
                "requires a numeric Meta identifier",
            ));
        }
    }
    credentials
        .ok_or_else(|| {
            ValidationError::new(
                "/credentials/whatsapp",
                "native WhatsApp credentials required",
            )
        })?
        .validate()?;
    super::validation::validate_admission(admission, pointer)
}
