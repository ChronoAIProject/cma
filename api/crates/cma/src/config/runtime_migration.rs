//! Operator-owned policy and evidence publication paths; no request-supplied files.
use layer2_core::sandbox::RuntimeMigrationPolicy;
use std::{fs::File, io::Read, path::PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RuntimeMigrationConfig {
    pub enabled: bool,
    pub policy: Option<RuntimeMigrationPolicy>,
    pub bundle_path: Option<PathBuf>,
}

impl RuntimeMigrationConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let enabled = match lookup("CHRONO_RUNTIME_MIGRATION_ENABLED")?.as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            _ => return Err("CHRONO_RUNTIME_MIGRATION_ENABLED must be true or false".into()),
        };
        let path = lookup("CHRONO_RUNTIME_MIGRATION_POLICY_FILE")?.filter(|path| !path.is_empty());
        let bundle_path = lookup("CHRONO_RUNTIME_MIGRATION_BUNDLE_FILE")?
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        let policy = path
            .map(|path| {
                let mut bytes = Vec::new();
                File::open(path)
                    .map_err(|_| "runtime migration policy unavailable")?
                    .take(65_537)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "runtime migration policy unreadable")?;
                if bytes.len() > 65_536 {
                    return Err("runtime migration policy exceeds bound");
                }
                let mut policy: RuntimeMigrationPolicy = serde_json::from_slice(&bytes)
                    .map_err(|_| "runtime migration policy malformed")?;
                // Every restart begins a new local source incarnation and baseline.
                // This is server binding at startup, never renewal of supplied proof.
                for source in &mut policy.qualified_sources {
                    source.process_incarnation =
                        layer2_core::sandbox::runtime_desktop_process_incarnation().to_owned();
                    source
                        .validate()
                        .map_err(|_| "runtime migration source unsupported")?;
                }
                policy
                    .validate()
                    .map_err(|_| "runtime migration policy invalid")?;
                Ok::<_, &str>(policy)
            })
            .transpose()
            .map_err(str::to_owned)?;
        if enabled && (policy.is_none() || bundle_path.is_none()) {
            return Err(
                "runtime migration activation requires policy and bundle publication paths".into(),
            );
        }
        if policy.is_some() != bundle_path.is_some() {
            return Err(
                "runtime migration policy and bundle paths must be configured together".into(),
            );
        }
        Ok(Self {
            enabled,
            policy,
            bundle_path,
        })
    }
}
