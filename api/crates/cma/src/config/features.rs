//! Independent rollout policy for the current CMA interfaces.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FeatureFlags {
    pub model_budget_writes_enabled: bool,
    pub model_request_budget_writes_enabled: bool,
    pub v2_ui_transport: bool,
    pub trigger_command_admission: bool,
    pub nyxid_resource_request: bool,
    pub nyxid_proxy: bool,
    pub document_revocation_reserve: bool,
    pub bounded_approval_reads: bool,
}
impl FeatureFlags {
    pub(crate) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        fn flag(
            lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
            key: &str,
        ) -> Result<bool, String> {
            match lookup(key)?.as_deref() {
                None | Some("false") => Ok(false),
                Some("true") => Ok(true),
                _ => Err(format!("{key} must be true or false")),
            }
        }
        Ok(Self {
            bounded_approval_reads: flag(lookup, "CHRONO_AGENT_APPROVAL_BOUNDED_READS_ENABLED")?,
            model_budget_writes_enabled: flag(lookup, "CHRONO_MODEL_BUDGET_WRITES_ENABLED")?,
            model_request_budget_writes_enabled: flag(
                lookup,
                "CHRONO_MODEL_REQUEST_BUDGET_WRITES_ENABLED",
            )?,
            v2_ui_transport: flag(lookup, "CHRONO_V2_UI_TRANSPORT_ENABLED")?,
            trigger_command_admission: flag(lookup, "CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED")?,
            nyxid_resource_request: flag(lookup, "CHRONO_NYXID_RESOURCE_REQUEST_ENABLED")?,
            nyxid_proxy: flag(lookup, "CHRONO_NYXID_PROXY_ENABLED")?,
            document_revocation_reserve: flag(
                lookup,
                "CHRONO_DOCUMENT_REVOCATION_RESERVE_ENABLED",
            )?,
        })
    }
}

#[cfg(test)]
mod approval_tests {
    use super::*;

    #[test]
    fn approval_reader_rollout_requires_an_explicit_boolean() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_AGENT_APPROVAL_BOUNDED_READS_ENABLED")
                    .then(|| value.map(str::to_owned))
                    .flatten())
            })
            .unwrap();
            assert_eq!(flags.bounded_approval_reads, expected);
        }
        for value in ["", "1", "TRUE"] {
            assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
                == "CHRONO_AGENT_APPROVAL_BOUNDED_READS_ENABLED")
                .then(|| value.to_owned())))
            .is_err());
        }
    }
}
