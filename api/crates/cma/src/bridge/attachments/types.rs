//! Private connector authority; callback references are never fetch grants.
use std::time::Duration;

use layer2_core::{
    bridge::{ingress::InboxAttachment, BridgePlatform, BridgeWorkPin},
    resource_access::ResourceOwner,
};
use reqwest::Url;
use tokio::time::Instant;
use zeroize::Zeroizing;

pub(super) const MAX_BYTES: usize = 1_048_576;
pub(super) const MAX_DURATION: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unavailable {
    Authorization,
    Address,
    Transport,
    Response,
    Bounds,
    Mime,
    Scan,
    Deadline,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct SourceBinding {
    pub owner: ResourceOwner,
    pub bridge: BridgeWorkPin,
    pub bot_id: String,
    pub platform: BridgePlatform,
    pub source_id: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct FetchRequest {
    pub source: SourceBinding,
    pub reference: InboxAttachment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mime {
    Text,
    Png,
    Jpeg,
    Pdf,
}

impl Mime {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text/plain",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Pdf => "application/pdf",
        }
    }

    // Signature corroboration is only a MIME gate, not full format validation
    // or a malware scan. The separate trusted scanner must approve all bytes.
    pub(super) fn corroborates(self, bytes: &[u8]) -> bool {
        match self {
            Self::Text => layer2_core::bridge::ingress::valid_attachment_text(bytes),
            Self::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            Self::Jpeg => bytes.starts_with(&[0xff, 0xd8, 0xff]),
            Self::Pdf => bytes.starts_with(b"%PDF-"),
        }
    }
}

/// Supplied by trusted connector configuration, independently of a callback.
/// Exact names only; suffix/wildcard policies and literal addresses are absent.
pub(super) struct ConnectorHosts(Vec<String>);

impl ConnectorHosts {
    pub(super) fn new(hosts: &[&str]) -> Result<Self, Unavailable> {
        if hosts.is_empty() || hosts.len() > 16 {
            return Err(Unavailable::Authorization);
        }
        let mut names = Vec::new();
        for host in hosts {
            if host.len() > 253
                || host.ends_with('.')
                || !host.contains('.')
                || !host
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'-')
                || host
                    .split('.')
                    .any(|s| s.is_empty() || s.len() > 63 || s.starts_with('-') || s.ends_with('-'))
            {
                return Err(Unavailable::Authorization);
            }
            let origin =
                Url::parse(&format!("https://{host}/")).map_err(|_| Unavailable::Authorization)?;
            if origin.domain() != Some(*host) {
                return Err(Unavailable::Authorization);
            }
            names.push((*host).to_owned());
        }
        Ok(Self(names))
    }
}

/// No Deserialize/Debug: a signed callback cannot manufacture authority or
/// expose an authorized URL (which may itself contain an access credential).
pub(super) struct FetchGrant {
    pub(super) request: FetchRequest,
    pub(super) url: Url,
    pub(super) host: String,
    pub(super) mime: Mime,
    pub(super) max_bytes: usize,
    pub(super) deadline: Instant,
}

impl FetchGrant {
    /// Only an independently authorized connector may issue this grant. The
    /// URL can differ from an opaque reference; never forward a NyxID key here.
    pub(super) fn issue(
        request: FetchRequest,
        hosts: &ConnectorHosts,
        authorized_url: &str,
        mime: Mime,
        max_bytes: usize,
        deadline: Instant,
    ) -> Result<Self, Unavailable> {
        if authorized_url.len() > 4096 || max_bytes == 0 || max_bytes > MAX_BYTES {
            return Err(Unavailable::Bounds);
        }
        if !authorized_url.starts_with("https://")
            || authorized_url
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
        {
            return Err(Unavailable::Address);
        }
        let url = Url::parse(authorized_url).map_err(|_| Unavailable::Address)?;
        let host = url.domain().ok_or(Unavailable::Address)?.to_owned();
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.port_or_known_default() != Some(443)
            || !hosts.0.contains(&host)
        {
            return Err(Unavailable::Address);
        }
        if deadline <= Instant::now() || deadline > Instant::now() + MAX_DURATION {
            return Err(Unavailable::Deadline);
        }
        Ok(Self {
            request,
            url,
            host,
            mime,
            max_bytes,
            deadline,
        })
    }
}

/// Transient private bytes; the composed worker separately captures approved text.
/// Contents are not printable/serializable and are wiped on dropped failures.
pub(super) struct FetchedAttachment {
    pub(super) bytes: Zeroizing<Vec<u8>>,
    pub(super) mime: Mime,
    pub(super) sha256: String,
    pub(super) source: SourceBinding,
}
