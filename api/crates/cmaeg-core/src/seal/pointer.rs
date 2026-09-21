//! Channel record pointers sealed as `cmaeg1.ch` tokens.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
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
        let url = url::Url::parse(&self.record_url)
            .map_err(|_| ValidationError::new("/record_url", "must be an absolute record URL"))?;
        let allowed_http = url.scheme() == "http"
            && url.host().is_some_and(|host| match host {
                url::Host::Domain(host) => host == "localhost" || host.ends_with(".svc"),
                url::Host::Ipv4(ip) => ip.is_loopback(),
                url::Host::Ipv6(ip) => ip.is_loopback(),
            });
        if url.host_str().is_none()
            || url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !(url.scheme() == "https" || allowed_http)
        {
            return Err(ValidationError::new(
                "/record_url",
                "must use https or loopback/.svc http without credentials, query or fragment",
            ));
        }
        Ok(())
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
