//! Explicit admission cohort; no implicit process-start cutoff or historical backfill.
use serde::{Deserialize, Serialize};
use std::time::{Duration, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    pub from: String,
    pub to: Option<String>,
    pub overdue_seconds: u64,
}
impl Selection {
    pub(crate) fn bounds(&self, observed: u64) -> Result<(u64, u64), &'static str> {
        let canonical = |value: &str| {
            layer2_core::seconds_from_rfc3339(value)
                .filter(|seconds| {
                    layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_secs(*seconds)) == value
                })
                .ok_or("invalid_selection")
        };
        let from = canonical(&self.from)?;
        let to = self.to.as_deref().map(canonical).transpose()?;
        if to.is_some_and(|to| to <= from) || !(60..=604_800).contains(&self.overdue_seconds) {
            return Err("invalid_selection");
        }
        let through = to.unwrap_or(observed).min(observed);
        if through < from {
            return Err("not_started");
        }
        Ok((from, through))
    }
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        let Some(value) = lookup("CHRONO_TASK_AUDIT_SELECTION")? else {
            return Ok(Some(Self {
                from: "2026-11-24T16:00:00Z".into(),
                to: None,
                overdue_seconds: 3600,
            }));
        };
        let parse = || {
            if value.len() > 512 {
                return Err(());
            }
            let selection: Option<Self> = serde_json::from_str(&value).map_err(|_| ())?;
            // Future cutoffs are valid configuration; the sampler reports not_started.
            if let Some(selection) = &selection {
                selection.bounds(u64::MAX / 2).map_err(|_| ())?;
            }
            Ok(selection)
        };
        parse().map_err(|()| "CHRONO_TASK_AUDIT_SELECTION requires JSON null or canonical UTC from/to and overdue_seconds in 60..=604800".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_defaults_to_fixed_release_cutoff_and_null_disables() {
        let default = Selection::from_lookup(&mut |_| Ok(None)).unwrap().unwrap();
        assert_eq!(default.from, "2026-11-24T16:00:00Z");
        assert_eq!(default.to, None);
        assert_eq!(default.overdue_seconds, 3600);
        let from = layer2_core::seconds_from_rfc3339(&default.from).unwrap();
        assert_eq!(default.bounds(from - 1), Err("not_started"));
        assert_eq!(default.bounds(from + 10), Ok((from, from + 10)));
        assert_eq!(default.bounds(from + 86400), Ok((from, from + 86400)));
        assert!(Selection::from_lookup(&mut |_| Ok(Some("null".into())))
            .unwrap()
            .is_none());
        assert!(Selection::from_lookup(&mut |_| Ok(Some(" \nnull ".into())))
            .unwrap()
            .is_none());
        assert!(Selection::from_lookup(&mut |_| Err("lookup failed".into())).is_err());
    }

    #[test]
    fn fixed_cutoff_survives_restart_and_rejects_ambiguous_policy() {
        let value =
            r#"{"from":"2026-01-01T00:00:00Z","to":"2026-01-02T00:00:00Z","overdue_seconds":3600}"#;
        let selection = Selection::from_lookup(&mut |_| Ok(Some(value.into())))
            .unwrap()
            .unwrap();
        let from = layer2_core::seconds_from_rfc3339(&selection.from).unwrap();
        assert_eq!(selection.bounds(from - 1), Err("not_started"));
        assert_eq!(selection.bounds(from + 60).unwrap(), (from, from + 60));
        assert_eq!(
            selection.bounds(from + 200_000).unwrap(),
            (from, from + 86_400)
        );
        for bad in [
            String::new(),
            "{}".into(),
            "\"null\"".into(),
            "false".into(),
            " ".repeat(513),
            value.replace("00:00:00Z", "00:00:00+00:00"),
            value.replace("3600", "0"),
            value.replace("2026-01-02", "2025-01-02"),
            value.replace("3600}", "3600,\"apply\":true}"),
        ] {
            assert!(Selection::from_lookup(&mut |_| Ok(Some(bad.clone()))).is_err());
        }
    }
}
