//! Record domains retain their own identifiers, ownership and retention rules.
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    #[default]
    Channel,
    Connection,
    Interaction,
    WhatsappFlow,
}

impl RecordKind {
    pub fn collection(self) -> &'static str {
        match self {
            Self::Channel => super::mongo::RECORDS,
            Self::Connection => super::mongo::CONNECTIONS,
            Self::Interaction => super::mongo::INTERACTIONS,
            Self::WhatsappFlow => super::mongo::WHATSAPP_FLOWS,
        }
    }
}
