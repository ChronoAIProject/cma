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

/// Meta webhooks use base64; Graph metadata may report the same digest in hex.
pub fn media_sha256(value: &str) -> Option<[u8; 32]> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        let mut digest = [0; 32];
        for (index, output) in digest.iter_mut().enumerate() {
            *output = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
        }
        return Some(digest);
    }
    if value.len() != 44 {
        return None;
    }
    STANDARD.decode(value).ok()?.try_into().ok()
}

pub(super) fn validate(
    group_ids: &[String],
    app_id: &str,
    waba_id: &str,
    phone_number_id: &str,
    admission: &AdmissionPolicy,
    credentials: Option<&WhatsappCredentials>,
    pointer: &str,
) -> Result<(), ValidationError> {
    validate_groups(group_ids, pointer)?;
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

/// Provider group IDs are opaque base64/base64url identifiers, never phone numbers.
pub fn valid_group_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'/' | b'=')
        })
}
fn validate_groups(values: &[String], pointer: &str) -> Result<(), ValidationError> {
    let mut unique = std::collections::BTreeSet::new();
    if values.len() > 16
        || values
            .iter()
            .any(|value| !valid_group_id(value) || !unique.insert(value))
    {
        return Err(ValidationError::new(
            format!("{pointer}/group_ids"),
            "at most 16 unique bounded Meta group IDs required",
        ));
    }
    Ok(())
}

/// A group reply never substitutes the participant for its group destination.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WhatsappGroupReply {
    /// Exact source and credential revision; retained group effects revoke on rotation.
    pub revision: String,
    pub app_id: String,
    pub waba_id: String,
    pub phone_number_id: String,
    pub group_id: String,
    pub participant_id: String,
    pub message_id: String,
}
impl WhatsappGroupReply {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if [
            &self.app_id,
            &self.waba_id,
            &self.phone_number_id,
            &self.participant_id,
        ]
        .iter()
        .any(|value| !valid_meta_id(value))
            || self.revision.len() != 64
            || !self
                .revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || !valid_group_id(&self.group_id)
            || !valid_message_id(&self.message_id)
        {
            return Err(ValidationError::new(
                "/reply",
                "invalid native WhatsApp group destination",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whatsapp_groups_are_bounded_unique_and_optional() {
        let old = serde_json::json!({"type":"whatsapp","app_id":"12345","waba_id":"23456","phone_number_id":"34567","admission":{"type":"all"}});
        let source: crate::channel::Source = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(source).unwrap(), old);
        assert!(validate_groups(&[], "/sources/0").is_ok());
        assert!(validate_groups(&["opaque+/==".into()], "/sources/0").is_ok());
        for values in [
            vec!["".into()],
            vec!["../../x".into()],
            vec!["a".into(), "a".into()],
            vec!["a".repeat(513)],
            (0..17).map(|i| format!("group{i}")).collect(),
        ] {
            assert!(validate_groups(&values, "/sources/0").is_err());
        }
    }
    #[test]
    fn whatsapp_group_reply_wire_is_distinct_from_legacy_private_reply() {
        use crate::seal::ReplyAddress;
        let private = serde_json::json!({"type":"whatsapp","app_id":"12345","waba_id":"23456","phone_number_id":"34567","recipient_id":"447700900123","message_id":"wamid.original"});
        let reply: ReplyAddress = serde_json::from_value(private.clone()).unwrap();
        assert_eq!(serde_json::to_value(reply).unwrap(), private);
        let group = WhatsappGroupReply {
            revision: "ab".repeat(32),
            app_id: "12345".into(),
            waba_id: "23456".into(),
            phone_number_id: "34567".into(),
            group_id: "opaque+/==".into(),
            participant_id: "447700900123".into(),
            message_id: "wamid.original".into(),
        };
        group.validate().unwrap();
        let value = serde_json::to_value(ReplyAddress::WhatsappGroup(group.clone())).unwrap();
        assert_eq!(value["type"], "whatsapp_group");
        assert_eq!(
            serde_json::from_value::<ReplyAddress>(value).unwrap(),
            ReplyAddress::WhatsappGroup(group.clone())
        );
        let mut invalid = group;
        invalid.participant_id = invalid.group_id.clone();
        assert!(invalid.validate().is_err());
    }
}
