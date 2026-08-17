//! The allowlist: which parameters of an action may be written down.
//!
//! This file is the security boundary of the record. [`DetailKey`] is a closed
//! set and [`ActivityDetail`] accepts nothing else, so a field added to a
//! request later cannot reach durable storage by default — there is simply no
//! key for it. A denylist would do the opposite: it admits everything nobody
//! thought to forbid, which is how audit logs come to contain credentials.
//!
//! Read the set below for what is *absent*. There is no key whose value is a
//! variable's value, a credential, a token, a cookie or a request header, and
//! the counting keys exist precisely so a bundle of variables can be described
//! without any of its contents being kept.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Longest detail value kept.
///
/// Bounded so one entry cannot grow without limit: these are identifiers,
/// numbers and short names, and anything longer is a caller sending something
/// this log was not meant to hold.
const MAX_DETAIL_VALUE: usize = 256;

/// A parameter that may be recorded against an action.
///
/// The allowlist. Adding a key here is a deliberate decision that the thing it
/// names is safe to keep forever, which is the review this type exists to
/// force. Note what is absent and cannot be added by accident: there is no key
/// for a variable's value, a credential, a token, or a request header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DetailKey {
    /// Which runtime implementation was asked for.
    Runtime,
    /// Terminal width asked for, in columns.
    Columns,
    /// Terminal height asked for, in rows.
    Rows,
    /// Which reusable bundle was selected, by identifier.
    EnvironmentProfile,
    /// How much the agent was to be allowed to do unattended.
    ///
    /// Kept because it is the single most consequential thing a caller chooses:
    /// an unattended agent doing more than somebody expected is a question that
    /// gets asked afterwards, and this is the answer.
    PermissionMode,
    /// Which model was selected.
    Model,
    /// The name a caller gave a resource.
    ResourceName,
    /// How many variables a bundle carried. The count, never the entries.
    VariableCount,
    /// How many packages a bundle carried.
    PackageCount,
    /// How many skills a bundle carried.
    SkillCount,
    /// The position in a session's history a caller resumed from.
    Cursor,
    /// How many entries a caller asked for.
    Limit,
    /// A subject the action concerned, when it is not the actor.
    Subject,
    /// The plane a session was asked to be moved onto.
    ///
    /// One of two words, neither of which is the owner's content: which
    /// surface a session is driven on is exactly what a reader of this record
    /// needs to make sense of a handover somebody asked for.
    Plane,
}

impl DetailKey {
    /// The stable string this key is stored and rendered under.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Columns => "columns",
            Self::Rows => "rows",
            Self::EnvironmentProfile => "environment_profile",
            Self::PermissionMode => "permission_mode",
            Self::Model => "model",
            Self::ResourceName => "resource_name",
            Self::VariableCount => "variable_count",
            Self::PackageCount => "package_count",
            Self::SkillCount => "skill_count",
            Self::Cursor => "cursor",
            Self::Limit => "limit",
            Self::Subject => "subject",
            Self::Plane => "plane",
        }
    }

    /// The key a stored string names, if it names one at all.
    ///
    /// Unknown strings are refused rather than carried through. The allowlist
    /// has to hold on the way out as well as on the way in, or a key written by
    /// something that predates this set would reappear unreviewed.
    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::Runtime,
            Self::Columns,
            Self::Rows,
            Self::EnvironmentProfile,
            Self::PermissionMode,
            Self::Model,
            Self::ResourceName,
            Self::VariableCount,
            Self::PackageCount,
            Self::SkillCount,
            Self::Cursor,
            Self::Limit,
            Self::Subject,
            Self::Plane,
        ]
        .into_iter()
        .find(|k| k.as_str() == code)
    }
}

/// The parameters of an action, as they may be kept.
///
/// A map that only accepts allowlisted keys, and truncates what it is given.
/// Constructing one is the only way parameters reach the record, so there is
/// no path by which an unreviewed field becomes a stored one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityDetail(BTreeMap<DetailKey, String>);

impl ActivityDetail {
    /// An empty detail map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one parameter, replacing any previous value for that key.
    pub fn record(&mut self, key: DetailKey, value: impl Into<String>) {
        let mut value = value.into();
        if value.len() > MAX_DETAIL_VALUE {
            // Cut back to a character boundary: the value came from a caller
            // and may be anything, and truncating a multi-byte string by bytes
            // would panic on their input rather than shorten it.
            let mut cut = MAX_DETAIL_VALUE;
            while cut > 0 && !value.is_char_boundary(cut) {
                cut -= 1;
            }
            value.truncate(cut);
        }
        self.0.insert(key, value);
    }

    /// Whether nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every parameter kept, in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = (DetailKey, &str)> + '_ {
        self.0.iter().map(|(k, v)| (*k, v.as_str()))
    }

    /// Read one parameter back.
    pub fn get(&self, key: DetailKey) -> Option<&str> {
        self.0.get(&key).map(String::as_str)
    }

    /// Build from stored strings, dropping any key that is not allowlisted.
    pub fn from_stored(entries: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut detail = Self::new();
        for (key, value) in entries {
            match DetailKey::from_code(&key) {
                Some(k) => detail.record(k, value),
                None => tracing::warn!(
                    key = %key,
                    "a stored parameter names no allowlisted key and was dropped"
                ),
            }
        }
        detail
    }
}

impl Serialize for ActivityDetail {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key.as_str(), value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ActivityDetail {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = BTreeMap::<String, String>::deserialize(deserializer)?;
        Ok(Self::from_stored(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_allowlisted_key_round_trips_through_its_stored_form() {
        // The allowlist has to hold in both directions, or an entry written
        // today becomes unreadable — or worse, readable under a key nobody
        // reviewed — tomorrow.
        for key in [
            DetailKey::Runtime,
            DetailKey::Columns,
            DetailKey::Rows,
            DetailKey::EnvironmentProfile,
            DetailKey::PermissionMode,
            DetailKey::Model,
            DetailKey::ResourceName,
            DetailKey::VariableCount,
            DetailKey::PackageCount,
            DetailKey::SkillCount,
            DetailKey::Cursor,
            DetailKey::Limit,
            DetailKey::Subject,
        ] {
            assert_eq!(
                DetailKey::from_code(key.as_str()),
                Some(key),
                "{key:?} does not survive its own stored form"
            );
        }
    }

    #[test]
    fn a_key_outside_the_allowlist_is_refused() {
        // The property the whole design rests on: something not on the list
        // has no way in, however it is spelled.
        for code in [
            "value",
            "variables",
            "authorization",
            "cookie",
            "token",
            "password",
            "",
        ] {
            assert_eq!(DetailKey::from_code(code), None, "{code:?} must be refused");
        }
    }

    #[test]
    fn stored_entries_outside_the_allowlist_are_dropped_on_the_way_out() {
        let detail = ActivityDetail::from_stored([
            ("runtime".to_string(), "example-runtime".to_string()),
            (
                "authorization".to_string(),
                "should never appear".to_string(),
            ),
        ]);
        assert_eq!(detail.get(DetailKey::Runtime), Some("example-runtime"));
        let rendered = serde_json::to_string(&detail).expect("serialises");
        assert!(!rendered.contains("should never appear"), "got {rendered}");
        assert!(!rendered.contains("authorization"), "got {rendered}");
    }

    #[test]
    fn a_long_value_is_truncated_rather_than_kept_whole() {
        let mut detail = ActivityDetail::new();
        detail.record(DetailKey::ResourceName, "x".repeat(4096));
        let kept = detail.get(DetailKey::ResourceName).expect("recorded");
        assert!(kept.len() <= MAX_DETAIL_VALUE, "kept {} bytes", kept.len());
    }

    #[test]
    fn truncation_does_not_split_a_multi_byte_character() {
        // A value arriving from a caller can be anything; slicing it by bytes
        // would panic on the wrong input.
        let mut detail = ActivityDetail::new();
        detail.record(DetailKey::ResourceName, "é".repeat(400));
        let kept = detail.get(DetailKey::ResourceName).expect("recorded");
        assert!(kept.chars().all(|c| c == 'é'), "got {kept}");
    }
}
