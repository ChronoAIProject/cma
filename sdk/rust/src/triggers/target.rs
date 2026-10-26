//! Inline targets may omit legacy references; legacy targets must provide all three.
use super::{Target, TriggerInlineTarget};
use crate::profiles::ProfileReference;
use serde::{Deserialize, Deserializer, de::Error};

fn empty_reference() -> ProfileReference {
    ProfileReference {
        id: String::new(),
        revision: 1,
    }
}

impl Target {
    /// Configure a Trigger directly without selecting legacy profile revisions.
    pub fn inline(configuration: TriggerInlineTarget) -> Self {
        Self {
            sandbox_profile: empty_reference(),
            workspace_profile: empty_reference(),
            agent_profile: empty_reference(),
            inline: Some(configuration),
        }
    }
}

#[derive(Deserialize)]
struct WireReference {
    id: String,
    revision: u32,
}

fn reference<E: Error>(value: Option<WireReference>, inline: bool) -> Result<ProfileReference, E> {
    match value {
        Some(value) if value.revision > 0 => Ok(ProfileReference {
            id: value.id,
            revision: value.revision,
        }),
        Some(value) if inline && value.id.is_empty() => Ok(empty_reference()),
        None if inline => Ok(empty_reference()),
        _ => Err(E::custom(
            "profile revisions must be positive; only inline targets permit empty legacy references",
        )),
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct WireTarget {
            sandbox_profile: Option<WireReference>,
            workspace_profile: Option<WireReference>,
            agent_profile: Option<WireReference>,
            inline: Option<TriggerInlineTarget>,
        }
        let value = WireTarget::deserialize(deserializer)?;
        let inline = value.inline.is_some();
        Ok(Self {
            sandbox_profile: reference(value.sandbox_profile, inline)?,
            workspace_profile: reference(value.workspace_profile, inline)?,
            agent_profile: reference(value.agent_profile, inline)?,
            inline: value.inline,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn inline() -> Value {
        json!({"agent": {"reasoning_effort": "medium", "permission_mode": "guarded",
            "skills": [], "system_prompt": "", "services": []}})
    }

    #[test]
    fn inline_target_accepts_missing_or_empty_legacy_profile_fields() {
        for fields in [
            json!({}),
            json!({
                "sandbox_profile": {"id": "", "revision": 0},
                "workspace_profile": {"id": "", "revision": 0},
                "agent_profile": {"id": "", "revision": 0}
            }),
        ] {
            let mut document = fields;
            document["inline"] = inline();
            let target: Target = serde_json::from_value(document).unwrap();
            assert_eq!(target.sandbox_profile, empty_reference());
            assert_eq!(target.workspace_profile, empty_reference());
            assert_eq!(target.agent_profile, empty_reference());
            assert_eq!(target, Target::inline(target.inline.clone().unwrap()));
            let round_trip: Target =
                serde_json::from_value(serde_json::to_value(&target).unwrap()).unwrap();
            assert_eq!(round_trip, target);
        }
    }

    #[test]
    fn legacy_target_requires_all_three_positive_references() {
        let legacy = json!({
            "sandbox_profile": {"id": "sbp_one", "revision": 1},
            "workspace_profile": {"id": "wkp_one", "revision": 2},
            "agent_profile": {"id": "agp_one", "revision": 3}
        });
        assert!(serde_json::from_value::<Target>(legacy.clone()).is_ok());
        assert!(serde_json::from_value::<Target>(json!({})).is_err());
        for field in ["sandbox_profile", "workspace_profile", "agent_profile"] {
            let mut missing = legacy.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<Target>(missing).is_err());
            for id in ["", "profile_one"] {
                let mut invalid = legacy.clone();
                invalid[field] = json!({"id": id, "revision": 0});
                assert!(serde_json::from_value::<Target>(invalid.clone()).is_err());
                if !id.is_empty() {
                    invalid["inline"] = inline();
                    assert!(serde_json::from_value::<Target>(invalid).is_err());
                }
            }
        }
    }
}
