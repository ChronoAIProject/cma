//! Lenient, bounded parse of the NyxID relay attachment list.
//!
//! NyxID copies sender-controlled facts (file names, MIME types, sizes) into the
//! callback envelope and adds attachment fields over time. A strict item type
//! turned every such drift into a 401 that NyxID never retries, so the sender's
//! message was lost. Attachment items are therefore parsed leniently into the
//! sanitized [`Attachment`]: facts outside CMAEG's bounds are repaired or dropped
//! and counted, while the rest of the envelope stays strict. No intermediate JSON
//! value is built; the request body cap is the only memory bound this parse needs.
use super::payload::{identifier, Attachment};
use crate::telemetry::names;
use serde::de::{Deserialize, Deserializer, Error, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::fmt;

/// Positions at or beyond this index are consumed and counted, never kept.
pub const MAX_RELAY_ATTACHMENTS: usize = 32;
const MAX_URL: usize = 4096;

/// A kept attachment and its original position in NyxID's list. The position is
/// the `{index}` of NyxID's attachment download route, so items are never renumbered.
pub struct IndexedAttachment {
    pub index: u8,
    pub attachment: Attachment,
}

/// The sanitized attachment list. `received` counts every raw position, including
/// dropped ones, so presence checks refuse a malformed list instead of treating it
/// as a text-only message.
#[derive(Default)]
pub struct RelayAttachments {
    pub kept: Vec<IndexedAttachment>,
    pub received: u32,
    pub repairs: RelayRepairs,
}

/// The bounded set of envelope facts a repair can name; each is a metric label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairField {
    AttachmentItem,
    AttachmentOverflow,
    AttachmentContainer,
    ContentType,
    Url,
    PlatformMessageId,
    FileKey,
    ImageKey,
    Filename,
    MimeType,
    SizeBytes,
    DownloadUrl,
    SenderDisplayName,
}
impl RepairField {
    const ALL: [Self; 13] = [
        Self::AttachmentItem,
        Self::AttachmentOverflow,
        Self::AttachmentContainer,
        Self::ContentType,
        Self::Url,
        Self::PlatformMessageId,
        Self::FileKey,
        Self::ImageKey,
        Self::Filename,
        Self::MimeType,
        Self::SizeBytes,
        Self::DownloadUrl,
        Self::SenderDisplayName,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::AttachmentItem => "attachment_item",
            Self::AttachmentOverflow => "attachment_overflow",
            Self::AttachmentContainer => "attachment_container",
            Self::ContentType => "content_type",
            Self::Url => "url",
            Self::PlatformMessageId => "platform_message_id",
            Self::FileKey => "file_key",
            Self::ImageKey => "image_key",
            Self::Filename => "filename",
            Self::MimeType => "mime_type",
            Self::SizeBytes => "size_bytes",
            Self::DownloadUrl => "download_url",
            Self::SenderDisplayName => "sender_display_name",
        }
    }
}

/// Per-field repair counts for one callback.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelayRepairs([u32; 13]);
impl RelayRepairs {
    pub fn record(&mut self, field: RepairField) {
        let count = &mut self.0[field as usize];
        *count = count.saturating_add(1);
    }
    pub fn count(&self, field: RepairField) -> u32 {
        self.0[field as usize]
    }
    /// Non-zero counts only.
    pub fn iter(&self) -> impl Iterator<Item = (RepairField, u32)> {
        RepairField::ALL
            .into_iter()
            .zip(self.0)
            .filter(|(_, count)| *count > 0)
    }
    /// Callers record only after authentication, so unauthenticated bodies cannot
    /// inflate the counter.
    pub fn record_metrics(&self) {
        for (field, count) in self.iter() {
            metrics::counter!(names::RELAY_ENVELOPE_REPAIRS, names::FIELD => field.label())
                .increment(u64::from(count));
        }
    }
    fn merge(&mut self, other: &Self) {
        for (total, count) in self.0.iter_mut().zip(other.0) {
            *total = total.saturating_add(count);
        }
    }
}

impl<'de> Deserialize<'de> for RelayAttachments {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ContainerVisitor)
    }
}

/// Implements every scalar visit as "drain and return `$value`", so no JSON type
/// is ever a parse error. Strings are borrowed where the input allows.
macro_rules! accept_scalars {
    ($value:expr) => {
        fn visit_bool<E: Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok($value)
        }
        fn visit_i64<E: Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok($value)
        }
        fn visit_u64<E: Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok($value)
        }
        fn visit_f64<E: Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok($value)
        }
        fn visit_str<E: Error>(self, _: &str) -> Result<Self::Value, E> {
            Ok($value)
        }
        fn visit_bytes<E: Error>(self, _: &[u8]) -> Result<Self::Value, E> {
            Ok($value)
        }
    };
}

fn drain_map<'de, A: MapAccess<'de>>(map: &mut A) -> Result<(), A::Error> {
    while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
    Ok(())
}
fn drain_seq<'de, A: SeqAccess<'de>>(seq: &mut A) -> Result<(), A::Error> {
    while seq.next_element::<IgnoredAny>()?.is_some() {}
    Ok(())
}

struct ContainerVisitor;
impl ContainerVisitor {
    /// A non-list container is one malformed position: it is refused, never kept.
    fn malformed() -> RelayAttachments {
        let mut list = RelayAttachments {
            received: 1,
            ..RelayAttachments::default()
        };
        list.repairs.record(RepairField::AttachmentContainer);
        list
    }
}
impl<'de> Visitor<'de> for ContainerVisitor {
    type Value = RelayAttachments;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an attachment list")
    }
    fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
        Ok(RelayAttachments::default())
    }
    fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
        Ok(RelayAttachments::default())
    }
    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut list = RelayAttachments::default();
        let mut position = 0_usize;
        loop {
            if position < MAX_RELAY_ATTACHMENTS {
                let Some(item) = seq.next_element::<LenientItem>()? else {
                    break;
                };
                list.repairs.merge(&item.repairs);
                if let Some(attachment) = item.attachment {
                    list.kept.push(IndexedAttachment {
                        // Bounded by MAX_RELAY_ATTACHMENTS, which fits in a byte.
                        index: position as u8,
                        attachment,
                    });
                }
            } else {
                if seq.next_element::<IgnoredAny>()?.is_none() {
                    break;
                }
                list.repairs.record(RepairField::AttachmentOverflow);
            }
            position += 1;
            list.received = u32::try_from(position).unwrap_or(u32::MAX);
        }
        Ok(list)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        drain_map(&mut map)?;
        Ok(Self::malformed())
    }
    accept_scalars!(Self::malformed());
}

/// One raw list position: an object becomes an attachment, anything else is dropped.
struct LenientItem {
    attachment: Option<Attachment>,
    repairs: RelayRepairs,
}
impl LenientItem {
    fn dropped() -> Self {
        let mut repairs = RelayRepairs::default();
        repairs.record(RepairField::AttachmentItem);
        Self {
            attachment: None,
            repairs,
        }
    }
}
impl<'de> Deserialize<'de> for LenientItem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ItemVisitor)
    }
}
struct ItemVisitor;
impl<'de> Visitor<'de> for ItemVisitor {
    type Value = LenientItem;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an attachment")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut raw = RawItem::default();
        while let Some(key) = map.next_key::<FactName>()? {
            match raw.slot(key) {
                // The first value of a known key wins; later duplicates are drained.
                Some(slot) if slot.is_none() => *slot = Some(map.next_value::<Lenient>()?),
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(raw.sanitize())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        drain_seq(&mut seq)?;
        Ok(LenientItem::dropped())
    }
    fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
        Ok(LenientItem::dropped())
    }
    accept_scalars!(LenientItem::dropped());
}

/// Attachment keys NyxID serializes; any other key is ignored because NyxID adds
/// fields over time.
#[derive(Clone, Copy)]
enum FactName {
    ContentType,
    Url,
    PlatformMessageId,
    FileKey,
    ImageKey,
    Filename,
    MimeType,
    SizeBytes,
    DownloadUrl,
    Unknown,
}
impl<'de> Deserialize<'de> for FactName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_identifier(FactNameVisitor)
    }
}
struct FactNameVisitor;
impl Visitor<'_> for FactNameVisitor {
    type Value = FactName;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an attachment field name")
    }
    fn visit_str<E: Error>(self, value: &str) -> Result<FactName, E> {
        Ok(match value {
            "content_type" => FactName::ContentType,
            "url" => FactName::Url,
            "platform_message_id" => FactName::PlatformMessageId,
            "file_key" => FactName::FileKey,
            "image_key" => FactName::ImageKey,
            "filename" => FactName::Filename,
            "mime_type" => FactName::MimeType,
            "size_bytes" => FactName::SizeBytes,
            "download_url" => FactName::DownloadUrl,
            _ => FactName::Unknown,
        })
    }
    fn visit_bytes<E: Error>(self, value: &[u8]) -> Result<FactName, E> {
        std::str::from_utf8(value).map_or(Ok(FactName::Unknown), |value| self.visit_str(value))
    }
}

/// Any JSON value, reduced to the shapes an attachment fact can take.
enum Lenient {
    Str(String),
    U64(u64),
    Null,
    Other,
}
impl<'de> Deserialize<'de> for Lenient {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(LenientVisitor)
    }
}
struct LenientVisitor;
impl<'de> Visitor<'de> for LenientVisitor {
    type Value = Lenient;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }
    fn visit_str<E: Error>(self, value: &str) -> Result<Lenient, E> {
        Ok(Lenient::Str(value.to_owned()))
    }
    fn visit_string<E: Error>(self, value: String) -> Result<Lenient, E> {
        Ok(Lenient::Str(value))
    }
    fn visit_u64<E: Error>(self, value: u64) -> Result<Lenient, E> {
        Ok(Lenient::U64(value))
    }
    fn visit_i64<E: Error>(self, value: i64) -> Result<Lenient, E> {
        Ok(u64::try_from(value).map_or(Lenient::Other, Lenient::U64))
    }
    fn visit_f64<E: Error>(self, _: f64) -> Result<Lenient, E> {
        Ok(Lenient::Other)
    }
    fn visit_bool<E: Error>(self, _: bool) -> Result<Lenient, E> {
        Ok(Lenient::Other)
    }
    fn visit_bytes<E: Error>(self, _: &[u8]) -> Result<Lenient, E> {
        Ok(Lenient::Other)
    }
    fn visit_unit<E: Error>(self) -> Result<Lenient, E> {
        Ok(Lenient::Null)
    }
    fn visit_none<E: Error>(self) -> Result<Lenient, E> {
        Ok(Lenient::Null)
    }
    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Lenient, D::Error> {
        deserializer.deserialize_any(self)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Lenient, A::Error> {
        drain_seq(&mut seq)?;
        Ok(Lenient::Other)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Lenient, A::Error> {
        drain_map(&mut map)?;
        Ok(Lenient::Other)
    }
}

/// The first raw value of each known key, before bounds are applied.
#[derive(Default)]
struct RawItem {
    content_type: Option<Lenient>,
    url: Option<Lenient>,
    platform_message_id: Option<Lenient>,
    file_key: Option<Lenient>,
    image_key: Option<Lenient>,
    filename: Option<Lenient>,
    mime_type: Option<Lenient>,
    size_bytes: Option<Lenient>,
    download_url: Option<Lenient>,
}
impl RawItem {
    fn slot(&mut self, key: FactName) -> Option<&mut Option<Lenient>> {
        Some(match key {
            FactName::ContentType => &mut self.content_type,
            FactName::Url => &mut self.url,
            FactName::PlatformMessageId => &mut self.platform_message_id,
            FactName::FileKey => &mut self.file_key,
            FactName::ImageKey => &mut self.image_key,
            FactName::Filename => &mut self.filename,
            FactName::MimeType => &mut self.mime_type,
            FactName::SizeBytes => &mut self.size_bytes,
            FactName::DownloadUrl => &mut self.download_url,
            FactName::Unknown => return None,
        })
    }
    /// Applies the same bounds the strict envelope check used to enforce, repairing
    /// instead of rejecting.
    fn sanitize(self) -> LenientItem {
        let mut repairs = RelayRepairs::default();
        let content_type = match self.content_type {
            Some(Lenient::Str(value)) if identifier(&value) => value,
            _ => {
                repairs.record(RepairField::ContentType);
                "unknown".to_owned()
            }
        };
        let url = match self.url {
            Some(Lenient::Str(value)) if retrievable_url(&value) => value,
            _ => {
                repairs.record(RepairField::Url);
                String::new()
            }
        };
        let mut text = |value: Option<Lenient>, max: usize, field: RepairField| match value {
            None | Some(Lenient::Null) => None,
            Some(Lenient::Str(value)) if bounded(&value, max) => Some(value),
            Some(_) => {
                repairs.record(field);
                None
            }
        };
        let platform_message_id = text(
            self.platform_message_id,
            256,
            RepairField::PlatformMessageId,
        );
        let file_key = text(self.file_key, 1024, RepairField::FileKey);
        let image_key = text(self.image_key, 1024, RepairField::ImageKey);
        let filename = text(self.filename, 512, RepairField::Filename);
        let mime_type = text(self.mime_type, 256, RepairField::MimeType);
        let download_url = text(self.download_url, MAX_URL, RepairField::DownloadUrl);
        let size_bytes = match self.size_bytes {
            None | Some(Lenient::Null) => None,
            Some(Lenient::U64(size)) if size <= i64::MAX as u64 => Some(size),
            Some(_) => {
                repairs.record(RepairField::SizeBytes);
                None
            }
        };
        LenientItem {
            attachment: Some(Attachment {
                content_type,
                download_url,
                url,
                platform_message_id,
                file_key,
                image_key,
                filename,
                mime_type,
                size_bytes,
            }),
            repairs,
        }
    }
}

/// Non-empty, at most `max` bytes, without control characters.
fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

/// Empty (not retrievable), or an http(s) URL with a host and no user information.
fn retrievable_url(value: &str) -> bool {
    if value.len() > MAX_URL || value.chars().any(char::is_control) {
        return false;
    }
    value.is_empty()
        || reqwest::Url::parse(value).is_ok_and(|url| {
            matches!(url.scheme(), "https" | "http")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
}

#[cfg(test)]
#[path = "attachments_tests.rs"]
mod tests;
