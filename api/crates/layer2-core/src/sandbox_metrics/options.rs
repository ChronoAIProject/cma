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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disambiguator: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilterOptions {
    pub items: Vec<FilterOption>,
    pub next: Option<String>,
}

/// The primary label is always the person's or Sandbox's plain display name.
pub fn option_label(kind: OptionKind, value: &str, name: Option<&str>) -> String {
    if kind == OptionKind::Runtime {
        return if value.trim().is_empty()
            || contains_identifier(value)
            || value.chars().any(char::is_control)
        {
            "Unnamed runtime".into()
        } else {
            value.to_owned()
        };
    }
    public_option_name(value, name)
        .unwrap_or(if kind == OptionKind::Owner {
            "Unnamed owner"
        } else {
            "Unnamed Sandbox"
        })
        .to_owned()
}

pub fn public_option_name<'a>(value: &str, name: Option<&'a str>) -> Option<&'a str> {
    name.map(str::trim).filter(|name| {
        !name.is_empty()
            && !name.contains(value)
            && !contains_identifier(name)
            && !name.chars().any(char::is_control)
    })
}

pub fn option_joined(at: Option<&str>) -> Option<String> {
    let at =
        time::OffsetDateTime::parse(at?, &time::format_description::well_known::Rfc3339).ok()?;
    Some(format!("Joined {}", at.date()))
}

/// Only duplicates on the returned page need a secondary label. Prefer human
/// context; a derived reference is a last resort when that context still ties.
pub fn disambiguate_options(items: &mut [FilterOption], hints: &[Option<String>]) {
    let details: Vec<_> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let duplicates: Vec<_> = items
                .iter()
                .enumerate()
                .filter(|(_, other)| other.label == item.label)
                .collect();
            if duplicates.len() < 2 {
                return None;
            }
            let hint = hints.get(index).and_then(Option::as_deref);
            if let Some(hint) = hint {
                if duplicates
                    .iter()
                    .filter(|(i, _)| hints.get(*i).and_then(Option::as_deref) == Some(hint))
                    .count()
                    == 1
                {
                    return Some(hint.to_owned());
                }
            }
            let reference = format!("{:x}", Sha256::digest(item.value.as_bytes()));
            Some(match hint {
                Some(hint) => format!("{hint} · reference {}", &reference[..12]),
                None => format!("reference {}", &reference[..12]),
            })
        })
        .collect();
    for (item, detail) in items.iter_mut().zip(details) {
        item.disambiguator = detail;
    }
}

fn contains_identifier(name: &str) -> bool {
    name.contains("sbx_")
        || name.as_bytes().windows(36).any(|part| {
            part.iter().enumerate().all(|(i, c)| {
                if [8, 13, 18, 23].contains(&i) {
                    *c == b'-'
                } else {
                    c.is_ascii_hexdigit()
                }
            })
        })
        || name
            .split(|c: char| !c.is_ascii_hexdigit() && c != '-')
            .any(|part| {
                (part.len() == 36 && part.chars().filter(|c| *c == '-').count() == 4)
                    || (part.len() >= 32 && part.chars().all(|c| c.is_ascii_hexdigit()))
            })
}
