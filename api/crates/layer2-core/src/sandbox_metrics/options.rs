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
    OptionContext::owner(at).hints.into_iter().next()
}

/// Human context in increasing detail, shared by discovery and owner rankings.
#[derive(Clone, Debug, Default)]
pub struct OptionContext {
    hints: Vec<String>,
}

impl OptionContext {
    pub fn owner(joined_at: Option<&str>) -> Self {
        let at = joined_at.and_then(|at| {
            time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339).ok()
        });
        Self::timestamp(None, "Joined", at)
    }

    pub fn sandbox(owner: &str, owner_name: Option<&str>, created_at: Option<u64>) -> Self {
        let at = created_at
            .and_then(|at| i64::try_from(at).ok())
            .and_then(|at| time::OffsetDateTime::from_unix_timestamp(at).ok());
        Self::timestamp(public_option_name(owner, owner_name), "Created", at)
    }

    fn timestamp(hint: Option<&str>, event: &str, at: Option<time::OffsetDateTime>) -> Self {
        let mut hints: Vec<_> = hint.map(str::to_owned).into_iter().collect();
        if let Some(at) = at.and_then(|at| at.checked_to_offset(time::UtcOffset::UTC)) {
            let prefix = hint.map(|hint| format!("{hint} · ")).unwrap_or_default();
            let date = format!("{prefix}{event} {}", at.date());
            let minute = format!("{date} {:02}:{:02}", at.hour(), at.minute());
            let second = format!("{minute}:{:02}", at.second());
            hints.extend([date, format!("{minute} UTC"), format!("{second} UTC")]);
            if at.nanosecond() != 0 {
                let fraction = format!("{:09}", at.nanosecond());
                hints.push(format!("{second}.{} UTC", fraction.trim_end_matches('0')));
            }
        }
        Self { hints }
    }

    fn hint(&self, level: usize) -> Option<&str> {
        self.hints
            .get(level)
            .or(self.hints.last())
            .map(String::as_str)
    }
}

/// Compatibility for callers with only one human hint. Timestamp-aware callers
/// should use `disambiguate_options_with_context` to exhaust human distinctions.
pub fn disambiguate_options(items: &mut [FilterOption], hints: &[Option<String>]) {
    let contexts: Vec<_> = hints
        .iter()
        .map(|hint| OptionContext {
            hints: hint.iter().cloned().collect(),
        })
        .collect();
    disambiguate_options_with_context(items, &contexts);
}

/// Only duplicates on the returned page need secondary text. Use the shortest
/// distinguishing human context, then a reference only for indistinguishable ties.
pub fn disambiguate_options_with_context(items: &mut [FilterOption], contexts: &[OptionContext]) {
    let levels = contexts
        .iter()
        .map(|context| context.hints.len())
        .max()
        .unwrap_or(0);
    let hint = |index: usize, level| contexts.get(index).and_then(|context| context.hint(level));
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
            for level in 0..levels {
                if let Some(detail) = hint(index, level) {
                    if duplicates
                        .iter()
                        .filter(|(i, _)| hint(*i, level) == Some(detail))
                        .count()
                        == 1
                    {
                        return Some(detail.to_owned());
                    }
                }
            }
            let reference = format!("{:x}", Sha256::digest(item.value.as_bytes()));
            Some(match hint(index, levels) {
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
