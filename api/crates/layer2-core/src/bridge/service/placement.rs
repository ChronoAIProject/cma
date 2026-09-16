use super::*;

impl BridgeService {
    /// Rollout admission only; current target/profile/owner checks still apply.
    pub fn check_new_placement(&self, config: &BridgeConfig) -> Result<(), BridgeError> {
        self.connections.check_new_placement(config)
    }

    pub(super) fn check_placement_change(
        &self,
        original: Option<&BridgeRecord>,
        change: &BridgeChange,
    ) -> Result<(), BridgeError> {
        let config = match change {
            BridgeChange::Create { config, .. } => Some(config.as_ref()),
            BridgeChange::Update { config, .. } => {
                let current = original.ok_or(BridgeError::NotFound)?.current()?;
                (current.config.placement != config.placement).then_some(config.as_ref())
            }
            BridgeChange::Enable => Some(&original.ok_or(BridgeError::NotFound)?.current()?.config),
            _ => None,
        };
        if let Some(config) = config {
            self.check_new_placement(config)?;
        }
        Ok(())
    }
}
