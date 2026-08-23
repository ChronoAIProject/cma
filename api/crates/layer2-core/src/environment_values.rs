//! What a profile is made of, and the limits every one of them obeys.
//!
//! Split from the profile itself so that file is about a bundle and this one
//! about the values in it. The division is not arbitrary: everything here is a
//! thing a caller names — a variable, a package, a skill, a service — and the
//! rules that make one storable.
//!
//! **Values are never rendered.** A variable's value is exactly the place a
//! caller will put a token, so `Debug` redacts it and nothing here logs one.

use serde::{Deserialize, Serialize};

/// Longest accepted profile name.
pub(super) const MAX_NAME: usize = 120;
/// Longest accepted variable value.
///
/// Generous enough for a certificate or a long token, short enough that a
/// profile cannot be used as a general-purpose file store.
pub(super) const MAX_VALUE: usize = 8192;
/// Most entries any one list may carry.
///
/// A bound rather than a limit anyone will meet: it exists so a single profile
/// cannot grow until it is too large for the store that holds it.
pub(super) const MAX_ENTRIES: usize = 100;

/// One environment variable handed to the session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentVariable {
    /// Variable name, as the process will see it.
    pub key: String,
    /// Value, opaque to the core.
    pub value: String,
}

impl std::fmt::Debug for EnvironmentVariable {
    /// Redacted, so no accidental `{:?}` puts a caller's token in a log.
    ///
    /// The key is safe to print and is what makes a log line useful; the value
    /// is the part a caller will inevitably use for a credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "EnvironmentVariable {{ key: {:?}, value: <redacted {} bytes> }}",
            self.key,
            self.value.len()
        )
    }
}

impl EnvironmentVariable {
    /// Build a variable.
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }

    /// Whether a name is usable as an environment variable.
    ///
    /// The POSIX shape: a letter or underscore, then letters, digits, or
    /// underscores. Anything else is rejected rather than sanitised, because a
    /// silently renamed variable is one the session will not find.
    pub fn is_valid_key(key: &str) -> bool {
        let mut chars = key.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
}

/// A piece of software the session should have available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftwarePackage {
    /// Package name, as the runtime image understands it.
    pub name: String,
    /// Version to pin, when the caller cares which one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl SoftwarePackage {
    /// A package at whatever version the runtime provides.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: None,
        }
    }

    /// How this package is named to whatever installs it.
    pub fn requirement(&self) -> String {
        match &self.version {
            Some(v) => format!("{}@{}", self.name, v),
            None => self.name.clone(),
        }
    }
}

/// Identifier of a skill in the catalogue.
///
/// Opaque: the core stores and forwards it, and only the catalogue knows how
/// to resolve one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SkillId(pub String);

/// A skill a session was given, and which version of it arrived.
///
/// The version rides with the identifier because they are one fact: "this
/// session has nyxid" is not something anybody can act on without knowing
/// which nyxid. Optional because a registry need not report one, and a session
/// recorded before this did not carry it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GrantedSkill {
    pub id: SkillId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A service on the identity platform, as that platform names it.
///
/// Opaque here, exactly like the credential itself: the core carries the
/// selection from a profile to whatever mints a credential, and never asks
/// what any of them mean. Only the provider knows.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ServiceId(pub String);

impl ServiceId {
    /// Wrap an identifier.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The identifier as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ServiceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl SkillId {
    /// Wrap an identifier.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SkillId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
