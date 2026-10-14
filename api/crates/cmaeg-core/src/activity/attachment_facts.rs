//! Normalization of sender-declared attachment facts into the validated domain.
//!
//! Platform facts are hints the sender declared: Telegram documents width,
//! height, duration, file name and MIME type "as defined by the sender", widths
//! can be 0, names can run to hundreds of bytes and MIME types carry parameters.
//! An activity that fails validation is lost on the relay path, because NyxID
//! never retries a refused callback. Every extractor therefore passes its facts
//! through this one pure function, which uses the validator's own constants and
//! grammars so that the two domains cannot drift apart.

use super::attachment::{
    forbidden_text_char, mime_restricted_name, Attachment, MAX_DIMENSION, MAX_DURATION_SECONDS,
    MAX_EMOJI_BYTES, MAX_FILENAME_BYTES,
};

/// Marks the end of a shortened file name, so a reader knows it is incomplete.
const ELLIPSIS: &str = "…";
/// Longest ASCII extension preserved when a file name is shortened.
const MAX_KEPT_EXTENSION_BYTES: usize = 16;

/// The optional facts that [`Attachment::normalize_facts`] had to change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FactRepairs(u8);

impl FactRepairs {
    pub const FILENAME: Self = Self(1);
    pub const MIME_TYPE: Self = Self(1 << 1);
    pub const DIMENSIONS: Self = Self(1 << 2);
    pub const DURATION: Self = Self(1 << 3);
    pub const EMOJI: Self = Self(1 << 4);

    /// Metric label of each flag, in a stable order.
    const FIELDS: [(Self, &'static str); 5] = [
        (Self::FILENAME, "filename"),
        (Self::MIME_TYPE, "mime_type"),
        (Self::DIMENSIONS, "dimensions"),
        (Self::DURATION, "duration_seconds"),
        (Self::EMOJI, "emoji"),
    ];

    /// Whether every fact was already inside the validated domain.
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Whether every flag in `other` is set.
    pub fn contains(&self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The repaired fields, named for metric labels.
    pub fn fields(&self) -> impl Iterator<Item = &'static str> {
        let repairs = *self;
        Self::FIELDS
            .into_iter()
            .filter(move |(flag, _)| repairs.contains(*flag))
            .map(|(_, name)| name)
    }

    fn insert_if(&mut self, changed: bool, flag: Self) {
        if changed {
            self.0 |= flag.0;
        }
    }
}

impl Attachment {
    /// Map every optional fact into the domain `validate_attachments` accepts.
    ///
    /// Only `filename`, `mime_type`, `width`, `height`, `duration_seconds` and
    /// `emoji` can change; the reference, kind, size, variant and locators are
    /// left exactly as extracted. A value that cannot be repaired is dropped
    /// rather than invented, and a flag is reported only for a value that
    /// actually changed.
    pub fn normalize_facts(&mut self) -> FactRepairs {
        let mut repairs = FactRepairs::default();
        repairs.insert_if(
            repair_text(&mut self.filename, normalize_filename),
            FactRepairs::FILENAME,
        );
        repairs.insert_if(
            repair_text(&mut self.mime_type, normalize_mime_type),
            FactRepairs::MIME_TYPE,
        );
        let dimension =
            |value: Option<u32>| value.filter(|value| (1..=MAX_DIMENSION).contains(value));
        let dimensions = match (dimension(self.width), dimension(self.height)) {
            (Some(width), Some(height)) => (Some(width), Some(height)),
            _ => (None, None),
        };
        repairs.insert_if(
            dimensions != (self.width, self.height),
            FactRepairs::DIMENSIONS,
        );
        (self.width, self.height) = dimensions;
        // Dropped rather than clamped: a clamped duration would be invented.
        let duration = self
            .duration_seconds
            .filter(|seconds| *seconds <= MAX_DURATION_SECONDS);
        repairs.insert_if(duration != self.duration_seconds, FactRepairs::DURATION);
        self.duration_seconds = duration;
        repairs.insert_if(
            repair_text(&mut self.emoji, normalize_emoji),
            FactRepairs::EMOJI,
        );
        repairs
    }

    /// The attachment without any fact: only `reference`, `kind` and `source`.
    ///
    /// Ingress falls back to this when an activity still fails validation, so a
    /// fact can degrade an attachment but never lose the message carrying it.
    pub fn minimal(&self) -> Attachment {
        Attachment {
            reference: self.reference.clone(),
            kind: self.kind,
            filename: None,
            mime_type: None,
            size_bytes: None,
            source: self.source.clone(),
            variant: None,
            duration_seconds: None,
            width: None,
            height: None,
            thumbnail: None,
            emoji: None,
        }
    }
}

/// Replace a present value with its normalized form; reports whether it changed.
fn repair_text(slot: &mut Option<String>, normalize: fn(&str) -> Option<String>) -> bool {
    let Some(original) = slot.as_deref() else {
        return false;
    };
    let normalized = normalize(original);
    let changed = normalized.as_deref() != Some(original);
    *slot = normalized;
    changed
}

fn without_forbidden_chars(value: &str) -> String {
    value.chars().filter(|c| !forbidden_text_char(*c)).collect()
}

/// Shorten an oversized name on a char boundary, keeping a short ASCII
/// extension so the type of the file stays recognisable.
fn normalize_filename(value: &str) -> Option<String> {
    let cleaned = without_forbidden_chars(value);
    let name = cleaned.trim();
    if name.is_empty() {
        return None;
    }
    if name.len() <= MAX_FILENAME_BYTES {
        return Some(name.to_owned());
    }
    let shortened = match name.rsplit_once('.').filter(|(_, extension)| {
        (1..=MAX_KEPT_EXTENSION_BYTES).contains(&extension.len())
            && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }) {
        Some((stem, extension)) => {
            let budget = MAX_FILENAME_BYTES - ELLIPSIS.len() - 1 - extension.len();
            format!(
                "{}{ELLIPSIS}.{extension}",
                &stem[..stem.floor_char_boundary(budget)]
            )
        }
        None => format!(
            "{}{ELLIPSIS}",
            &name[..name.floor_char_boundary(MAX_FILENAME_BYTES - ELLIPSIS.len())]
        ),
    };
    Some(shortened)
}

/// Keep only the lowercase `type/subtype` essence; parameters such as a
/// charset are dropped and anything else is not a usable MIME type.
fn normalize_mime_type(value: &str) -> Option<String> {
    let lowered = value.trim().to_ascii_lowercase();
    let essence = lowered.split(';').next().unwrap_or_default().trim();
    mime_restricted_name(essence).then(|| essence.to_owned())
}

fn normalize_emoji(value: &str) -> Option<String> {
    let cleaned = without_forbidden_chars(value);
    (1..=MAX_EMOJI_BYTES)
        .contains(&cleaned.len())
        .then_some(cleaned)
}
