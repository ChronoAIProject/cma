//! Explicit, non-secret authority binding for rich-post resource reads.
use super::Platform;
use crate::ValidationError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LarkResourceBinding {
    /// Exact allowed NyxID UserService; never a provider credential.
    pub user_service_id: String,
    pub catalog_service_id: String,
    /// Receiving app from the independently configured relay bot.
    pub app_id: String,
}

impl LarkResourceBinding {
    pub fn validate(&self, pointer: &str) -> Result<(), ValidationError> {
        for (field, value) in [
            ("user_service_id", self.user_service_id.as_str()),
            ("catalog_service_id", self.catalog_service_id.as_str()),
            ("app_id", self.app_id.as_str()),
        ] {
            if !safe_id(value, 128) || (field == "app_id" && !value.starts_with("cli_")) {
                return Err(ValidationError::new(
                    format!("{pointer}/{field}"),
                    "requires a bounded platform identifier",
                ));
            }
        }
        Ok(())
    }
}

pub fn safe_id(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub fn lark_resource_service(platform: Platform) -> Option<(&'static str, &'static str)> {
    match platform {
        Platform::Lark => Some(("api-lark-bot", "https://open.larksuite.com")),
        Platform::Feishu => Some(("api-feishu-bot", "https://open.feishu.cn")),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LarkResourceKind {
    Image,
    File,
}
impl LarkResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::File => "file",
        }
    }
}
