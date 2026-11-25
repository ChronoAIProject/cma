//! Administrator filter discovery, independent of report admission and readiness.
use super::{MetricsError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

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
        if let Some(label) = pinned_runtime_label(value) {
            return label;
        }
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

/// OCI pins expose the repository basename and the leading digest shown in
/// Sandbox details. General identifier redaction still applies to the name.
fn pinned_runtime_label(value: &str) -> Option<String> {
    let (image, digest) = value.rsplit_once("@sha256:")?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let name = image.rsplit('/').next().filter(|name| {
        !name.is_empty()
            && !contains_identifier(name)
            && name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':')
            })
    });
    Some(format!(
        "{} · sha256:{}",
        name.unwrap_or("Runtime image"),
        &digest[..12]
    ))
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

/// Current-call authority for directory observation precision; never query input.
#[derive(Clone, Copy, Debug, Default)]
pub enum OwnerContextAccess {
    #[default]
    DateOnly,
    DirectoryRead,
}
impl OwnerContextAccess {
    pub fn from_authorization(authorization: &crate::Authorization) -> Self {
        if authorization.allows(crate::Capability::ReadUserDirectory) {
            Self::DirectoryRead
        } else {
            Self::DateOnly
        }
    }
}

/// Human context in increasing detail, shared by discovery and owner rankings.
#[derive(Clone, Debug, Default)]
pub struct OptionContext {
    hints: Vec<String>,
}

impl OptionContext {
    /// Safe default for callers without independently verified directory access.
    pub fn owner(joined_at: Option<&str>) -> Self {
        Self::owner_with_access(joined_at, OwnerContextAccess::DateOnly)
    }

    pub fn owner_with_access(joined_at: Option<&str>, access: OwnerContextAccess) -> Self {
        let at = joined_at.and_then(|at| {
            time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339).ok()
        });
        let mut context = Self::timestamp(None, "Joined", at);
        if matches!(access, OwnerContextAccess::DateOnly) {
            context.hints.truncate(1);
        }
        context
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

/// Only duplicates in the supplied set need secondary text. Use the shortest
/// distinguishing human context, then a reference only for indistinguishable ties.
pub fn disambiguate_options_with_context(items: &mut [FilterOption], contexts: &[OptionContext]) {
    let mut counts = HashMap::new();
    for item in items.iter() {
        *counts.entry(item.label.as_str()).or_insert(0_usize) += 1;
    }
    let duplicate: Vec<_> = items
        .iter()
        .map(|item| counts[item.label.as_str()] > 1)
        .collect();
    let mut levels = vec![0; items.len()];
    let mut references = vec![0; items.len()];
    loop {
        let details: Vec<_> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                if !duplicate[i] {
                    return None;
                }
                let hint = contexts.get(i).and_then(|context| context.hint(levels[i]));
                if references[i] == 0 {
                    return hint.map(str::to_owned);
                }
                let digest = format!("{:x}", Sha256::digest(item.value.as_bytes()));
                let reference = match references[i] {
                    1 => format!("reference {}", &digest[..12]),
                    2 => format!("reference {digest}"),
                    _ => format!("reference {digest} · choice {}", i + 1),
                };
                Some(
                    hint.map(|hint| format!("{hint} · {reference}"))
                        .unwrap_or(reference),
                )
            })
            .collect();
        // Compare the strings actually chosen for display, even when different
        // rows have reached different precision levels or contain generated-looking names.
        let mut groups: HashMap<_, Vec<usize>> = HashMap::new();
        for (i, item) in items.iter().enumerate().filter(|(i, _)| duplicate[*i]) {
            groups
                .entry((item.label.as_str(), details[i].as_deref()))
                .or_default()
                .push(i);
        }
        let collisions: Vec<_> = groups
            .into_values()
            .filter(|group| group.len() > 1)
            .flatten()
            .collect();
        if collisions.is_empty() {
            for (item, detail) in items.iter_mut().zip(details) {
                item.disambiguator = detail;
            }
            return;
        }
        let mut advanced = false;
        for &i in &collisions {
            if references[i] == 0
                && contexts
                    .get(i)
                    .is_some_and(|context| levels[i] + 1 < context.hints.len())
            {
                levels[i] += 1;
                advanced = true;
            }
        }
        if advanced {
            continue;
        }
        // Exhausted rows can keep their shorter hint while peers gain human
        // detail. Add references only when no colliding row has human detail left.
        for &i in &collisions {
            if (1..3).contains(&references[i]) {
                references[i] += 1;
                advanced = true;
            }
        }
        if !advanced {
            for i in collisions {
                if references[i] == 0 {
                    references[i] = 1;
                }
            }
        }
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
