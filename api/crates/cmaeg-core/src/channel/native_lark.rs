//! A separately provisioned single-tenant custom app; never an existing NyxID app.
use super::{AdmissionPolicy, Secret};
use crate::ValidationError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LarkRegion {
    Lark,
    Feishu,
}
impl LarkRegion {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lark => "lark",
            Self::Feishu => "feishu",
        }
    }
    pub fn origin(self) -> &'static str {
        match self {
            Self::Lark => "https://open.larksuite.com",
            Self::Feishu => "https://open.feishu.cn",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeLarkIdentity {
    pub region: LarkRegion,
    pub app_id: String,
    pub tenant_key: String,
    /// Independently checked using the app's bot-info endpoint.
    pub bot_open_id: String,
}
pub fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
impl NativeLarkIdentity {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !safe_id(&self.app_id)
            || !self.app_id.starts_with("cli_")
            || !safe_id(&self.tenant_key)
            || !safe_id(&self.bot_open_id)
            || !self.bot_open_id.starts_with("ou_")
        {
            return Err(ValidationError::new(
                "/sources/native_lark/identity",
                "requires exact region, app, tenant and bot open_id",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeLarkSource {
    /// Must be true: this source requires a separate native app and webhook.
    pub separate_app: bool,
    pub identity: NativeLarkIdentity,
    pub admission: AdmissionPolicy,
    /// Owner consent to earlier visible messages in the exact original chat/thread.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub conversation_history: bool,
}
impl NativeLarkSource {
    pub fn validate(
        &self,
        credentials: Option<&NativeLarkCredentials>,
    ) -> Result<(), ValidationError> {
        self.identity.validate()?;
        if !self.separate_app || self.admission.native_bot_mentions() {
            return Err(ValidationError::new(
                "/sources/native_lark",
                "requires a separately configured native app; bot messages are unsupported",
            ));
        }
        credentials
            .ok_or_else(|| {
                ValidationError::new(
                    "/credentials/native_lark",
                    "native app credentials required",
                )
            })?
            .validate()?;
        super::validation::validate_admission(&self.admission, "/sources/native_lark")
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeLarkCredentials {
    pub app_secret: Secret,
    pub encrypt_key: Secret,
    pub verification_token: Secret,
}
impl NativeLarkCredentials {
    pub fn validate(&self) -> Result<(), ValidationError> {
        for value in [
            &self.app_secret,
            &self.encrypt_key,
            &self.verification_token,
        ] {
            if value.as_str().len() < 16
                || value.as_str().len() > 512
                || !value.as_str().bytes().all(|b| (33..=126).contains(&b))
            {
                return Err(ValidationError::new("/credentials/native_lark", "requires bounded printable app secret, Encrypt Key and Verification Token, at least 16 bytes each"));
            }
        }
        Ok(())
    }
    pub(super) fn private_json(&self) -> serde_json::Value {
        serde_json::json!({"app_secret":self.app_secret.as_str(),"encrypt_key":self.encrypt_key.as_str(),"verification_token":self.verification_token.as_str()})
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeLarkReply {
    /// Retained exact receiving-bot mention proof from original admission.
    pub mentioned: bool,
    pub identity: NativeLarkIdentity,
    pub chat_id: String,
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Digest of the exact source and credential revision at admission.
    pub revision: String,
}
impl NativeLarkReply {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.identity.validate()?;
        if !safe_id(&self.chat_id)
            || !self.chat_id.starts_with("oc_")
            || !safe_id(&self.message_id)
            || !self.message_id.starts_with("om_")
            || self.thread_id.as_ref().is_some_and(|id| !safe_id(id))
            || self.revision.len() != 64
            || !self.revision.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(ValidationError::new(
                "/reply",
                "invalid native Lark destination or revision",
            ));
        }
        Ok(())
    }
}
