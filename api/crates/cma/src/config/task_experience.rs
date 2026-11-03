use layer2_core::sandbox::task_experience::TaskExperiencePolicy;
use std::sync::Arc;

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Arc<TaskExperiencePolicy>, String> {
    match lookup("CHRONO_TASK_EXPERIENCE_POLICY")? {
        None => Ok(Arc::default()),
        Some(value) => serde_json::from_str(&value).map(Arc::new).map_err(|_| {
            "CHRONO_TASK_EXPERIENCE_POLICY must be a valid bounded task experience policy".into()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{sandbox::task_experience::TaskExperienceCapability::*, SubjectId};

    #[test]
    fn task_experience_config_defaults_off_and_selects_exact_accounts() {
        assert!(!from_lookup(&mut |_| Ok(None))
            .unwrap()
            .allows(&SubjectId::new("owner-a"), TaskHomeAdmission));
        let policy = from_lookup(&mut |_| Ok(Some(r#"{"mode":"accounts","accounts":["owner-a"],"capabilities":["task_home_admission"]}"#.into()))).unwrap();
        assert!(policy.allows(&SubjectId::new("owner-a"), TaskHomeAdmission));
        assert!(!policy.allows(&SubjectId::new("owner-ab"), TaskHomeAdmission));
        assert!(!policy.allows(&SubjectId::new("owner-a"), DesktopEnable));
    }

    #[test]
    fn task_experience_config_rejects_unknown_duplicate_and_contradictory_values() {
        for value in [
            "",
            "{}",
            r#"{"mode":"random","accounts":[],"capabilities":[]}"#,
            r#"{"mode":"all","accounts":["owner-a"],"capabilities":[]}"#,
            r#"{"mode":"disabled","accounts":[],"capabilities":["desktop_enable"]}"#,
            r#"{"mode":"all","accounts":[],"capabilities":["everything"]}"#,
            r#"{"mode":"all","accounts":[],"capabilities":[],"extra":true}"#,
            r#"{"mode":"accounts","accounts":["owner-a","owner-a"],"capabilities":[]}"#,
            r#"{"mode":"accounts","accounts":[" owner-a"],"capabilities":[]}"#,
            r#"{"mode":"all","accounts":[],"capabilities":["desktop_enable","desktop_enable"]}"#,
        ] {
            let error = from_lookup(&mut |_| Ok(Some(value.into()))).unwrap_err();
            assert!(!error.contains("owner-a"));
        }
    }
}
