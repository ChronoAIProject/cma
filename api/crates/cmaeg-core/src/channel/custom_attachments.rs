//! A custom source's opt-in to attachment descriptors carried in event data.
//!
//! The owner names where in `ce.data` a sender lists attachment items and the
//! exact HTTPS origins the gateway may later fetch them from. Ingress extracts
//! and validates the items against this setting; nothing here fetches.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{require_string, ValidationError};

pub const DEFAULT_CUSTOM_ATTACHMENT_POINTER: &str = "/attachments";
/// Ingress records an inline item's locator as `<pointer>/<i>/url`. With at
/// most sixteen items `i ≤ 15`, so the suffix adds at most seven bytes and a
/// 240-byte pointer keeps every locator within the 256-byte inline-locator
/// bound that `validate_attachments` enforces on the stored activity.
pub const MAX_CUSTOM_ATTACHMENT_POINTER_BYTES: usize = 240;
pub const MAX_CUSTOM_ATTACHMENT_ORIGINS: usize = 16;
/// The longest DNS name; an origin's host cannot usefully exceed it.
const MAX_ORIGIN_HOST_BYTES: usize = 253;

/// Where a custom event lists its attachments and which origins they may name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomAttachments {
    /// RFC 6901 pointer, relative to the event's `data`, at an array of items.
    #[serde(default = "default_pointer")]
    pub pointer: String,
    /// Exact HTTPS origins (`https://host[:port]`) an item URL may name. Empty
    /// admits only inline `data:` items.
    #[serde(default)]
    pub origins: Vec<String>,
}

fn default_pointer() -> String {
    DEFAULT_CUSTOM_ATTACHMENT_POINTER.to_owned()
}

impl CustomAttachments {
    /// Validate the setting, reporting `{base}/pointer` or `{base}/origins/{i}`.
    pub fn validate(&self, base: &str) -> Result<(), ValidationError> {
        validate_pointer(&self.pointer, &format!("{base}/pointer"))?;
        if self.origins.len() > MAX_CUSTOM_ATTACHMENT_ORIGINS {
            return Err(ValidationError::new(
                format!("{base}/origins/{MAX_CUSTOM_ATTACHMENT_ORIGINS}"),
                format!("at most {MAX_CUSTOM_ATTACHMENT_ORIGINS} origins are allowed"),
            ));
        }
        let mut seen = BTreeSet::new();
        for (index, origin) in self.origins.iter().enumerate() {
            let pointer = format!("{base}/origins/{index}");
            validate_origin(origin, &pointer)?;
            if !seen.insert(origin.as_str()) {
                return Err(ValidationError::new(pointer, "origins must be unique"));
            }
        }
        Ok(())
    }
}

fn validate_pointer(pointer: &str, at: &str) -> Result<(), ValidationError> {
    require_string(pointer, at, 1, MAX_CUSTOM_ATTACHMENT_POINTER_BYTES)?;
    let escapes_valid = pointer
        .split('~')
        .skip(1)
        .all(|escaped| escaped.starts_with(['0', '1']));
    if !pointer.starts_with('/') || !escapes_valid || pointer.chars().any(char::is_control) {
        return Err(ValidationError::new(
            at,
            "must be an RFC 6901 JSON pointer starting with '/' without control characters",
        ));
    }
    Ok(())
}

/// An origin is compared byte for byte with the origin of each item URL, so it
/// must already be in the serialized form a URL's origin takes: lowercase host,
/// no default port, no path, query, fragment or trailing slash.
fn validate_origin(origin: &str, at: &str) -> Result<(), ValidationError> {
    let invalid = || {
        ValidationError::new(
            at,
            "must be an exact https origin such as https://host[:port]",
        )
    };
    let url = url::Url::parse(origin).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.origin().ascii_serialization() != origin
    {
        return Err(invalid());
    }
    let host_valid = match url.host() {
        Some(url::Host::Domain(domain)) => dns_name(domain),
        Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => true,
        None => false,
    };
    if !host_valid
        || url
            .host_str()
            .is_none_or(|host| host.len() > MAX_ORIGIN_HOST_BYTES)
    {
        return Err(invalid());
    }
    Ok(())
}

/// URL parsing accepts characters such as `*` in a host, so a wildcard like
/// `https://*.example` would otherwise survive as a literal host name.
fn dns_name(domain: &str) -> bool {
    domain.split('.').all(|label| {
        !label.is_empty()
            && label.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte)
            })
    })
}
