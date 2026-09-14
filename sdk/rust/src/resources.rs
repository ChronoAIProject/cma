//! Shared REST pagination and mutation authority.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// An explicit key identifying one logical mutation, including all retries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Require 1 to 255 visible ASCII characters, without spaces.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() || value.len() > 255 || !value.bytes().all(|b| (33..=126).contains(&b))
        {
            return Err(Error::validation(
                "idempotency key must contain 1 to 255 visible ASCII characters",
            ));
        }
        Ok(Self(value))
    }

    /// The unchanged value sent in the `Idempotency-Key` header.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An opaque-cursor page returned by a REST list operation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResourcePage<T, Cursor = String> {
    /// Resources in server-defined order.
    pub items: Vec<T>,
    /// Exclusive cursor for the next request, when another page exists.
    pub next: Option<Cursor>,
}

/// A complete bounded child-resource listing.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResourceList<T> {
    /// Resources returned by the parent-scoped endpoint.
    pub items: Vec<T>,
}

/// Optional opaque cursor and page size.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PageOptions {
    /// Opaque server-issued cursor; the client never parses it.
    pub cursor: Option<String>,
    /// Maximum items requested; domain limits are enforced by the service.
    pub limit: Option<u16>,
}

impl PageOptions {
    pub(crate) fn query(&self) -> Vec<(&'static str, String)> {
        let mut query = Vec::new();
        if let Some(cursor) = &self.cursor {
            query.push(("cursor", cursor.clone()));
        }
        if let Some(limit) = self.limit {
            query.push(("limit", limit.to_string()));
        }
        query
    }
}

/// Optional durable-log cursor and result bound. This is not a live subscription.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventOptions {
    /// Return entries strictly after this sequence.
    pub after: Option<u64>,
    /// Maximum durable entries requested.
    pub limit: Option<u16>,
}

impl EventOptions {
    pub(crate) fn query(self) -> Vec<(&'static str, String)> {
        let mut query = Vec::new();
        if let Some(after) = self.after {
            query.push(("after", after.to_string()));
        }
        if let Some(limit) = self.limit {
            query.push(("limit", limit.to_string()));
        }
        query
    }
}

pub(crate) fn identity(actual: &str, expected: &str) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(Error::protocol(
            "response resource identity did not match the request",
        ))
    }
}

pub(crate) fn positive(value: u64) -> Result<()> {
    if value == 0 {
        Err(Error::validation("revision must be positive"))
    } else {
        Ok(())
    }
}

pub(crate) fn deserialize_positive<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: TryFrom<u64>,
{
    let value = u64::deserialize(deserializer)?;
    if value == 0 {
        return Err(serde::de::Error::custom("expected a positive integer"));
    }
    T::try_from(value).map_err(|_| serde::de::Error::custom("integer exceeds the field range"))
}
