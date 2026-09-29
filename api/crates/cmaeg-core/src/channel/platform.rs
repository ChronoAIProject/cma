//! NyxID relay platforms and their spelling on the relay wire.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Telegram,
    Lark,
    Feishu,
    Discord,
    Slack,
    Whatsapp,
}

impl Platform {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Lark => "lark",
            Self::Feishu => "feishu",
            Self::Discord => "discord",
            Self::Slack => "slack",
            Self::Whatsapp => "whatsapp",
        }
    }

    /// Whether a NyxID platform string (header, claim or body) names this platform.
    pub fn matches_wire(self, value: &str) -> bool {
        value.parse::<Self>().is_ok_and(|platform| platform == self)
    }

    /// Whether the gateway derives sender and addressing facts from the raw platform
    /// event. Other platforms can only be admitted from the signed normalized envelope,
    /// which a source must accept explicitly with `trust_normalized`.
    pub const fn verifies_raw_event(self) -> bool {
        matches!(self, Self::Telegram | Self::Lark | Self::Feishu)
    }
}

impl FromStr for Platform {
    type Err = ValidationError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            // NyxID managed Telegram bots identify their transport as `telegram-new`.
            "telegram" | "telegram-new" => Ok(Self::Telegram),
            "lark" => Ok(Self::Lark),
            "feishu" => Ok(Self::Feishu),
            "discord" => Ok(Self::Discord),
            "slack" => Ok(Self::Slack),
            "whatsapp" => Ok(Self::Whatsapp),
            _ => Err(ValidationError::new("/platform", "unsupported platform")),
        }
    }
}

impl ChannelDefinition {
    /// The platform of the channel's NyxID relay source, if it has one.
    pub fn relay_platform(&self) -> Option<Platform> {
        self.sources.iter().find_map(|source| match source {
            Source::NyxidRelay { platform, .. } => Some(*platform),
            Source::Custom { .. } => None,
        })
    }
}
