use crate::{Error, Result};
use serde::{Deserialize, Deserializer};

/// Opaque Agent resume coordinate, separate from standard Responses sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCursor(String);
impl AgentCursor {
    /// Validate a retained opaque coordinate without treating it as authority.
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let parts = parts(&value, "ag1", 3)?;
        coordinate(parts[2])?;
        Ok(Self(value))
    }
    /// Exact retained wire coordinate.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(super) fn sequence(&self) -> u64 {
        self.0.rsplit('.').next().unwrap().parse().unwrap()
    }
    pub(super) fn incarnation(&self) -> &str {
        self.0.split('.').nth(1).unwrap()
    }
}
impl<'de> Deserialize<'de> for AgentCursor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
/// Opaque continuation binding a snapshot incarnation, fixed head and delivered page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPageCursor(String);
impl AgentPageCursor {
    /// Validate a server-issued snapshot continuation token.
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let parts = parts(&value, "ags1", 4)?;
        if coordinate(parts[3])? >= coordinate(parts[2])? {
            return Err(Error::validation(
                "snapshot continuation must precede its fixed head",
            ));
        }
        Ok(Self(value))
    }
    /// Exact retained continuation token.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(super) fn incarnation(&self) -> &str {
        self.0.split('.').nth(1).unwrap()
    }
    pub(super) fn coordinates(&self) -> (u64, u64) {
        let p: Vec<_> = self.0.split('.').collect();
        (p[2].parse().unwrap(), p[3].parse().unwrap())
    }
}
impl<'de> Deserialize<'de> for AgentPageCursor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
fn parts<'a>(value: &'a str, family: &str, count: usize) -> Result<Vec<&'a str>> {
    if value.len() > 111 {
        return Err(Error::validation("Agent cursor exceeds its bound"));
    }
    let p: Vec<_> = value.split('.').collect();
    if p.len() != count || p[0] != family || !valid_incarnation(p[1]) {
        return Err(Error::validation(
            "invalid Agent cursor family or incarnation",
        ));
    }
    Ok(p)
}
fn coordinate(value: &str) -> Result<u64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Error::validation("noncanonical Agent cursor sequence"));
    }
    value
        .parse()
        .map_err(|_| Error::validation("Agent cursor sequence overflow"))
}
pub(super) fn valid_incarnation(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn identifier(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(Error::validation("invalid Agent event identifier"));
    }
    Ok(())
}
