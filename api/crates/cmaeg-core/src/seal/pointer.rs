//! Channel record pointers sealed as `cmaeg1.ch` tokens.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    channel::validate_https_url,
    ids::{ChannelId, RecordId},
    seal::{open, seal, MasterKeyRing, OpenError, Opened, TokenKind},
    ValidationError,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelPointer {
    pub v: u8,
    pub record_url: String,
    pub record_id: RecordId,
}

impl ChannelPointer {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.v != 1 {
            return Err(ValidationError::new("/v", "unsupported pointer version"));
        }
        validate_https_url(&self.record_url, "/record_url")
    }

    pub fn seal_id(&self, ring: &MasterKeyRing) -> Result<ChannelId, crate::seal::SealError> {
        seal(ring, TokenKind::Channel, self).map(ChannelId::from_sealed)
    }

    pub fn open_id(id: &ChannelId, ring: &MasterKeyRing) -> Result<Opened<Self>, OpenError> {
        let opened: Opened<Self> = open(ring, TokenKind::Channel, id.as_str())?;
        opened.payload.validate().map_err(|_| OpenError::Format)?;
        Ok(opened)
    }
}
