use super::{writers_held_error, Phase, Supervisor};
use crate::failure::{Result, RuntimeError};
use crate::install_state::RecoveryState;

impl Supervisor {
    pub(super) async fn recover_journals(&self) -> Result<()> {
        let _operation = self.operation_gate.lock().await;
        if self.held_checkpoint.lock().await.is_some() {
            return Err(writers_held_error());
        }
        self.recover_journals_locked().await
    }

    pub(super) async fn recover_journals_locked(&self) -> Result<()> {
        self.mutations.verify()?;
        let home = crate::persistent_home::verify(&self.config)?;
        let durable = crate::install::recover(&self.config)?;
        let restored = home
            .as_ref()
            .map(|home| home.restore_completed(&durable))
            .transpose()?
            .unwrap_or(false);
        let mut phase = self.phase.write().await;
        let mut bootstrap = self.restore_bootstrap.write().await;
        let consistent = match durable {
            RecoveryState::Completed if bootstrap.is_none() => match *phase {
                Phase::Provisioned => {
                    *phase = Phase::Installed;
                    true
                }
                Phase::Installed | Phase::Credentialed => true,
                Phase::RestorePending | Phase::RestoreCredentialed => false,
            },
            RecoveryState::Inherited(receipt) => {
                if bootstrap.is_none() && *phase == Phase::Provisioned {
                    *bootstrap = Some(receipt.clone());
                    *phase = Phase::RestorePending;
                }
                if bootstrap.as_ref() != Some(&receipt) {
                    false
                } else if home.is_some() {
                    // Completion may have reached disk before its response or
                    // in-memory phase update. Recover it without re-extraction.
                    if restored {
                        *phase = match *phase {
                            Phase::RestorePending => Phase::Installed,
                            Phase::RestoreCredentialed => Phase::Credentialed,
                            other => other,
                        };
                        matches!(*phase, Phase::Installed | Phase::Credentialed)
                    } else {
                        matches!(*phase, Phase::RestorePending | Phase::RestoreCredentialed)
                    }
                } else {
                    matches!(
                        *phase,
                        Phase::RestorePending | Phase::RestoreCredentialed | Phase::Credentialed
                    )
                }
            }
            RecoveryState::Absent | RecoveryState::Terminal => {
                bootstrap.is_none() && *phase == Phase::Provisioned
            }
            RecoveryState::Completed => false,
        };
        if !consistent {
            return Err(RuntimeError::rejected(
                "install_state_invalid",
                "runtime lifecycle disagrees with its durable installation state",
            ));
        }
        drop(bootstrap);
        drop(phase);
        crate::workspace_runtime::recover_locked(&self.config, self).await?;
        self.recover_mutations_locked().await
    }
}
