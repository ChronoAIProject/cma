//! Administrator filter discovery, independent of report admission and readiness.
use super::{MetricsError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionKind {
    Owner,
    Sandbox,
    Runtime,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionQuery {
    pub kind: OptionKind,
    #[serde(default)]
    pub search: String,
    pub owner: Option<String>,
    pub selected: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
impl OptionQuery {
    pub fn validate(&self) -> Result<u16> {
        let limit = self.limit.unwrap_or(50);
        let invalid_text =
            |value: &str, max: usize| value.len() > max || value.chars().any(char::is_control);
        if !(1..=50).contains(&limit)
            || invalid_text(&self.search, 128)
            || self
                .owner
                .as_deref()
                .is_some_and(|v| v.is_empty() || invalid_text(v, 255))
            || self
                .selected
                .as_deref()
                .is_some_and(|v| v.is_empty() || invalid_text(v, 255))
            || self
                .cursor
                .as_deref()
                .is_some_and(|v| v.is_empty() || v.len() > 4096)
            || (self.kind != OptionKind::Sandbox && self.owner.is_some())
            || (self.selected.is_some() && (self.cursor.is_some() || !self.search.is_empty()))
        {
            return Err(MetricsError::Invalid("Invalid filter option query.".into()));
        }
        Ok(limit)
    }
    pub fn cursor_key(&self) -> Result<String> {
        let bytes = serde_json::to_vec(&(self.kind, &self.search, &self.owner, &self.selected))
            .map_err(|_| MetricsError::Unavailable)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilterOption {
    pub value: String,
    pub label: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilterOptions {
    pub items: Vec<FilterOption>,
    pub next: Option<String>,
}

/// Stable references distinguish equal or missing names across pages and searches.
/// References are hashes, never fragments of the underlying identifier.
pub fn option_label(kind: OptionKind, value: &str, name: Option<&str>) -> String {
    if kind == OptionKind::Runtime {
        return value.to_owned();
    }
    let (fallback, noun) = match kind {
        OptionKind::Owner => ("Unnamed owner", "account"),
        _ => ("Unnamed Sandbox", "Sandbox"),
    };
    let name = name
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.contains(value) && !contains_identifier(name))
        .unwrap_or(fallback);
    let reference = format!("{:x}", Sha256::digest(value.as_bytes()));
    format!("{name} · {noun} {}", &reference[..12])
}

fn contains_identifier(name: &str) -> bool {
    name.contains("sbx_")
        || name
            .split(|c: char| !c.is_ascii_hexdigit() && c != '-')
            .any(|part| {
                (part.len() == 36 && part.chars().filter(|c| *c == '-').count() == 4)
                    || (part.len() >= 32 && part.chars().all(|c| c.is_ascii_hexdigit()))
            })
}
