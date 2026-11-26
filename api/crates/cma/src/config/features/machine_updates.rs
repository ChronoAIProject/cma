use layer2_core::SandboxId;
use uuid::Uuid;

const SETTING: &str = "CHRONO_MANAGED_MACHINE_RUNTIME_UPDATES";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ManagedMachineUpdates {
    #[default]
    Disabled,
    Canary(Uuid),
    All,
}

impl ManagedMachineUpdates {
    pub(super) fn read(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        match lookup(SETTING)?.as_deref() {
            None | Some("disabled") => Ok(Self::Disabled),
            Some("all") => Ok(Self::All),
            Some(value) => sandbox_uuid(value)
                .map(Self::Canary)
                .ok_or_else(|| format!("{SETTING} requires disabled, all, or a Sandbox ID")),
        }
    }

    pub(crate) fn includes(self, id: &SandboxId) -> bool {
        match self {
            Self::Disabled => false,
            Self::Canary(expected) => sandbox_uuid(id.as_str()) == Some(expected),
            Self::All => true,
        }
    }
}

fn sandbox_uuid(value: &str) -> Option<Uuid> {
    let suffix = value.strip_prefix("sbx_")?;
    let id = Uuid::parse_str(suffix).ok()?;
    (suffix == id.simple().to_string()).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_canary_scope_fail_closed() {
        let allowed = SandboxId::generate();
        let other = SandboxId::generate();
        assert!(!ManagedMachineUpdates::read(&mut |_| Ok(None))
            .unwrap()
            .includes(&allowed));
        assert!(ManagedMachineUpdates::read(&mut |_| Ok(Some("all".into())))
            .unwrap()
            .includes(&other));
        let canary = ManagedMachineUpdates::read(&mut |_| Ok(Some(allowed.to_string()))).unwrap();
        assert!(canary.includes(&allowed));
        assert!(!canary.includes(&other));
        assert!(ManagedMachineUpdates::read(&mut |_| Ok(Some("true".into()))).is_err());
    }
}
